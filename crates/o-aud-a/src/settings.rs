//! View and analysis settings, chosen in the editor and stored with the plugin state.
//!
//! The settings are persisted by name (`#[persist = "settings"]`), so enum variant and
//! field names are part of saved sessions: never rename them. Fields missing from older
//! sessions take their defaults.

use serde::{Deserialize, Deserializer, Serialize};

use crate::layout::OverviewLayout;
use crate::style::{PlotStyle, Rgb, ViewStyles};

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

/// Display tilt choices of earlier versions, kept to read their sessions; the tilt is
/// now continuous ([`SpectrumSettings::slope`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum Slope {
    Flat,
    Db3,
    Db45,
}

impl Slope {
    /// Tilt in dB per octave.
    fn db_per_octave(self) -> f32 {
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
    /// Lanes of the sonogram, spectral and 3D views: one signal, or left and right, or
    /// mid and side, each in a lane of its own.
    Lanes {
        Single => "SINGLE",
        LeftRight => "L · R",
        MidSide => "M · S",
    } default Single
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

/// Level range choices of earlier versions, kept to read their sessions; the range is
/// now continuous ([`LEVEL_RANGE_DB`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum DbRange {
    R60,
    R90,
    R120,
}

impl DbRange {
    /// Range in dB.
    fn db(self) -> f32 {
        match self {
            Self::R60 => 60.0,
            Self::R90 => 90.0,
            Self::R120 => 120.0,
        }
    }
}

/// A continuous setting as stored: a number, or the choice that earlier versions stored.
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored<T> {
    Number(f32),
    Choice(T),
}

/// Reads a level range in dB, also from a [`DbRange`] variant name.
fn level_range_db<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    Ok(match Stored::<DbRange>::deserialize(deserializer)? {
        Stored::Number(db) => LEVEL_RANGE_DB.clamp(db),
        Stored::Choice(range) => range.db(),
    })
}

/// Reads a display tilt in dB per octave, also from a [`Slope`] variant name.
fn slope_db<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    Ok(match Stored::<Slope>::deserialize(deserializer)? {
        Stored::Number(db) => SpectrumSettings::SLOPE_DB.clamp(db),
        Stored::Choice(slope) => slope.db_per_octave(),
    })
}

/// Shown level range of the spectrum, sonogram and 3D views in dB.
pub const LEVEL_RANGE_DB: Range = Range {
    min: 30.0,
    max: 150.0,
    default: 90.0,
    taper: Taper::Linear,
};

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

/// Stereo scope diagram: mid and side (a goniometer), polar, or left against right.
/// Lines or dots are a separate setting ([`StereoSettings::lines`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeMode {
    /// Earlier versions' goniometer drawn with lines; read as [`Dots`](Self::Dots) with
    /// lines on (see [`StereoSettings::upgrade`]).
    Lines,
    /// The mid and side goniometer.
    #[default]
    Dots,
    Polar,
    Xy,
}

impl Choice for ScopeMode {
    const ALL: &'static [Self] = &[Self::Dots, Self::Polar, Self::Xy];
    const LABELS: &'static [&'static str] = &["M / S", "POLAR", "L / R"];
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
    /// Integrated loudness target preset, loudest first. Persisted by variant name.
    LoudnessTarget {
        SpotifyLoud => "−11",
        Streaming => "−14",
        Deezer => "−15",
        Podcast => "−16",
        Aes => "−18",
        SpotifyQuiet => "−19",
        Ebu => "−23",
        Atsc => "−24",
        Custom => "CUSTOM",
    } default Streaming
}

impl LoudnessTarget {
    /// Target in LUFS; `None` for [`Self::Custom`], whose value is a setting.
    pub fn lufs(self) -> Option<f32> {
        match self {
            Self::SpotifyLoud => Some(-11.0),
            Self::Streaming => Some(-14.0),
            Self::Deezer => Some(-15.0),
            Self::Podcast => Some(-16.0),
            Self::Aes => Some(-18.0),
            Self::SpotifyQuiet => Some(-19.0),
            Self::Ebu => Some(-23.0),
            Self::Atsc => Some(-24.0),
            Self::Custom => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::SpotifyLoud => "Spotify Loud",
            Self::Streaming => "Spotify, YouTube, Tidal, Amazon",
            Self::Deezer => "Deezer",
            Self::Podcast => "Apple Music, podcasts",
            Self::Aes => "AES streaming (TD1008)",
            Self::SpotifyQuiet => "Spotify Quiet",
            Self::Ebu => "EBU R 128 broadcast",
            Self::Atsc => "ATSC A/85 broadcast",
            Self::Custom => "Custom",
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
        Ms1 => "1ms",
        Ms2 => "2",
        Ms5 => "5",
        Ms10 => "10",
        Ms20 => "20",
        Ms50 => "50",
        Ms100 => "100",
        Ms200 => "200",
        Ms500 => "500",
        S1 => "1s",
        S2 => "2s",
        S5 => "5s",
        S10 => "10s",
        S30 => "30s",
        S60 => "60s",
    } default Ms20
}

impl TimeSpan {
    /// Span in seconds.
    pub fn seconds(self) -> f32 {
        const MS: [f32; 15] = [
            1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0,
            10_000.0, 30_000.0, 60_000.0,
        ];
        MS[self.index()] / 1_000.0
    }

    /// Spans drawn from the envelope history, scrolling, instead of from samples.
    pub fn is_roll(self) -> bool {
        self.seconds() > 0.5
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
    /// The theme: accent of buttons, tabs, sliders and panel marks. Never colors a
    /// diagram; CUSTOM uses the theme's own accent color.
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

/// Spectral view settings: the sonogram layer and the waveform layer on top of it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectralSettings {
    /// The sonogram layer's own analysis and tone, independent of the sonogram view.
    /// `None` until set: earlier versions shared the sonogram's settings, and
    /// [`AnalyzerSettings::upgrade`] starts from a copy of them.
    pub sonogram: Option<SonogramSettings>,
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
            sonogram: None,
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumSettings {
    pub fft: FftSize,
    pub window: Window,
    pub averaging: Averaging,
    pub smoothing: Smoothing,
    /// Display tilt around 1 kHz in dB per octave; 3 dB shows pink noise flat.
    #[serde(deserialize_with = "slope_db")]
    pub slope: f32,
    pub channels: ChannelMode,
    /// Shown level range in dB.
    #[serde(deserialize_with = "level_range_db")]
    pub range: f32,
    pub limit: FreqLimit,
    pub scale: FreqScale,
    /// Shows the highest level per frequency until reset.
    pub peak_hold: bool,
}

impl SpectrumSettings {
    pub const SLOPE_DB: Range = Range {
        min: 0.0,
        max: 6.0,
        default: 0.0,
        taper: Taper::Linear,
    };
}

impl Default for SpectrumSettings {
    fn default() -> Self {
        Self {
            fft: FftSize::default(),
            window: Window::default(),
            averaging: Averaging::default(),
            smoothing: Smoothing::default(),
            slope: Self::SLOPE_DB.default,
            channels: ChannelMode::default(),
            range: LEVEL_RANGE_DB.default,
            limit: FreqLimit::default(),
            scale: FreqScale::default(),
            peak_hold: false,
        }
    }
}

/// Sonogram settings. The 3D spectrum views use the same FFT.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SonogramSettings {
    pub fft: FftSize,
    pub signal: Signal,
    pub scale: FreqScale,
    pub speed: Speed,
    /// Shown level range in dB.
    #[serde(deserialize_with = "level_range_db")]
    pub range: f32,
    pub limit: FreqLimit,
    /// One lane of `signal`, or two lanes.
    pub channels: Lanes,
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

    /// The signal of each lane: the first, and the second if there are two.
    pub fn signals(&self) -> (Signal, Option<Signal>) {
        match self.channels {
            Lanes::Single => (self.signal, None),
            Lanes::LeftRight => (Signal::Left, Some(Signal::Right)),
            Lanes::MidSide => (Signal::Mid, Some(Signal::Side)),
        }
    }

    /// Names of the lanes' signals; the second is empty with one lane.
    pub fn lane_names(&self) -> [&'static str; 2] {
        let (first, second) = self.signals();
        [
            Signal::LABELS[first.index()],
            second.map_or("", |signal| Signal::LABELS[signal.index()]),
        ]
    }

    /// Number of lanes.
    pub fn lanes(&self) -> usize {
        if self.channels == Lanes::Single { 1 } else { 2 }
    }

    /// Position from 0 (quiet) to 1 (loud) of a level in dB on the color scale.
    pub fn tone(&self, db: f32) -> f32 {
        let range = self.range;
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
            range: LEVEL_RANGE_DB.default,
            limit: FreqLimit::Audible,
            channels: Lanes::Single,
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
    /// Joins the scope's samples with lines instead of drawing dots, in every diagram.
    pub lines: bool,
}

impl StereoSettings {
    /// Turns the line mode of earlier versions into the goniometer with lines.
    pub fn upgrade(&mut self) {
        if self.mode == ScopeMode::Lines {
            self.mode = ScopeMode::Dots;
            self.lines = true;
        }
    }
}

/// Loudness meter settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessSettings {
    pub target: LoudnessTarget,
    pub scale: LoudnessScale,
    pub span: HistorySpan,
    /// The custom target in LUFS.
    pub custom_lufs: f32,
    /// Shows the streaming platforms' normalization in place of the history.
    pub platforms: bool,
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            target: LoudnessTarget::default(),
            scale: LoudnessScale::default(),
            span: HistorySpan::default(),
            custom_lufs: -14.0,
            platforms: false,
        }
    }
}

impl LoudnessSettings {
    /// The custom target's range in LUFS.
    pub const CUSTOM_LUFS: Range = Range {
        min: -36.0,
        max: -6.0,
        default: -14.0,
        taper: Taper::Linear,
    };

    /// The integrated loudness target in LUFS.
    pub fn target_lufs(&self) -> f32 {
        self.target
            .lufs()
            .unwrap_or_else(|| Self::CUSTOM_LUFS.clamp(self.custom_lufs).round())
    }
}

/// 3D view settings, including the camera.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceSettings {
    pub mode: SurfaceMode,
    pub bands: SurfaceBands,
    pub depth: SurfaceDepth,
    /// Shown level range in dB.
    #[serde(deserialize_with = "level_range_db")]
    pub range: f32,
    /// Camera rotation around the vertical axis in degrees.
    pub yaw_deg: f32,
    /// Camera elevation in degrees.
    pub pitch_deg: f32,
    /// Camera zoom factor; one fits the scene.
    pub zoom: f32,
    /// Draws a grid on the floor and the back walls of the scene.
    pub grid: bool,
    /// Horizontal shift of the scene as a fraction of the view's width, right positive.
    pub pan_x: f32,
    /// Vertical shift of the scene as a fraction of the view's height, down positive.
    pub pan_y: f32,
}

impl SurfaceSettings {
    pub const DEFAULT_YAW_DEG: f32 = -28.0;
    pub const DEFAULT_PITCH_DEG: f32 = 32.0;
    pub const PITCH_RANGE_DEG: (f32, f32) = (4.0, 89.0);
    pub const ZOOM: Range = Range {
        min: 0.25,
        max: 8.0,
        default: 1.0,
        taper: Taper::Log,
    };
    /// Largest shift of the scene as a fraction of the view's size.
    pub const PAN_LIMIT: f32 = 1.5;

    /// Resets the camera to its default view.
    pub fn reset_camera(&mut self) {
        self.yaw_deg = Self::DEFAULT_YAW_DEG;
        self.pitch_deg = Self::DEFAULT_PITCH_DEG;
        self.zoom = Self::ZOOM.default;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
    }
}

impl Default for SurfaceSettings {
    fn default() -> Self {
        Self {
            mode: SurfaceMode::default(),
            bands: SurfaceBands::default(),
            depth: SurfaceDepth::default(),
            range: LEVEL_RANGE_DB.default,
            yaw_deg: Self::DEFAULT_YAW_DEG,
            pitch_deg: Self::DEFAULT_PITCH_DEG,
            zoom: Self::ZOOM.default,
            grid: false,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

/// All settings of one analyzer instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyzerSettings {
    pub tab: Tab,
    /// The theme of the controls.
    pub scheme: ColorScheme,
    /// Accent of the CUSTOM theme.
    pub accent: Rgb,
    /// Background and grid of every diagram.
    pub plot: PlotStyle,
    /// Each view's diagram colors.
    pub styles: ViewStyles,
    pub overview: OverviewLayout,
    pub spectral: SpectralSettings,
    pub spectrum: SpectrumSettings,
    pub sonogram: SonogramSettings,
    pub stereo: StereoSettings,
    pub oscilloscope: OscilloscopeSettings,
    pub loudness: LoudnessSettings,
    pub surface: SurfaceSettings,
}

impl AnalyzerSettings {
    /// Brings settings of earlier versions up to date.
    pub fn upgrade(&mut self) {
        self.stereo.upgrade();
        if self.spectral.sonogram.is_none() {
            self.spectral.sonogram = Some(self.sonogram);
        }
    }

    /// The spectral view's sonogram layer settings.
    pub fn spectral_sonogram(&self) -> SonogramSettings {
        self.spectral.sonogram.unwrap_or(self.sonogram)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnalyzerSettings, Choice, FftSize, LoudnessSettings, LoudnessTarget, ScopeMode,
        ScopeSamples, ScopeZoom, SonogramSettings, SpectralSettings, Speed, SurfaceBands,
        SurfaceDepth, TimeSpan,
    };

    #[test]
    fn loudness_targets_load_by_name_and_custom_targets_clamp() {
        let stored: LoudnessSettings =
            serde_json::from_str(r#"{"target":"Ebu"}"#).expect("valid JSON");
        assert_eq!(stored.target_lufs(), -23.0);
        assert!(!stored.platforms);
        let mut custom = LoudnessSettings {
            target: LoudnessTarget::Custom,
            custom_lufs: -9.6,
            ..LoudnessSettings::default()
        };
        assert_eq!(custom.target_lufs(), -10.0, "whole LUFS");
        custom.custom_lufs = -80.0;
        assert_eq!(custom.target_lufs(), -36.0);
        assert!(
            LoudnessTarget::ALL
                .windows(2)
                .all(|pair| pair[0].lufs().unwrap_or(f32::MIN) > pair[1].lufs().unwrap_or(f32::MIN)),
            "presets run from loud to quiet, CUSTOM last"
        );
    }

    #[test]
    fn sessions_of_earlier_versions_keep_their_ranges_and_scope_lines() {
        let stored = r#"{
            "spectrum": { "range": "R120", "slope": "Db45" },
            "sonogram": { "range": "R60" },
            "surface": { "range": "R90" },
            "stereo": { "mode": "Lines" }
        }"#;
        let mut settings: AnalyzerSettings = serde_json::from_str(stored).expect("old session");
        assert_eq!(settings.spectrum.range, 120.0);
        assert_eq!(settings.spectrum.slope, 4.5);
        assert_eq!(settings.sonogram.range, 60.0);
        assert_eq!(settings.surface.range, 90.0);
        assert_eq!(
            settings.spectral_sonogram(),
            settings.sonogram,
            "the spectral view keeps the sonogram settings it shared"
        );
        settings.upgrade();
        assert_eq!(settings.stereo.mode, ScopeMode::Dots);
        assert!(settings.stereo.lines);
        assert_eq!(settings.spectral.sonogram, Some(settings.sonogram));
        assert!(!settings.surface.grid);

        settings.spectrum.range = 75.0;
        settings.spectrum.slope = 1.5;
        let json = serde_json::to_string(&settings).expect("serializes");
        let again: AnalyzerSettings = serde_json::from_str(&json).expect("round trip");
        assert_eq!(again, settings);
    }

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
