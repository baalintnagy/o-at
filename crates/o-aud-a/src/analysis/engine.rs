//! The measurements of one signal, the input or a bus: an [`Engine`] runs the analyses
//! its views need ([`Wants`]) and publishes them as a [`SignalReading`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::analysis::fft::History;
use crate::analysis::loudness::LoudnessMeter;
use crate::analysis::reading::{
    ENVELOPE_BUCKET_S, ENVELOPE_LEN, EnvelopeBucket, LOUDNESS_HISTORY_LEN, LoudnessPoint,
    SignalReading,
};
use crate::analysis::spectrum::{
    SONOGRAM_QUEUE, SPECTRUM_RATE_HZ, SURFACE_RATE_HZ, SonogramFeed, SonogramStage, SpectrumStage,
    SurfaceFrame, SurfaceStage,
};
use crate::analysis::stereo::StereoMeter;
use crate::layout::Module;
use crate::settings::{AnalyzerSettings, Choice, FftSize, ScopeSamples, Signal, TimeSpan};

const EMPTY_BUCKET: EnvelopeBucket = [[f32::MAX, f32::MIN]; 4];

/// Envelope generations, unique across engines, so the editor restarts a signal's
/// envelope whenever its engine is replaced.
static ENVELOPE_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_envelope_generation() -> u64 {
    ENVELOPE_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Frames the scopes need for an oscilloscope `span` at `sample_rate` Hz: the span
/// twice over, so a trigger can be found before the newest span, and at least the
/// stereo scope's samples.
pub fn scope_frames(span: TimeSpan, sample_rate: f32) -> usize {
    ((2.0 * span.seconds() * sample_rate).ceil() as usize).clamp(ScopeSamples::MAX, FftSize::MAX)
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

/// The analyses a signal runs, for the views that show it. Loudness and the stereo
/// meters always run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wants {
    /// The averaged spectrum, also for the stereo view's bands.
    pub spectrum: bool,
    pub sonogram: bool,
    pub spectral: bool,
    pub surface: bool,
    /// The latest frames for the oscilloscope and the stereo scope.
    pub scope: bool,
    /// The oscilloscope's millisecond envelope.
    pub envelope: bool,
}

impl Wants {
    pub const ALL: Self = Self {
        spectrum: true,
        sonogram: true,
        spectral: true,
        surface: true,
        scope: true,
        envelope: true,
    };

    /// The analyses for the views `shows` the signal in.
    pub fn for_views(shows: impl Fn(Module) -> bool) -> Self {
        Self {
            spectrum: shows(Module::Spectrum) || shows(Module::Stereo),
            sonogram: shows(Module::Sonogram),
            spectral: shows(Module::Spectral),
            surface: shows(Module::Surface),
            scope: shows(Module::Oscilloscope) || shows(Module::Stereo),
            envelope: shows(Module::Oscilloscope),
        }
    }
}

/// Results that accumulate between publications.
#[derive(Default)]
struct Pending {
    sonogram: SonogramFeed,
    spectral: SonogramFeed,
    surface: SurfaceFrame,
    loudness_history: VecDeque<LoudnessPoint>,
    envelope: VecDeque<EnvelopeBucket>,
}

/// All measurements of one stream format, of the input or of a bus.
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
    /// Loudness blocks completed since the last publication.
    loudness_blocks: u32,
    pending: Pending,
    spectrum_dirty: bool,
    scope_dirty: bool,
    surface_dirty: bool,
    /// Frames published for the scopes.
    scope_frames: usize,
    bucket: EnvelopeBucket,
    bucket_frames: usize,
    frames_per_bucket: usize,
    envelope_generation: u64,
    wants: Wants,
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
            pending: Pending::default(),
            spectrum_dirty: true,
            scope_dirty: true,
            surface_dirty: true,
            scope_frames: ScopeSamples::MAX,
            bucket: EMPTY_BUCKET,
            bucket_frames: 0,
            frames_per_bucket: ((sample_rate as f32 * ENVELOPE_BUCKET_S).round() as usize).max(1),
            envelope_generation: next_envelope_generation(),
            wants: Wants::ALL,
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

    /// Sets the analyses to run. An image that a view shows again starts afresh.
    pub fn set_wants(&mut self, wants: Wants) {
        let before = self.wants;
        if wants.sonogram && !before.sonogram {
            self.sonogram.restart();
            self.pending.sonogram.columns.clear();
        }
        if wants.spectral && !before.spectral {
            self.spectral.restart();
            self.pending.spectral.columns.clear();
        }
        if wants.surface && !before.surface {
            self.surface.restart();
            self.pending.surface = SurfaceFrame::default();
            self.surface_dirty = true;
        }
        if !wants.envelope && before.envelope {
            self.pending.envelope.clear();
            self.bucket = EMPTY_BUCKET;
            self.bucket_frames = 0;
            self.envelope_generation = next_envelope_generation();
        }
        self.wants = wants;
    }

    pub fn reset_loudness(&mut self) {
        self.loudness.reset();
        self.pending.loudness_history.clear();
    }

    pub fn reset_peaks(&mut self) {
        self.spectrum.reset_peaks();
        self.spectrum_dirty = true;
    }

    /// Analyzes one frame; the FFT analyses only run when `view` is set.
    pub fn process(&mut self, frame: [f32; 2], view: bool) {
        self.history.push(frame);
        if self.wants.envelope {
            self.record_envelope(frame);
        }
        self.stereo.process(frame);
        if self.loudness.process(frame) {
            self.loudness_blocks += 1;
            let reading = self.loudness.reading();
            let history = &mut self.pending.loudness_history;
            if history.len() >= LOUDNESS_HISTORY_LEN {
                history.pop_front();
            }
            history.push_back([reading.momentary_lufs, reading.short_term_lufs]);
        }
        if !view {
            return;
        }
        let wants = self.wants;
        if (wants.spectrum || wants.scope) && self.spectrum_ticker.step() {
            if wants.spectrum {
                self.spectrum.tick(&self.history);
                self.spectrum_dirty = true;
            }
            // The scopes refresh with the spectrum, 60 times per second.
            self.scope_dirty |= wants.scope;
        }
        if wants.sonogram {
            self.sonogram.observe(frame);
            if self.sonogram_ticker.step() {
                self.sonogram
                    .tick(&self.history, &mut self.pending.sonogram);
            }
        }
        if wants.spectral {
            self.spectral.observe(frame);
            if self.spectral_ticker.step() {
                self.spectral
                    .tick(&self.history, &mut self.pending.spectral);
            }
        }
        if wants.surface && self.surface_ticker.step() {
            self.surface.tick(&self.history, &mut self.pending.surface);
            self.surface_dirty = true;
        }
    }

    fn record_envelope(&mut self, frame: [f32; 2]) {
        for (signal, extent) in Signal::ALL.iter().zip(&mut self.bucket) {
            let value = signal.of(frame);
            extent[0] = extent[0].min(value);
            extent[1] = extent[1].max(value);
        }
        self.bucket_frames += 1;
        if self.bucket_frames >= self.frames_per_bucket {
            let envelope = &mut self.pending.envelope;
            if envelope.len() >= ENVELOPE_LEN {
                envelope.pop_front();
            }
            envelope.push_back(self.bucket);
            self.bucket = EMPTY_BUCKET;
            self.bucket_frames = 0;
        }
    }

    /// Writes the results into `reading` and marks what it rewrote as fresh, handing over
    /// new sonogram columns and envelope buckets. `whole` rewrites everything, for a
    /// reading that held another signal.
    pub fn publish(&mut self, reading: &mut SignalReading, whole: bool) {
        reading.sample_rate = self.sample_rate;
        reading.channels = self.channels;
        if self.spectrum_dirty || whole {
            self.spectrum.write(&mut reading.spectrum);
            reading.fresh.spectrum = true;
            self.spectrum_dirty = false;
        }
        if self.scope_dirty || whole {
            reading.scope.clear();
            reading.scope.extend(self.history.latest(self.scope_frames));
            reading.fresh.scope = true;
            self.scope_dirty = false;
        }
        let wants = self.wants;
        for (wanted, feed, target) in [
            (
                wants.sonogram,
                &mut self.pending.sonogram,
                &mut reading.sonogram,
            ),
            (
                wants.spectral,
                &mut self.pending.spectral,
                &mut reading.spectral,
            ),
        ] {
            if wanted {
                hand_over(feed, target);
            } else if whole {
                *target = SonogramFeed::default();
            }
        }
        if wants.surface && (self.surface_dirty || whole) {
            reading.surface.clone_from(&self.pending.surface);
            reading.fresh.surface = true;
            self.surface_dirty = false;
        } else if !wants.surface && whole {
            reading.surface = SurfaceFrame::default();
            reading.fresh.surface = true;
        }

        reading.stereo = self.stereo.reading();
        reading.loudness = *self.loudness.reading();
        let history = &self.pending.loudness_history;
        if self.loudness_blocks > 0 || whole || reading.loudness_history.len() != history.len() {
            reading.loudness_history.clone_from(history);
            reading.fresh.loudness_history = true;
            self.loudness_blocks = 0;
        }
        if reading.envelope_generation != self.envelope_generation {
            reading.envelope.clear();
            reading.envelope_generation = self.envelope_generation;
        }
        reading.envelope.extend(self.pending.envelope.drain(..));
        let excess = reading.envelope.len().saturating_sub(ENVELOPE_LEN);
        reading.envelope.drain(..excess);
    }
}

/// Moves the new columns of `feed` to `published`, keeping at most [`SONOGRAM_QUEUE`]
/// that the editor has not taken yet.
fn hand_over(feed: &mut SonogramFeed, published: &mut SonogramFeed) {
    if published.generation != feed.generation {
        published.columns.clear();
    }
    published.generation = feed.generation;
    published.range_hz = feed.range_hz;
    published.scale = feed.scale;
    published.lanes = feed.lanes;
    published.columns.extend(feed.columns.drain(..));
    let excess = published.columns.len().saturating_sub(SONOGRAM_QUEUE);
    published.columns.drain(..excess);
}
