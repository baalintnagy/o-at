//! FFT-based analyses: the averaged spectrum, sonogram columns and the 3D histories.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::analysis::fft::{BandBins, BandMap, FLOOR_DB, FftTap, History, power_to_db};
use crate::analysis::stereo::{BandStereo, BandStereoMeter, correlation, pan_position};
use crate::settings::{
    FreqLimit, FreqScale, SonogramSettings, SpectrumSettings, SurfaceSettings, Window,
};

/// Spectrum updates per second.
pub const SPECTRUM_RATE_HZ: f32 = 60.0;
/// 3D history rows per second.
pub const SURFACE_RATE_HZ: f32 = 30.0;
/// Rows of a sonogram column.
pub const SONOGRAM_ROWS: usize = 512;
/// Sonogram columns kept for the editor while it is not drawing.
pub const SONOGRAM_QUEUE: usize = 1_024;
/// Pan positions of the stereogram, from left to right.
pub const PAN_COLUMNS: usize = 49;
/// Decay time constant of the stereogram in seconds.
const STEREOGRAM_TIME_S: f32 = 0.35;

/// Sonogram generations, unique across stages, so a view switching between the input's
/// and a source's feed always starts again. Zero is never handed out: it marks an empty
/// feed.
static SONOGRAM_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_sonogram_generation() -> u64 {
    SONOGRAM_GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// The averaged spectrum, for the editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpectrumFrame {
    /// Width of a bin in Hz.
    pub bin_hz: f32,
    /// Levels per bin in dB of the primary and, if shown, the secondary curve.
    pub primary_db: Vec<f32>,
    pub secondary_db: Vec<f32>,
    /// Held peaks per bin in dB; empty without peak hold.
    pub primary_peak_db: Vec<f32>,
    pub secondary_peak_db: Vec<f32>,
    /// Stereo measurements per band.
    pub bands: BandStereo,
    /// Changes whenever the curves restart, for example after a setting changed.
    pub generation: u64,
}

/// Averaged spectrum of one or two signals.
pub struct SpectrumStage {
    settings: SpectrumSettings,
    sample_rate: f32,
    tap: FftTap,
    averaged: [Vec<f32>; 2],
    frames: u64,
    shown_db: [Vec<f32>; 2],
    peak_db: [Vec<f32>; 2],
    prefix: Vec<f64>,
    bands: BandStereoMeter,
    generation: u64,
}

impl SpectrumStage {
    pub fn new(settings: SpectrumSettings, sample_rate: f32) -> Self {
        let mut stage = Self {
            settings,
            sample_rate,
            tap: FftTap::new(settings.fft, settings.window),
            averaged: [Vec::new(), Vec::new()],
            frames: 0,
            shown_db: [Vec::new(), Vec::new()],
            peak_db: [Vec::new(), Vec::new()],
            prefix: Vec::new(),
            bands: BandStereoMeter::default(),
            generation: 0,
        };
        stage.restart();
        stage
    }

    fn bin_hz(&self) -> f32 {
        self.sample_rate / self.tap.samples() as f32
    }

    fn restart(&mut self) {
        let bins = self.tap.bins();
        for side in 0..2 {
            self.averaged[side] = vec![0.0; bins];
            self.shown_db[side] = vec![FLOOR_DB; bins];
            self.peak_db[side] = vec![FLOOR_DB; bins];
        }
        self.prefix = vec![0.0; bins + 1];
        self.frames = 0;
        self.bands.configure(bins, self.bin_hz());
        self.generation += 1;
    }

    /// Applies new settings; changes that alter the curves restart them.
    pub fn configure(&mut self, settings: SpectrumSettings) {
        if settings == self.settings {
            return;
        }
        let resized = self.tap.configure(settings.fft, settings.window);
        let restart = resized
            || settings.channels != self.settings.channels
            || settings.averaging != self.settings.averaging
            || settings.smoothing != self.settings.smoothing
            || settings.peak_hold != self.settings.peak_hold;
        self.settings = settings;
        if restart {
            self.restart();
        }
    }

    pub fn reset_peaks(&mut self) {
        self.peak_db
            .iter_mut()
            .for_each(|peaks| peaks.fill(FLOOR_DB));
        self.frames = 0;
        self.generation += 1;
    }

    /// Analyzes the latest frames.
    pub fn tick(&mut self, history: &History) {
        self.tap.analyze(history);
        let interval_s = 1.0 / SPECTRUM_RATE_HZ;
        self.bands.update(&self.tap, interval_s);
        self.frames += 1;
        let weight = match self.settings.averaging.time_constant_s() {
            None => 1.0,
            Some(time) if time.is_infinite() => 1.0 / self.frames as f32,
            Some(time) => 1.0 - (-interval_s / time).exp(),
        };
        let (primary, secondary) = self.settings.channels.signals();
        let signals = [Some(primary), secondary];
        for (side, signal) in signals.iter().enumerate() {
            let Some(signal) = *signal else {
                continue;
            };
            for (bin, average) in self.averaged[side].iter_mut().enumerate() {
                let power = self.tap.power(signal, bin);
                *average += weight * (power - *average);
            }
            smooth(
                &self.averaged[side],
                self.settings.smoothing.octaves(),
                &mut self.prefix,
                &mut self.shown_db[side],
            );
            if self.settings.peak_hold {
                for (peak, shown) in self.peak_db[side].iter_mut().zip(&self.shown_db[side]) {
                    *peak = peak.max(*shown);
                }
            }
        }
    }

    pub fn write(&self, frame: &mut SpectrumFrame) {
        frame.bin_hz = self.bin_hz();
        frame.generation = self.generation;
        let (_, secondary) = self.settings.channels.signals();
        frame.primary_db.clone_from(&self.shown_db[0]);
        if secondary.is_some() {
            frame.secondary_db.clone_from(&self.shown_db[1]);
        } else {
            frame.secondary_db.clear();
        }
        if self.settings.peak_hold {
            frame.primary_peak_db.clone_from(&self.peak_db[0]);
            if secondary.is_some() {
                frame.secondary_peak_db.clone_from(&self.peak_db[1]);
            } else {
                frame.secondary_peak_db.clear();
            }
        } else {
            frame.primary_peak_db.clear();
            frame.secondary_peak_db.clear();
        }
        self.bands.write(&mut frame.bands);
    }
}

/// Converts powers to dB, averaged over `octaves` around each bin.
fn smooth(power: &[f32], octaves: Option<f32>, prefix: &mut [f64], output: &mut [f32]) {
    let Some(octaves) = octaves else {
        for (shown, value) in output.iter_mut().zip(power) {
            *shown = power_to_db(*value);
        }
        return;
    };
    let mut sum = 0.0;
    prefix[0] = 0.0;
    for (index, value) in power.iter().enumerate() {
        sum += f64::from(*value);
        prefix[index + 1] = sum;
    }
    let ratio = 2.0_f32.powf(0.5 * octaves);
    let last = power.len() - 1;
    for (bin, shown) in output.iter_mut().enumerate() {
        let low = ((bin as f32 / ratio).floor() as usize).min(bin);
        let high = ((bin as f32 * ratio).ceil() as usize).clamp(bin, last);
        let mean = (prefix[high + 1] - prefix[low]) / (high + 1 - low) as f64;
        *shown = power_to_db(mean as f32);
    }
}

/// The waveform between two sonogram columns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveExtent {
    pub min: f32,
    pub max: f32,
    /// RMS level, linear.
    pub rms: f32,
}

impl Default for WaveExtent {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 0.0,
            rms: 0.0,
        }
    }
}

/// One sonogram column with the waveform it covers, for one or two lanes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SonogramColumn {
    /// [`SONOGRAM_ROWS`] levels in dB per lane, lowest frequency first, lane by lane.
    pub levels: Vec<f32>,
    pub wave: [WaveExtent; 2],
}

/// Waveform accumulated since the last column: minimum, maximum, sum of squares, frames.
type WaveSum = (f32, f32, f64, u32);

const EMPTY_WAVE: WaveSum = (f32::MAX, f32::MIN, 0.0, 0);

fn add_to(sum: &mut WaveSum, value: f32) {
    sum.0 = sum.0.min(value);
    sum.1 = sum.1.max(value);
    sum.2 += f64::from(value * value);
    sum.3 += 1;
}

fn extent((min, max, squares, count): WaveSum) -> WaveExtent {
    if count == 0 {
        return WaveExtent::default();
    }
    WaveExtent {
        min,
        max,
        rms: (squares / f64::from(count)).sqrt() as f32,
    }
}

/// New sonogram columns, for the editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SonogramFeed {
    /// New columns, oldest first.
    pub columns: VecDeque<SonogramColumn>,
    /// Shown frequency range in Hz.
    pub range_hz: (f32, f32),
    pub scale: FreqScale,
    /// Lanes per column.
    pub lanes: usize,
    /// Changes whenever the rows map to other frequencies or the lanes change.
    pub generation: u64,
}

/// Sonogram columns at the selected speed.
pub struct SonogramStage {
    settings: SonogramSettings,
    sample_rate: f32,
    tap: FftTap,
    map: BandMap,
    range_hz: (f32, f32),
    generation: u64,
    /// Waveform per lane since the last column.
    wave: [WaveSum; 2],
}

impl SonogramStage {
    pub fn new(settings: SonogramSettings, sample_rate: f32) -> Self {
        let mut stage = Self {
            settings,
            sample_rate,
            tap: FftTap::new(settings.fft, Window::Hann),
            map: BandMap::default(),
            range_hz: (0.0, 0.0),
            generation: 0,
            wave: [EMPTY_WAVE; 2],
        };
        stage.remap();
        stage
    }

    /// Records a frame for the next column's waveform extent.
    pub fn observe(&mut self, frame: [f32; 2]) {
        let (first, second) = self.settings.signals();
        add_to(&mut self.wave[0], first.of(frame));
        if let Some(second) = second {
            add_to(&mut self.wave[1], second.of(frame));
        }
    }

    /// Columns per second.
    pub fn rate_hz(&self) -> f32 {
        self.settings.speed.columns_per_s()
    }

    fn remap(&mut self) {
        let high = self.settings.limit.high_hz(self.sample_rate);
        let log = self.settings.scale != FreqScale::Linear;
        self.range_hz = (if log { FreqLimit::LOW_HZ } else { 0.0 }, high);
        self.map = BandMap::new(
            SONOGRAM_ROWS,
            self.range_hz,
            log,
            self.tap.bins(),
            self.sample_rate / self.tap.samples() as f32,
        );
        self.generation = next_sonogram_generation();
    }

    /// Starts again with a new generation, so the editor clears its image.
    pub fn restart(&mut self) {
        self.wave = [EMPTY_WAVE; 2];
        self.generation = next_sonogram_generation();
    }

    pub fn configure(&mut self, settings: SonogramSettings) {
        if settings == self.settings {
            return;
        }
        let resized = self.tap.configure(settings.fft, Window::Hann);
        let remap = resized
            || settings.scale != self.settings.scale
            || settings.limit != self.settings.limit
            || settings.channels != self.settings.channels;
        self.settings = settings;
        if remap {
            self.remap();
        }
    }

    /// Analyzes the latest frames into a new column of `feed`.
    pub fn tick(&mut self, history: &History, feed: &mut SonogramFeed) {
        self.tap.analyze(history);
        let mut column = if feed.columns.len() >= SONOGRAM_QUEUE {
            feed.columns.pop_front().unwrap_or_default()
        } else {
            SonogramColumn::default()
        };
        let lanes = self.settings.lanes();
        column.levels.resize(SONOGRAM_ROWS * lanes, FLOOR_DB);
        let (first, second) = self.settings.signals();
        for (lane, signal) in [Some(first), second].into_iter().flatten().enumerate() {
            let levels = &mut column.levels[lane * SONOGRAM_ROWS..(lane + 1) * SONOGRAM_ROWS];
            self.map
                .levels_db(|bin| self.tap.power(signal, bin), levels);
        }
        column.wave = self.wave.map(extent);
        self.wave = [EMPTY_WAVE; 2];
        feed.columns.push_back(column);
        feed.lanes = lanes;
        feed.range_hz = self.range_hz;
        feed.scale = self.settings.scale;
        if feed.generation != self.generation {
            feed.generation = self.generation;
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// The 3D histories, for the editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceFrame {
    pub bands: usize,
    /// Lanes per row.
    pub lanes: usize,
    /// Spectrum rows of `bands` levels in dB per lane, lane by lane, oldest first.
    pub rows: VecDeque<Vec<f32>>,
    /// Stereogram levels in dB, `bands` rows of [`PAN_COLUMNS`] pan positions.
    pub stereo_db: Vec<f32>,
    /// Correlation per stereogram cell.
    pub stereo_correlation: Vec<f32>,
    /// Frequency range of the bands in Hz.
    pub range_hz: (f32, f32),
}

/// Spectrum history and stereo field at a fixed rate.
pub struct SurfaceStage {
    settings: SurfaceSettings,
    fft: SonogramSettings,
    sample_rate: f32,
    tap: FftTap,
    map: BandMap,
    range_hz: (f32, f32),
    stereo_power: Vec<f32>,
    stereo_cross: Vec<f32>,
}

impl SurfaceStage {
    pub fn new(settings: SurfaceSettings, fft: SonogramSettings, sample_rate: f32) -> Self {
        let mut stage = Self {
            settings,
            fft,
            sample_rate,
            tap: FftTap::new(fft.fft, Window::Hann),
            map: BandMap::default(),
            range_hz: (0.0, 0.0),
            stereo_power: Vec::new(),
            stereo_cross: Vec::new(),
        };
        stage.remap();
        stage
    }

    fn remap(&mut self) {
        let bands = self.settings.bands.count();
        self.range_hz = (
            FreqLimit::LOW_HZ,
            FreqLimit::Audible.high_hz(self.sample_rate),
        );
        self.map = BandMap::new(
            bands,
            self.range_hz,
            true,
            self.tap.bins(),
            self.sample_rate / self.tap.samples() as f32,
        );
        self.stereo_power = vec![0.0; bands * PAN_COLUMNS];
        self.stereo_cross = vec![0.0; bands * PAN_COLUMNS];
    }

    /// Clears the stereogram, for a new history.
    pub fn restart(&mut self) {
        self.stereo_power.fill(0.0);
        self.stereo_cross.fill(0.0);
    }

    pub fn configure(&mut self, settings: SurfaceSettings, fft: SonogramSettings) {
        let resized = self.tap.configure(fft.fft, Window::Hann);
        let remap = resized || settings.bands != self.settings.bands;
        self.settings = settings;
        self.fft = fft;
        if remap {
            self.remap();
        }
    }

    /// Analyzes the latest frames into a new row of `frame`.
    pub fn tick(&mut self, history: &History, frame: &mut SurfaceFrame) {
        self.tap.analyze(history);
        let bands = self.settings.bands.count();
        let lanes = self.fft.lanes();
        if frame.bands != bands || frame.range_hz != self.range_hz || frame.lanes != lanes {
            frame.rows.clear();
            frame.bands = bands;
            frame.lanes = lanes;
            frame.range_hz = self.range_hz;
        }
        let depth = self.settings.depth.count();
        let mut row = if frame.rows.len() >= depth {
            frame.rows.pop_front().unwrap_or_default()
        } else {
            Vec::new()
        };
        while frame.rows.len() >= depth {
            frame.rows.pop_front();
        }
        row.resize(bands * lanes, FLOOR_DB);
        let (first, second) = self.fft.signals();
        for (lane, signal) in [Some(first), second].into_iter().flatten().enumerate() {
            self.map.levels_db(
                |bin| self.tap.power(signal, bin),
                &mut row[lane * bands..(lane + 1) * bands],
            );
        }
        frame.rows.push_back(row);

        let decay = (-1.0 / (SURFACE_RATE_HZ * STEREOGRAM_TIME_S)).exp();
        self.stereo_power
            .iter_mut()
            .for_each(|value| *value *= decay);
        self.stereo_cross
            .iter_mut()
            .for_each(|value| *value *= decay);
        for (band, bins) in self.map.bands.iter().enumerate() {
            let (first, last) = match *bins {
                BandBins::Range(first, last) => (first, last),
                BandBins::Between(position) => {
                    let bin = position.round() as usize;
                    (bin, bin)
                }
            };
            for bin in first..=last {
                let (left, right, cross) = self.tap.stereo_power(bin);
                let pan = pan_position(left, right);
                let column = (((pan + 1.0) * 0.5 * (PAN_COLUMNS - 1) as f32).round() as usize)
                    .min(PAN_COLUMNS - 1);
                let cell = band * PAN_COLUMNS + column;
                self.stereo_power[cell] += (1.0 - decay) * 0.5 * (left + right);
                self.stereo_cross[cell] += (1.0 - decay) * cross;
            }
        }
        frame.stereo_db.clear();
        frame
            .stereo_db
            .extend(self.stereo_power.iter().map(|power| power_to_db(*power)));
        frame.stereo_correlation.clear();
        frame.stereo_correlation.extend(
            self.stereo_power
                .iter()
                .zip(&self.stereo_cross)
                .map(|(power, cross)| correlation(*power, *power, *cross)),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{SONOGRAM_ROWS, SonogramFeed, SonogramStage, SpectrumFrame, SpectrumStage, smooth};
    use crate::analysis::fft::History;
    use crate::settings::{
        Averaging, ChannelMode, Lanes, SonogramSettings, SpectrumSettings, Window,
    };

    fn sine_history(frequency: f32, rate: f32) -> History {
        let mut history = History::default();
        for n in 0..32_768 {
            let value = (TAU * frequency * n as f32 / rate).sin();
            history.push([value, 0.5 * value]);
        }
        history
    }

    #[test]
    fn spectrum_shows_both_channels_and_holds_peaks() {
        let rate = 48_000.0;
        let history = sine_history(1_000.0, rate);
        let settings = SpectrumSettings {
            channels: ChannelMode::LeftRight,
            averaging: Averaging::Off,
            window: Window::FlatTop,
            peak_hold: true,
            ..SpectrumSettings::default()
        };
        let mut stage = SpectrumStage::new(settings, rate);
        stage.tick(&history);
        let mut frame = SpectrumFrame::default();
        stage.write(&mut frame);
        let bin = (1_000.0 / frame.bin_hz).round() as usize;
        assert!(frame.primary_db[bin].abs() < 0.1);
        assert!((frame.secondary_db[bin] + 6.02).abs() < 0.1);
        assert_eq!(frame.primary_peak_db[bin], frame.primary_db[bin]);
        stage.tick(&History::default());
        stage.write(&mut frame);
        assert!(frame.primary_db[bin] < -150.0, "no averaging");
        assert!(frame.primary_peak_db[bin].abs() < 0.1, "peak held");
    }

    #[test]
    fn octave_smoothing_keeps_flat_spectra_flat() {
        let power = vec![0.5_f32; 1_000];
        let mut prefix = vec![0.0; 1_001];
        let mut output = vec![0.0; 1_000];
        smooth(&power, Some(1.0 / 3.0), &mut prefix, &mut output);
        assert!(output.iter().all(|db| (db + 3.0103).abs() < 1e-3));
    }

    #[test]
    fn stereo_lanes_separate_the_channels() {
        let rate = 48_000.0;
        let mut history = History::default();
        for n in 0..32_768 {
            // A tone on the left only.
            let value = (TAU * 1_000.0 * n as f32 / rate).sin();
            history.push([value, 0.0]);
        }
        let settings = SonogramSettings {
            channels: Lanes::LeftRight,
            ..SonogramSettings::default()
        };
        let mut stage = SonogramStage::new(settings, rate);
        let mut feed = SonogramFeed::default();
        stage.observe([0.5, -0.25]);
        stage.tick(&history, &mut feed);
        let column = feed.columns.back().expect("a column");
        assert_eq!(feed.lanes, 2);
        assert_eq!(column.levels.len(), 2 * SONOGRAM_ROWS);
        let loudest = |levels: &[f32]| levels.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            loudest(&column.levels[..SONOGRAM_ROWS]) > -3.0,
            "left has the tone"
        );
        assert!(
            loudest(&column.levels[SONOGRAM_ROWS..]) < -100.0,
            "right is silent"
        );
        assert_eq!(column.wave[0].max, 0.5);
        assert_eq!(column.wave[1].min, -0.25);
    }

    #[test]
    fn sonogram_columns_peak_at_the_tone() {
        let rate = 48_000.0;
        let history = sine_history(1_000.0, rate);
        let mut stage = SonogramStage::new(SonogramSettings::default(), rate);
        let mut feed = SonogramFeed::default();
        for value in [0.5, -0.25, 0.0, 0.0] {
            stage.observe([value, value]);
        }
        stage.tick(&history, &mut feed);
        let wave = feed.columns.back().expect("a column").wave[0];
        assert_eq!((wave.min, wave.max), (-0.25, 0.5));
        assert!((wave.rms - (0.3125_f32 / 4.0).sqrt()).abs() < 1e-6);
        let column = &feed.columns.back().expect("a column").levels;
        let loudest = column
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(row, _)| row)
            .unwrap_or_default();
        let (low, high) = feed.range_hz;
        let position = (1_000.0_f32 / low).ln() / (high / low).ln();
        let expected = (position * column.len() as f32) as usize;
        assert!(
            loudest.abs_diff(expected) <= 2,
            "row {loudest}, expected {expected}"
        );
        assert_eq!(feed.generation, stage.generation());
    }
}
