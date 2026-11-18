//! View and analysis settings, chosen in the editor and stored with the plugin state.
//!
//! The settings are persisted by name (`#[persist = "settings"]`), so enum variant and
//! field names are part of saved sessions: never rename them. Fields missing from older
//! sessions take their defaults.

use serde::{Deserialize, Serialize};

use crate::layout::OverviewLayout;

/// A setting with a fixed set of choices, shown as a segmented selector.
pub trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    /// Short labels, in the order of [`ALL`](Self::ALL).
    const LABELS: &'static [&'static str];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|choice| *choice == self)
            .unwrap_or(0)
    }

    fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or(Self::ALL[0])
    }
}

macro_rules! choice {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident => $label:literal),+ $(,)? } default $default:ident
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $($variant),+
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }

        impl Choice for $name {
            const ALL: &'static [Self] = &[$(Self::$variant),+];
            const LABELS: &'static [&'static str] = &[$($label),+];
        }
    };
}

choice! {
    /// The editor page.
    Tab {
        Overview => "OVERVIEW",
        Spectral => "SPECTRAL",
        Spectrum => "SPECTRUM",
        Sonogram => "SONOGRAM",
        Oscilloscope => "SCOPE",
        Stereo => "STEREO",
        Loudness => "LOUDNESS",
        Surface => "3D",
    } default Overview
}

choice! {
    /// FFT length in samples.
    FftSize {
        S1k => "1K",
        S2k => "2K",
        S4k => "4K",
        S8k => "8K",
        S16k => "16K",
        S32k => "32K",
    } default S8k
}

impl FftSize {
    /// Largest FFT length in samples.
    pub const MAX: usize = 32_768;

    /// Length in samples.
    pub fn samples(self) -> usize {
        1_024 << self.index()
    }
}

choice! {
    /// FFT window. Blackman-Harris trades resolution for leakage; flat-top reads the level
    /// of sines exactly.
    Window {
        Hann => "HANN",
        BlackmanHarris => "B-HARRIS",
        FlatTop => "FLAT-TOP",
    } default Hann
}

choice! {
    /// Time averaging of the spectrum.
    Averaging {
        Off => "OFF",
        Fast => "FAST",
        Medium => "MED",
        Slow => "SLOW",
        Infinite => "INF",
    } default Medium
}

impl Averaging {
    /// Time constant of the exponential average in seconds; `None` without averaging,
    /// infinity for the running mean.
    pub fn time_constant_s(self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Fast => Some(0.08),
            Self::Medium => Some(0.3),
            Self::Slow => Some(1.2),
            Self::Infinite => Some(f32::INFINITY),
        }
    }
}

choice! {
    /// Fractional-octave smoothing of the spectrum.
    Smoothing {
        Off => "OFF",
        Oct24 => "1/24",
        Oct12 => "1/12",
        Oct6 => "1/6",
        Oct3 => "1/3",
    } default Off
}

impl Smoothing {
    /// Smoothing bandwidth in octaves.
    pub fn octaves(self) -> Option<f32> {
        match self {
            Self::Off => None,
            Self::Oct24 => Some(1.0 / 24.0),
            Self::Oct12 => Some(1.0 / 12.0),
            Self::Oct6 => Some(1.0 / 6.0),
            Self::Oct3 => Some(1.0 / 3.0),
        }
    }
}

choice! {
    /// Display tilt around 1 kHz in dB per octave; 3 dB shows pink noise flat.
    Slope {
        Flat => "0",
        Db3 => "3",
        Db45 => "4.5",
    } default Flat
}

impl Slope {
    /// Tilt in dB per octave.
    pub fn db_per_octave(self) -> f32 {
        match self {
            Self::Flat => 0.0,
            Self::Db3 => 3.0,
            Self::Db45 => 4.5,
        }
    }
}

choice! {
    /// A signal derived from the stereo input.
    Signal {
        Mid => "MID",
        Side => "SIDE",
        Left => "LEFT",
        Right => "RIGHT",
    } default Mid
}

impl Signal {
    /// The signal's value in a stereo frame; mid and side are `(L ± R) / 2`.
    pub fn of(self, [left, right]: [f32; 2]) -> f32 {
        match self {
            Self::Mid => 0.5 * (left + right),
            Self::Side => 0.5 * (left - right),
            Self::Left => left,
            Self::Right => right,
        }
    }
}

choice! {
    /// The spectrum curves: two overlaid channels or one.
    ChannelMode {
        LeftRight => "L · R",
        MidSide => "M · S",
        Mid => "MID",
        Left => "LEFT",
        Right => "RIGHT",
    } default LeftRight
}

impl ChannelMode {
    /// The primary curve's signal and the secondary one, if shown.
    pub fn signals(self) -> (Signal, Option<Signal>) {
        match self {
            Self::LeftRight => (Signal::Left, Some(Signal::Right)),
            Self::MidSide => (Signal::Mid, Some(Signal::Side)),
            Self::Mid => (Signal::Mid, None),
            Self::Left => (Signal::Left, None),
            Self::Right => (Signal::Right, None),
        }
    }
}

choice! {
    /// Shown level range in dB.
    DbRange {
        R60 => "60 dB",
        R90 => "90 dB",
        R120 => "120 dB",
    } default R90
}

impl DbRange {
    /// Range in dB.
    pub fn db(self) -> f32 {
        match self {
            Self::R60 => 60.0,
            Self::R90 => 90.0,
            Self::R120 => 120.0,
        }
    }
}

choice! {
    /// Highest shown frequency: 20 kHz or the Nyquist frequency.
    FreqLimit {
        Audible => "20K",
        Nyquist => "NYQ",
    } default Audible
}

impl FreqLimit {
    /// Lowest shown frequency of logarithmic scales in Hz.
    pub const LOW_HZ: f32 = 20.0;

    /// Highest shown frequency in Hz at `sample_rate` Hz.
    pub fn high_hz(self, sample_rate: f32) -> f32 {
        let nyquist = 0.5 * sample_rate.max(8_000.0);
        match self {
            Self::Audible => nyquist.min(20_000.0),
            Self::Nyquist => nyquist,
        }
    }
}

choice! {
    /// Frequency axis: logarithmic, linear, or logarithmic with a grid and labels of
    /// semitones and notes.
    FreqScale {
        Log => "LOG",
        Linear => "LIN",
        Semitone => "ST",
    } default Log
}

choice! {
    /// Sonogram scroll speed in columns per second.
    Speed {
        S15 => "15/s",
        S30 => "30/s",
        S60 => "60/s",
        S120 => "120/s",
        S240 => "240/s",
    } default S60
}

impl Speed {
    /// Columns per second.
    pub fn columns_per_s(self) -> f32 {
        15.0 * (1 << self.index()) as f32
    }
}

choice! {
    /// Level colors of the sonogram and the 3D views.
    ColorMap {
        Ember => "EMBER",
        Viridis => "VIRIDIS",
        Ice => "ICE",
        Gray => "GRAY",
        Custom => "CUSTOM",
    } default Ember
}

choice! {
    /// Stereo scope drawing.
    ScopeMode {
        Lines => "LINES",
        Dots => "DOTS",
        Polar => "POLAR",
        Xy => "L / R",
    } default Dots
}

choice! {
    /// Samples drawn by the stereo scope per frame.
    ScopeSamples {
        S512 => "512",
        S1k => "1K",
        S2k => "2K",
        S4k => "4K",
    } default S2k
}

impl ScopeSamples {
    /// Largest number of scope samples.
    pub const MAX: usize = 4_096;

    pub fn count(self) -> usize {
        512 << self.index()
    }
}

choice! {
    /// Stereo scope scaling: follow the signal, or full scale at the edge.
    ScopeGain {
        Auto => "AUTO",
        Unity => "0 dBFS",
    } default Auto
}

choice! {
    /// Integrated loudness target in LUFS.
    LoudnessTarget {
        Streaming => "−14",
        Podcast => "−16",
        Ebu => "−23",
        Atsc => "−24",
    } default Streaming
}

impl LoudnessTarget {
    /// Target in LUFS.
    pub fn lufs(self) -> f32 {
        match self {
            Self::Streaming => -14.0,
            Self::Podcast => -16.0,
            Self::Ebu => -23.0,
            Self::Atsc => -24.0,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Streaming => "Streaming platforms",
            Self::Podcast => "Podcasts, Apple Music",
            Self::Ebu => "EBU R 128 broadcast",
            Self::Atsc => "ATSC A/85 broadcast",
        }
    }
}

choice! {
    /// Loudness readouts: absolute LUFS or LU relative to the target.
    LoudnessScale {
        Absolute => "LUFS",
        Relative => "LU",
    } default Absolute
}

choice! {
    /// Length of the loudness history graph.
    HistorySpan {
        S30 => "30 S",
        M2 => "2 MIN",
        M10 => "10 MIN",
    } default M2
}

impl HistorySpan {
    /// Length in seconds.
    pub fn seconds(self) -> f32 {
        match self {
            Self::S30 => 30.0,
            Self::M2 => 120.0,
            Self::M10 => 600.0,
        }
    }
}

choice! {
    /// The 3D view: spectrum history as ridges or a surface, or the stereo field.
    SurfaceMode {
        Ridges => "RIDGES",
        Surface => "SURFACE",
        Stereogram => "STEREOGRAM",
    } default Surface
}

choice! {
    /// Frequency bands of the 3D views.
    SurfaceBands {
        B64 => "64",
        B128 => "128",
        B256 => "256",
    } default B128
}

impl SurfaceBands {
    /// Largest band count.
    pub const MAX: usize = 256;

    pub fn count(self) -> usize {
        64 << self.index()
    }
}

choice! {
    /// History rows of the 3D spectrum views.
    SurfaceDepth {
        D32 => "32",
        D64 => "64",
        D128 => "128",
    } default D64
}

impl SurfaceDepth {
    /// Largest row count.
    pub const MAX: usize = 128;

    pub fn count(self) -> usize {
        32 << self.index()
    }
}

choice! {
    /// Oscilloscope time span across the screen.
    TimeSpan {
        Ms1 => "1",
        Ms2 => "2",
        Ms5 => "5",
        Ms10 => "10",
        Ms20 => "20",
        Ms50 => "50",
        Ms100 => "100",
        Ms200 => "200",
        Ms500 => "500",
    } default Ms20
}

impl TimeSpan {
    /// Span in seconds.
    pub fn seconds(self) -> f32 {
        const MS: [f32; 9] = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0];
        MS[self.index()] / 1_000.0
    }
}

choice! {
    /// Oscilloscope trigger: free running, or on a rising or falling crossing of the
    /// trigger level. Without a crossing the newest samples are shown.
    TriggerMode {
        Free => "FREE",
        Rising => "RISE",
        Falling => "FALL",
    } default Rising
}

choice! {
    /// Oscilloscope vertical gain: following the signal, or a fixed factor.
    ScopeZoom {
        Auto => "AUTO",
        X1 => "×1",
        X2 => "×2",
        X4 => "×4",
        X8 => "×8",
        X16 => "×16",
        X32 => "×32",
    } default Auto
}

impl ScopeZoom {
    /// Fixed gain factor; `None` for automatic gain.
    pub fn factor(self) -> Option<f32> {
        match self {
            Self::Auto => None,
            fixed => Some((1 << (fixed.index() - 1)) as f32),
        }
    }
}

choice! {
    /// Oscilloscope traces: overlaid or in lanes of their own.
    ScopeLayout {
        Overlay => "OVERLAY",
        Stacked => "STACKED",
    } default Overlay
}

choice! {
    /// Trace and accent colors of every view.
    ColorScheme {
        Oat => "O-AT",
        Phosphor => "PHOSPHOR",
        Sunset => "SUNSET",
        Ocean => "OCEAN",
        Mono => "MONO",
        Contrast => "CONTRAST",
        Custom => "CUSTOM",
    } default Oat
}

/// An sRGB color.
pub type Rgb = [u8; 3];

/// The colors of the CUSTOM scheme and heat map.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomColors {
    pub accent: Rgb,
    /// First trace: left, mid or a single signal.
    pub primary: Rgb,
    /// Second trace: right.
    pub secondary: Rgb,
    pub side: Rgb,
    /// Plot backgrounds.
    pub plot: Rgb,
    pub grid: Rgb,
    /// Heat map from quiet through the middle to loud.
    pub heat: [Rgb; 3],
}

impl Default for CustomColors {
    fn default() -> Self {
        Self {
            accent: [0xc6, 0xe0, 0x5a],
            primary: [0xc6, 0xe0, 0x5a],
            secondary: [0x5c, 0xa8, 0xff],
            side: [0xa4, 0x8b, 0xfa],
            plot: [0x13, 0x15, 0x1a],
            grid: [0x25, 0x2a, 0x34],
            heat: [[0x0b, 0x0d, 0x12], [0xd9, 0x3f, 0x5b], [0xfc, 0xf3, 0xa6]],
        }
    }
}

choice! {
    /// Waveform amplitude scale: linear, or dB from −60 dBFS at the centre.
    WaveScale {
        Linear => "LIN",
        Db => "dB",
    } default Linear
}

/// How a slider's position maps to its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taper {
    Linear,
    /// Logarithmic, like an audio fader: the lower values get most of the travel, which
    /// suits what the eye and the ear perceive. The value at mid travel is about 9 % of
    /// the range.
    Log,
}

/// Steepness of [`Taper::Log`]: the value spans `LOG_TAPER_BASE` to one.
const LOG_TAPER_BASE: f32 = 101.0;

/// A continuous setting's range, default and taper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub taper: Taper,
}

impl Range {
    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.default
        }
    }

    /// Slider position from 0 to 1 of `value`.
    pub fn position(&self, value: f32) -> f32 {
        let fraction = (self.clamp(value) - self.min) / (self.max - self.min);
        match self.taper {
            Taper::Linear => fraction,
            Taper::Log => (1.0 + (LOG_TAPER_BASE - 1.0) * fraction).ln() / LOG_TAPER_BASE.ln(),
        }
    }

    /// Value at slider `position` from 0 to 1.
    pub fn value(&self, position: f32) -> f32 {
        let position = position.clamp(0.0, 1.0);
        let fraction = match self.taper {
            Taper::Linear => position,
            Taper::Log => (LOG_TAPER_BASE.powf(position) - 1.0) / (LOG_TAPER_BASE - 1.0),
        };
        self.min + fraction * (self.max - self.min)
    }
}

/// Spectral view settings: the sonogram layer and the waveform layer on top of it. The
/// sonogram settings, brightness and contrast included, apply too.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectralSettings {
    pub scale: WaveScale,
    /// Opacity of the sonogram layer, 0..1.
    pub sonogram_opacity: f32,
    /// Opacity of the waveform layer, 0..1.
    pub waveform_opacity: f32,
    /// Vertical zoom of the waveform in dB.
    pub waveform_gain_db: f32,
}

impl SpectralSettings {
    pub const OPACITY: Range = Range {
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Log,
    };
    pub const WAVEFORM_GAIN_DB: Range = Range {
        min: 0.0,
        max: 36.0,
        default: 0.0,
        taper: Taper::Linear,
    };
}

impl Default for SpectralSettings {
    fn default() -> Self {
        Self {
            scale: WaveScale::default(),
            sonogram_opacity: 1.0,
            waveform_opacity: 0.85,
            waveform_gain_db: 0.0,
        }
    }
}

/// Oscilloscope settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OscilloscopeSettings {
    pub span: TimeSpan,
    pub channels: ChannelMode,
    pub trigger: TriggerMode,
    /// Signal the trigger watches.
    pub source: Signal,
    /// Trigger level as a fraction of the screen's half height, −1..1.
    pub level: f32,
    pub zoom: ScopeZoom,
    pub layout: ScopeLayout,
}

impl Default for OscilloscopeSettings {
    fn default() -> Self {
        Self {
            span: TimeSpan::default(),
            channels: ChannelMode::default(),
            trigger: TriggerMode::default(),
            source: Signal::Mid,
            level: 0.0,
            zoom: ScopeZoom::default(),
            layout: ScopeLayout::default(),
        }
    }
}

/// Spectrum analyzer settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumSettings {
    pub fft: FftSize,
    pub window: Window,
    pub averaging: Averaging,
    pub smoothing: Smoothing,
    pub slope: Slope,
    pub channels: ChannelMode,
    pub range: DbRange,
    pub limit: FreqLimit,
    pub scale: FreqScale,
    /// Shows the highest level per frequency until reset.
    pub peak_hold: bool,
}

/// Sonogram settings. The 3D spectrum views use the same FFT.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SonogramSettings {
    pub fft: FftSize,
    pub signal: Signal,
    pub scale: FreqScale,
    pub speed: Speed,
    pub range: DbRange,
    pub limit: FreqLimit,
    /// Level offset of the image in dB: raises or lowers every level before coloring.
    pub brightness_db: f32,
    /// Steepness of the color scale around its middle; one is linear in dB.
    pub contrast: f32,
}

impl SonogramSettings {
    pub const BRIGHTNESS_DB: Range = Range {
        min: -30.0,
        max: 30.0,
        default: 0.0,
        taper: Taper::Linear,
    };
    pub const CONTRAST: Range = Range {
        min: 0.3,
        max: 3.0,
        default: 1.0,
        taper: Taper::Linear,
    };

    /// Position from 0 (quiet) to 1 (loud) of a level in dB on the color scale.
    pub fn tone(&self, db: f32) -> f32 {
        let range = self.range.db();
        let linear = (db + self.brightness_db + range) / range;
        (0.5 + (linear - 0.5) * self.contrast).clamp(0.0, 1.0)
    }
}

impl Default for SonogramSettings {
    fn default() -> Self {
        Self {
            fft: FftSize::S4k,
            signal: Signal::Mid,
            scale: FreqScale::Log,
            speed: Speed::S60,
            range: DbRange::R90,
            limit: FreqLimit::Audible,
            brightness_db: 0.0,
            contrast: 1.0,
        }
    }
}

/// Stereo imager settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StereoSettings {
    pub mode: ScopeMode,
    pub samples: ScopeSamples,
    pub gain: ScopeGain,
}

/// Loudness meter settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessSettings {
    pub target: LoudnessTarget,
    pub scale: LoudnessScale,
    pub span: HistorySpan,
}

/// 3D view settings, including the camera.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceSettings {
    pub mode: SurfaceMode,
    pub bands: SurfaceBands,
    pub depth: SurfaceDepth,
    pub range: DbRange,
    /// Camera rotation around the vertical axis in degrees.
    pub yaw_deg: f32,
    /// Camera elevation in degrees.
    pub pitch_deg: f32,
    /// Camera zoom factor; one fits the scene.
    pub zoom: f32,
}

impl SurfaceSettings {
    pub const DEFAULT_YAW_DEG: f32 = -28.0;
    pub const DEFAULT_PITCH_DEG: f32 = 32.0;
    pub const PITCH_RANGE_DEG: (f32, f32) = (4.0, 89.0);
    pub const ZOOM_RANGE: (f32, f32) = (0.5, 3.0);

    /// Resets the camera to its default view.
    pub fn reset_camera(&mut self) {
        self.yaw_deg = Self::DEFAULT_YAW_DEG;
        self.pitch_deg = Self::DEFAULT_PITCH_DEG;
        self.zoom = 1.0;
    }
}

impl Default for SurfaceSettings {
    fn default() -> Self {
        Self {
            mode: SurfaceMode::default(),
            bands: SurfaceBands::default(),
            depth: SurfaceDepth::default(),
            range: DbRange::R90,
            yaw_deg: Self::DEFAULT_YAW_DEG,
            pitch_deg: Self::DEFAULT_PITCH_DEG,
            zoom: 1.0,
        }
    }
}

/// All settings of one analyzer instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyzerSettings {
    pub tab: Tab,
    pub scheme: ColorScheme,
    pub colormap: ColorMap,
    /// Colors of the CUSTOM scheme and heat map.
    pub custom: CustomColors,
    pub overview: OverviewLayout,
    pub spectral: SpectralSettings,
    pub spectrum: SpectrumSettings,
    pub sonogram: SonogramSettings,
    pub stereo: StereoSettings,
    pub oscilloscope: OscilloscopeSettings,
    pub loudness: LoudnessSettings,
    pub surface: SurfaceSettings,
}

#[cfg(test)]
mod tests {
    use super::{
        Choice, FftSize, ScopeSamples, ScopeZoom, SonogramSettings, SpectralSettings, Speed,
        SurfaceBands, SurfaceDepth, TimeSpan,
    };

    #[test]
    fn sizes_follow_their_labels() {
        assert_eq!(FftSize::S1k.samples(), 1_024);
        assert_eq!(FftSize::S32k.samples(), FftSize::MAX);
        assert_eq!(ScopeSamples::S4k.count(), ScopeSamples::MAX);
        assert_eq!(SurfaceBands::B256.count(), SurfaceBands::MAX);
        assert_eq!(SurfaceDepth::D128.count(), SurfaceDepth::MAX);
        assert_eq!(Speed::S240.columns_per_s(), 240.0);
        assert_eq!(FftSize::ALL.len(), FftSize::LABELS.len());
        assert_eq!(FftSize::from_index(2), FftSize::S4k);
        assert_eq!(TimeSpan::Ms500.seconds(), 0.5);
        assert_eq!(ScopeZoom::Auto.factor(), None);
        assert_eq!(ScopeZoom::X32.factor(), Some(32.0));
        let opacity = SpectralSettings::OPACITY;
        assert!(
            (opacity.value(0.5) - 0.0905).abs() < 1e-3,
            "log taper at mid travel"
        );
        for value in [0.0, 0.05, 0.5, 1.0] {
            assert!((opacity.value(opacity.position(value)) - value).abs() < 1e-5);
        }
        let mut sonogram = SonogramSettings::default();
        assert_eq!(sonogram.tone(0.0), 1.0);
        assert_eq!(sonogram.tone(-45.0), 0.5);
        sonogram.contrast = 2.0;
        sonogram.brightness_db = 10.0;
        assert_eq!(sonogram.tone(-55.0), 0.5, "brightness moves the middle");
        assert_eq!(sonogram.tone(-32.5), 1.0, "contrast steepens the scale");
    }
}
