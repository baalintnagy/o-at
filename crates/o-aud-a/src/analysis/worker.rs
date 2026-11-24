//! The analysis worker thread: drains the capture ring into the input's [`Engine`],
//! polls the bus taps and publishes everything for the editor.

use std::mem;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use audbus::{DEFAULT_NAMESPACE, Directory};
use rtrb::Consumer;

use crate::analysis::bus::{BusTap, SourceReading};
use crate::analysis::engine::{Engine, Wants};
use crate::analysis::reading::Published;
use crate::capture::StreamState;
use crate::layout::Module;
use crate::settings::AnalyzerSettings;
use crate::sources::{Shown, SourceList};

/// Pause between drains of the capture ring.
const POLL_INTERVAL: Duration = Duration::from_millis(4);
/// How often the list of live buses is refreshed for the SOURCES panel.
const BUS_LIST_INTERVAL: Duration = Duration::from_millis(500);
/// How long the FFT analyses keep running after the editor's last frame.
const VIEW_TIMEOUT: Duration = Duration::from_millis(750);

/// State shared by the worker, the editor and the plugin.
pub struct AnalysisShared {
    pub stream: Arc<StreamState>,
    pub settings: Arc<RwLock<AnalyzerSettings>>,
    /// The buses analyzed next to the input and what each view shows.
    pub sources: Arc<RwLock<SourceList>>,
    /// The bus directory namespace the sources are found in.
    namespace: String,
    published: Mutex<Published>,
    view_frames: AtomicU64,
    reset_loudness: AtomicBool,
    reset_peaks: AtomicBool,
    stop: AtomicBool,
}

impl AnalysisShared {
    /// Called by the editor once per frame; keeps the FFT analyses running.
    pub fn mark_viewed(&self) {
        self.view_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// Restarts integrated loudness, loudness range and the peak maxima.
    pub fn reset_loudness(&self) {
        self.reset_loudness.store(true, Ordering::Relaxed);
    }

    /// Clears the spectrum's held peaks and infinite average.
    pub fn reset_peaks(&self) {
        self.reset_peaks.store(true, Ordering::Relaxed);
    }

    /// Runs `read` on the latest results.
    pub fn with_published<R>(&self, read: impl FnOnce(&mut Published) -> R) -> R {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        read(&mut published)
    }

    pub fn settings(&self) -> AnalyzerSettings {
        *self.settings.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn sources(&self) -> SourceList {
        self.sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_sources(&self, sources: SourceList) {
        *self.sources.write().unwrap_or_else(PoisonError::into_inner) = sources;
    }

    /// Changes the settings with `edit`.
    pub fn edit_settings(&self, edit: impl FnOnce(&mut AnalyzerSettings)) {
        edit(
            &mut self
                .settings
                .write()
                .unwrap_or_else(PoisonError::into_inner),
        );
    }
}

/// The running worker; dropping it stops the thread.
pub struct Analyzer {
    shared: Arc<AnalysisShared>,
    worker: Option<JoinHandle<()>>,
}

impl Analyzer {
    /// Starts the worker reading `consumer` and the buses listed in `sources`.
    pub fn start(
        consumer: Consumer<[f32; 2]>,
        stream: Arc<StreamState>,
        settings: Arc<RwLock<AnalyzerSettings>>,
        sources: Arc<RwLock<SourceList>>,
    ) -> Self {
        Self::start_in(DEFAULT_NAMESPACE, consumer, stream, settings, sources)
    }

    /// Like [`start`](Self::start), for buses in a bus directory `namespace`.
    pub fn start_in(
        namespace: &str,
        consumer: Consumer<[f32; 2]>,
        stream: Arc<StreamState>,
        settings: Arc<RwLock<AnalyzerSettings>>,
        sources: Arc<RwLock<SourceList>>,
    ) -> Self {
        let shared = Arc::new(AnalysisShared {
            stream,
            settings,
            sources,
            namespace: namespace.to_owned(),
            published: Mutex::new(Published::default()),
            view_frames: AtomicU64::new(0),
            reset_loudness: AtomicBool::new(false),
            reset_peaks: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name(String::from("o-aud-a analysis"))
            .spawn(move || run(&worker_shared, consumer))
            .ok();
        Self { shared, worker }
    }

    pub fn shared(&self) -> &Arc<AnalysisShared> {
        &self.shared
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Resets the user asked for since the last round.
#[derive(Clone, Copy, Debug, Default)]
struct Resets {
    loudness: bool,
    peaks: bool,
}

fn run(shared: &AnalysisShared, mut consumer: Consumer<[f32; 2]>) {
    let mut engine: Option<Engine> = None;
    let mut buses = Buses::default();
    let mut seen_frames = 0;
    let mut last_view = Instant::now() - VIEW_TIMEOUT;
    while !shared.stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);
        let settings = shared.settings();
        let resets = Resets {
            loudness: shared.reset_loudness.swap(false, Ordering::Relaxed),
            peaks: shared.reset_peaks.swap(false, Ordering::Relaxed),
        };
        let frames = shared.view_frames.load(Ordering::Relaxed);
        if frames != seen_frames {
            seen_frames = frames;
            last_view = Instant::now();
        }
        let view = last_view.elapsed() < VIEW_TIMEOUT;
        // Buses are measured even before the host activates the plugin.
        buses.poll(shared, &settings, view, resets);

        let format = (shared.stream.sample_rate(), shared.stream.channels().max(1));
        if format.0 == 0 {
            // Not active yet: discard anything queued.
            let available = consumer.slots();
            if let Ok(chunk) = consumer.read_chunk(available) {
                chunk.commit_all();
            }
            shared.with_published(|published| {
                buses.publish(published);
                published.sequence += 1;
            });
            continue;
        }
        let engine = match &mut engine {
            Some(current) if current.format() == format => current,
            slot => slot.insert(Engine::new(format.0, format.1, &settings)),
        };
        engine.configure(&settings);
        engine.set_wants(buses.input_wants);
        if resets.loudness {
            engine.reset_loudness();
        }
        if resets.peaks {
            engine.reset_peaks();
        }

        let available = consumer.slots();
        if available > 0
            && let Ok(chunk) = consumer.read_chunk(available)
        {
            let (first, second) = chunk.as_slices();
            for frame in first.iter().chain(second) {
                engine.process(*frame, view);
            }
            chunk.commit_all();
        }
        let dropped = shared.stream.dropped_frames.load(Ordering::Relaxed);
        let mut published = shared
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        engine.publish(&mut published.input, false);
        published.dropped_frames = dropped;
        buses.publish(&mut published);
        published.sequence += 1;
    }
}

/// The bus sources: the directory and one tap per listed bus.
struct Buses {
    directory: Option<Arc<Directory>>,
    taps: Vec<BusTap>,
    /// Whether the list changed since the last publication.
    relisted: bool,
    /// Names of the buses with a transmitter.
    live: Vec<String>,
    last_list: Option<Instant>,
    /// The analyses the views that show the input need.
    input_wants: Wants,
}

impl Default for Buses {
    fn default() -> Self {
        Self {
            directory: None,
            taps: Vec::new(),
            relisted: false,
            live: Vec::new(),
            last_list: None,
            input_wants: Wants::ALL,
        }
    }
}

impl Buses {
    fn poll(
        &mut self,
        shared: &AnalysisShared,
        settings: &AnalyzerSettings,
        view: bool,
        resets: Resets,
    ) {
        let list = shared.sources();
        self.relist(&list);
        let views: Vec<(Module, Shown)> = Module::ALL
            .into_iter()
            .map(|module| (module, list.shown(module)))
            .collect();
        let shown = |module: Module| {
            views
                .iter()
                .find(|(listed, _)| *listed == module)
                .map(|(_, shown)| shown)
        };
        self.input_wants = Wants {
            // The input keeps its envelope, so ROLL has a history when it is opened.
            envelope: true,
            ..Wants::for_views(|module| shown(module).is_some_and(|shown| shown.input))
        };
        if self.directory.is_none() {
            // Retried every round until the shared directory can be opened.
            self.directory = Directory::open_namespace(&shared.namespace).ok();
        }
        let Some(directory) = &self.directory else {
            return;
        };
        if self
            .last_list
            .is_none_or(|time| time.elapsed() >= BUS_LIST_INTERVAL)
        {
            self.live = directory.buses().into_iter().map(|bus| bus.name).collect();
            self.last_list = Some(Instant::now());
        }
        for (index, tap) in self.taps.iter_mut().enumerate() {
            if resets.loudness {
                tap.reset_loudness();
            }
            if resets.peaks {
                tap.reset_peaks();
            }
            let wants =
                Wants::for_views(|module| shown(module).is_some_and(|shown| shown.has_bus(index)));
            tap.poll(directory, (settings, wants), view);
        }
    }

    /// Follows the list; taps of buses still listed are kept, so their measurements
    /// continue.
    fn relist(&mut self, list: &SourceList) {
        let current = self
            .taps
            .iter()
            .map(BusTap::listed)
            .eq(list.buses.iter().map(|bus| bus.name.as_str()));
        if current {
            return;
        }
        let mut previous = mem::take(&mut self.taps);
        self.taps = list
            .buses
            .iter()
            .map(|bus| {
                previous
                    .iter()
                    .position(|tap| tap.listed() == bus.name)
                    .map_or_else(|| BusTap::new(&bus.name), |index| previous.remove(index))
            })
            .collect();
        self.relisted = true;
    }

    fn publish(&mut self, published: &mut Published) {
        let whole = self.relisted || published.sources.len() != self.taps.len();
        published
            .sources
            .resize_with(self.taps.len(), SourceReading::default);
        for (tap, reading) in self.taps.iter_mut().zip(&mut published.sources) {
            tap.publish(reading, whole);
        }
        self.relisted = false;
        if published.live_buses != self.live {
            published.live_buses.clone_from(&self.live);
        }
    }
}
