//! The analysis worker: drains the capture ring and runs every measurement off the audio
//! thread.
//!
//! Loudness and the stereo meters run all the time, so integrated loudness keeps counting
//! while the editor is closed. The FFT analyses only run while an editor draws (see
//! [`AnalysisShared::mark_viewed`]). Results go to the editor through a mutex that only
//! the worker and the editor take; the audio thread never touches it.

pub mod fft;
pub mod loudness;
pub mod spectrum;
pub mod stereo;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rtrb::Consumer;

use crate::analysis::fft::History;
use crate::analysis::loudness::{LoudnessMeter, LoudnessReading};
use crate::analysis::spectrum::{
    SPECTRUM_RATE_HZ, SURFACE_RATE_HZ, SonogramFeed, SonogramStage, SpectrumFrame, SpectrumStage,
    SurfaceFrame, SurfaceStage,
};
use crate::analysis::stereo::{StereoMeter, StereoReading};
use crate::capture::StreamState;
use crate::settings::{
    AnalyzerSettings, Choice, FftSize, HistorySpan, ScopeSamples, Signal, TimeSpan,
};

/// Pause between drains of the capture ring.
const POLL_INTERVAL: Duration = Duration::from_millis(4);
/// How long the FFT analyses keep running after the editor's last frame.
const VIEW_TIMEOUT: Duration = Duration::from_millis(750);
/// Loudness history entries per second.
pub const LOUDNESS_HISTORY_RATE_HZ: f32 = 10.0;
/// Loudness history length in entries: the longest span.
pub const LOUDNESS_HISTORY_LEN: usize = (600.0 * LOUDNESS_HISTORY_RATE_HZ) as usize;

/// One loudness history entry: momentary and short-term loudness in LUFS.
pub type LoudnessPoint = [f32; 2];

/// Duration of an envelope bucket in seconds.
pub const ENVELOPE_BUCKET_S: f32 = 0.001;
/// Envelope buckets kept, a little over a minute.
pub const ENVELOPE_LEN: usize = 65_536;

/// The minimum and maximum of each [`Signal`] over one envelope bucket, indexed by
/// [`Choice::index`].
pub type EnvelopeBucket = [[f32; 2]; 4];

const EMPTY_BUCKET: EnvelopeBucket = [[f32::MAX, f32::MIN]; 4];

/// Everything the editor draws.
#[derive(Clone, Debug, Default)]
pub struct Published {
    /// Analyzed stream format; zero before the host activates the plugin.
    pub sample_rate: u32,
    pub channels: u32,
    pub spectrum: SpectrumFrame,
    pub sonogram: SonogramFeed,
    /// The spectral view's sonogram layer, analyzed with its own settings.
    pub spectral: SonogramFeed,
    pub surface: SurfaceFrame,
    /// The latest frames for the stereo scope and the oscilloscope, oldest first: at
    /// least [`ScopeSamples::MAX`], more for long oscilloscope spans.
    pub scope: Vec<[f32; 2]>,
    pub stereo: StereoReading,
    pub loudness: LoudnessReading,
    /// Loudness every 100 ms, oldest first.
    pub loudness_history: VecDeque<LoudnessPoint>,
    /// New oscilloscope envelope buckets, oldest first; the editor takes them.
    pub envelope: VecDeque<EnvelopeBucket>,
    /// Changes when earlier envelope buckets no longer fit, after a format change.
    pub envelope_generation: u64,
    /// Frames the worker could not take in time.
    pub dropped_frames: u64,
    /// Changes with every update.
    pub sequence: u64,
}

/// State shared by the worker, the editor and the plugin.
pub struct AnalysisShared {
    pub stream: Arc<StreamState>,
    pub settings: Arc<RwLock<AnalyzerSettings>>,
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

/// Counts samples to the next periodic analysis.
#[derive(Clone, Copy, Debug)]
struct Ticker {
    interval: f64,
    remaining: f64,
}

impl Ticker {
    fn new(sample_rate: f32, rate_hz: f32) -> Self {
        let interval = f64::from(sample_rate / rate_hz).max(1.0);
        Self {
            interval,
            remaining: interval,
        }
    }

    fn set_rate(&mut self, sample_rate: f32, rate_hz: f32) {
        let interval = f64::from(sample_rate / rate_hz).max(1.0);
        if interval != self.interval {
            self.interval = interval;
            self.remaining = self.remaining.min(interval);
        }
    }

    /// Counts one frame; `true` when the analysis is due.
    fn step(&mut self) -> bool {
        self.remaining -= 1.0;
        if self.remaining > 0.0 {
            return false;
        }
        self.remaining += self.interval;
        true
    }
}

/// All measurements of one stream format.
pub struct Engine {
    sample_rate: u32,
    channels: u32,
    history: History,
    loudness: LoudnessMeter,
    stereo: StereoMeter,
    spectrum: SpectrumStage,
    sonogram: SonogramStage,
    spectral: SonogramStage,
    surface: SurfaceStage,
    spectrum_ticker: Ticker,
    sonogram_ticker: Ticker,
    spectral_ticker: Ticker,
    surface_ticker: Ticker,
    loudness_blocks: u32,
    out: Published,
    spectrum_dirty: bool,
    /// Frames published for the scopes.
    scope_frames: usize,
    bucket: EnvelopeBucket,
    bucket_frames: usize,
    frames_per_bucket: usize,
    surface_dirty: bool,
}

impl Engine {
    pub fn new(sample_rate: u32, channels: u32, settings: &AnalyzerSettings) -> Self {
        let rate = sample_rate as f32;
        let sonogram = SonogramStage::new(settings.sonogram, rate);
        let spectral = SonogramStage::new(settings.spectral_sonogram(), rate);
        Self {
            sample_rate,
            channels,
            history: History::default(),
            loudness: LoudnessMeter::new(f64::from(sample_rate), channels as usize),
            stereo: StereoMeter::new(rate),
            spectrum: SpectrumStage::new(settings.spectrum, rate),
            sonogram_ticker: Ticker::new(rate, sonogram.rate_hz()),
            sonogram,
            spectral_ticker: Ticker::new(rate, spectral.rate_hz()),
            spectral,
            surface: SurfaceStage::new(settings.surface, settings.sonogram, rate),
            spectrum_ticker: Ticker::new(rate, SPECTRUM_RATE_HZ),
            surface_ticker: Ticker::new(rate, SURFACE_RATE_HZ),
            loudness_blocks: 0,
            out: Published {
                sample_rate,
                channels,
                ..Published::default()
            },
            spectrum_dirty: true,
            surface_dirty: true,
            scope_frames: ScopeSamples::MAX,
            bucket: EMPTY_BUCKET,
            bucket_frames: 0,
            frames_per_bucket: ((sample_rate as f32 * ENVELOPE_BUCKET_S).round() as usize).max(1),
        }
    }

    pub fn format(&self) -> (u32, u32) {
        (self.sample_rate, self.channels)
    }

    pub fn configure(&mut self, settings: &AnalyzerSettings) {
        let rate = self.sample_rate as f32;
        self.spectrum.configure(settings.spectrum);
        self.sonogram.configure(settings.sonogram);
        self.sonogram_ticker.set_rate(rate, self.sonogram.rate_hz());
        self.spectral.configure(settings.spectral_sonogram());
        self.spectral_ticker.set_rate(rate, self.spectral.rate_hz());
        self.surface.configure(settings.surface, settings.sonogram);
        self.scope_frames = scope_frames(settings.oscilloscope.span, rate);
    }

    pub fn reset_loudness(&mut self) {
        self.loudness.reset();
        self.out.loudness_history.clear();
        self.out.loudness = *self.loudness.reading();
    }

    pub fn reset_peaks(&mut self) {
        self.spectrum.reset_peaks();
        self.spectrum_dirty = true;
    }

    /// Analyzes one frame; the FFT analyses only run when `view` is set.
    pub fn process(&mut self, frame: [f32; 2], view: bool) {
        self.history.push(frame);
        for (signal, extent) in Signal::ALL.iter().zip(&mut self.bucket) {
            let value = signal.of(frame);
            extent[0] = extent[0].min(value);
            extent[1] = extent[1].max(value);
        }
        self.bucket_frames += 1;
        if self.bucket_frames >= self.frames_per_bucket {
            if self.out.envelope.len() >= ENVELOPE_LEN {
                self.out.envelope.pop_front();
            }
            self.out.envelope.push_back(self.bucket);
            self.bucket = EMPTY_BUCKET;
            self.bucket_frames = 0;
        }
        self.stereo.process(frame);
        if self.loudness.process(frame) {
            self.loudness_blocks += 1;
            let reading = self.loudness.reading();
            if self.out.loudness_history.len() >= LOUDNESS_HISTORY_LEN {
                self.out.loudness_history.pop_front();
            }
            self.out
                .loudness_history
                .push_back([reading.momentary_lufs, reading.short_term_lufs]);
        }
        if !view {
            return;
        }
        if self.spectrum_ticker.step() {
            self.spectrum.tick(&self.history);
            self.spectrum_dirty = true;
        }
        self.sonogram.observe(frame);
        if self.sonogram_ticker.step() {
            self.sonogram.tick(&self.history, &mut self.out.sonogram);
        }
        self.spectral.observe(frame);
        if self.spectral_ticker.step() {
            self.spectral.tick(&self.history, &mut self.out.spectral);
        }
        if self.surface_ticker.step() {
            self.surface.tick(&self.history, &mut self.out.surface);
            self.surface_dirty = true;
        }
    }

    /// Copies the results into `published`, handing over new sonogram columns.
    pub fn publish(&mut self, published: &mut Published, dropped_frames: u64) {
        published.sample_rate = self.sample_rate;
        published.channels = self.channels;
        if self.spectrum_dirty {
            self.spectrum.write(&mut self.out.spectrum);
            published.spectrum.clone_from(&self.out.spectrum);
            // The scopes refresh with the spectrum, 60 times per second.
            published.scope.clear();
            published
                .scope
                .extend(self.history.latest(self.scope_frames));
            self.spectrum_dirty = false;
        }
        hand_over(&mut self.out.sonogram, &mut published.sonogram);
        hand_over(&mut self.out.spectral, &mut published.spectral);
        if self.surface_dirty {
            published.surface.clone_from(&self.out.surface);
            self.surface_dirty = false;
        }

        published.stereo = self.stereo.reading();
        published.loudness = *self.loudness.reading();
        if self.loudness_blocks > 0
            || published.loudness_history.len() != self.out.loudness_history.len()
        {
            published
                .loudness_history
                .clone_from(&self.out.loudness_history);
            self.loudness_blocks = 0;
        }
        published.envelope.extend(self.out.envelope.drain(..));
        let excess = published.envelope.len().saturating_sub(ENVELOPE_LEN);
        published.envelope.drain(..excess);
        published.dropped_frames = dropped_frames;
        published.sequence += 1;
    }
}

/// Moves the new columns of `feed` to `published`, keeping at most
/// [`spectrum::SONOGRAM_QUEUE`] that the editor has not taken yet.
fn hand_over(feed: &mut SonogramFeed, published: &mut SonogramFeed) {
    if published.generation != feed.generation {
        published.columns.clear();
    }
    published.generation = feed.generation;
    published.range_hz = feed.range_hz;
    published.scale = feed.scale;
    published.lanes = feed.lanes;
    published.columns.extend(feed.columns.drain(..));
    let excess = published
        .columns
        .len()
        .saturating_sub(spectrum::SONOGRAM_QUEUE);
    published.columns.drain(..excess);
}

/// The running worker; dropping it stops the thread.
pub struct Analyzer {
    shared: Arc<AnalysisShared>,
    worker: Option<JoinHandle<()>>,
}

impl Analyzer {
    /// Starts the worker reading `consumer`.
    pub fn start(
        consumer: Consumer<[f32; 2]>,
        stream: Arc<StreamState>,
        settings: Arc<RwLock<AnalyzerSettings>>,
    ) -> Self {
        let shared = Arc::new(AnalysisShared {
            stream,
            settings,
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

fn run(shared: &AnalysisShared, mut consumer: Consumer<[f32; 2]>) {
    let mut engine: Option<Engine> = None;
    let mut seen_frames = 0;
    let mut last_view = Instant::now() - VIEW_TIMEOUT;
    while !shared.stop.load(Ordering::Relaxed) {
        thread::sleep(POLL_INTERVAL);
        let format = (shared.stream.sample_rate(), shared.stream.channels().max(1));
        if format.0 == 0 {
            // Not active yet: discard anything queued.
            let available = consumer.slots();
            if let Ok(chunk) = consumer.read_chunk(available) {
                chunk.commit_all();
            }
            continue;
        }
        let settings = shared.settings();
        let engine = match &mut engine {
            Some(current) if current.format() == format => current,
            slot => {
                shared.with_published(|published| {
                    published.envelope.clear();
                    published.envelope_generation += 1;
                });
                slot.insert(Engine::new(format.0, format.1, &settings))
            }
        };
        engine.configure(&settings);
        if shared.reset_loudness.swap(false, Ordering::Relaxed) {
            engine.reset_loudness();
        }
        if shared.reset_peaks.swap(false, Ordering::Relaxed) {
            engine.reset_peaks();
        }
        let frames = shared.view_frames.load(Ordering::Relaxed);
        if frames != seen_frames {
            seen_frames = frames;
            last_view = Instant::now();
        }
        let view = last_view.elapsed() < VIEW_TIMEOUT;

        let available = consumer.slots();
        if available > 0 {
            if let Ok(chunk) = consumer.read_chunk(available) {
                let (first, second) = chunk.as_slices();
                for frame in first.iter().chain(second) {
                    engine.process(*frame, view);
                }
                chunk.commit_all();
            }
        }
        let dropped = shared.stream.dropped_frames.load(Ordering::Relaxed);
        let mut published = shared
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        engine.publish(&mut published, dropped);
    }
}

/// Frames the scopes need for an oscilloscope `span` at `sample_rate` Hz: the span
/// twice over, so a trigger can be found before the newest span, and at least the
/// stereo scope's samples.
pub fn scope_frames(span: TimeSpan, sample_rate: f32) -> usize {
    ((2.0 * span.seconds() * sample_rate).ceil() as usize).clamp(ScopeSamples::MAX, FftSize::MAX)
}

/// Longest loudness history span in entries for `span`.
pub fn history_entries(span: HistorySpan) -> usize {
    (span.seconds() * LOUDNESS_HISTORY_RATE_HZ) as usize
}
