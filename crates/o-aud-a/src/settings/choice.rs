//! Settings with a fixed set of choices, shown as segmented selectors.
//!
//! Variant names are stored in saved sessions: never rename them.

use serde::{Deserialize, Serialize};

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
/// Lines or dots are a separate setting
/// ([`StereoSettings::lines`](super::StereoSettings::lines)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeMode {
    /// Earlier versions' goniometer drawn with lines; read as [`Dots`](Self::Dots) with
    /// lines on (see [`StereoSettings::upgrade`](super::StereoSettings::upgrade)).
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
