//! View and analysis settings, chosen in the editor and stored with the plugin state.
//!
//! The settings are persisted by name (`#[persist = "settings"]`), so enum variant and
//! field names are part of saved sessions: never rename them. Fields missing from older
//! sessions take their defaults.

use serde::{Deserialize, Serialize};

use crate::layout::OverviewLayout;
use crate::style::{PlotStyle, Rgb, ViewStyles};

mod choice;
mod legacy;
mod range;

pub use choice::{
    Averaging, ChannelMode, Choice, ColorMap, ColorScheme, FftSize, FreqLimit, FreqScale,
    HistorySpan, Lanes, LoudnessScale, LoudnessTarget, ScopeGain, ScopeLayout, ScopeMode,
    ScopeSamples, ScopeZoom, Signal, Smoothing, Speed, SurfaceBands, SurfaceDepth, SurfaceMode,
    Tab, TimeSpan, TriggerMode, WaveScale, Window,
};
pub use range::{LEVEL_RANGE_DB, Range, Taper};

use legacy::{level_range_db, slope_db};

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
