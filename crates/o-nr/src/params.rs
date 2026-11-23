//! Host-visible parameters. Parameter IDs and persist keys are part of saved sessions;
//! never rename or reuse them.

use std::sync::Arc;

use nih_plug::prelude::{BoolParam, Enum, EnumParam, FloatParam, FloatRange, Params};
use nih_plug_egui::EguiState;

use crate::dsp::engine::{NoiseMode, Resolution};
use crate::dsp::profile::ProfileStore;

/// Logical editor size in pixels.
pub const EDITOR_WIDTH: u32 = 980;
pub const EDITOR_HEIGHT: u32 = 600;

/// Where the noise estimate comes from.
#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeChoice {
    /// The learned noise profile.
    #[name = "Profile"]
    Profile,
    /// A continuously tracked noise floor.
    #[name = "Adaptive"]
    Adaptive,
}

impl From<ModeChoice> for NoiseMode {
    fn from(choice: ModeChoice) -> Self {
        match choice {
            ModeChoice::Profile => NoiseMode::Profile,
            ModeChoice::Adaptive => NoiseMode::Adaptive,
        }
    }
}

/// FFT size: 1024 to 8192 samples at 48 kHz, scaled with the sample rate.
#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolutionChoice {
    #[name = "Low"]
    Low,
    #[name = "Medium"]
    Medium,
    #[name = "High"]
    High,
    #[name = "Max"]
    Max,
}

impl From<ResolutionChoice> for Resolution {
    fn from(choice: ResolutionChoice) -> Self {
        match choice {
            ResolutionChoice::Low => Resolution::Low,
            ResolutionChoice::Medium => Resolution::Medium,
            ResolutionChoice::High => Resolution::High,
            ResolutionChoice::Max => Resolution::Max,
        }
    }
}

#[derive(Params)]
pub struct ONrParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// The learned noise profile.
    #[persist = "profile"]
    pub profile: ProfileStore,

    #[id = "mode"]
    pub mode: EnumParam<ModeChoice>,
    /// Largest attenuation in dB.
    #[id = "reduction"]
    pub reduction_db: FloatParam,
    /// Offset of the noise estimate in dB.
    #[id = "threshold"]
    pub threshold_db: FloatParam,
    /// Release time of falling gains in ms.
    #[id = "release"]
    pub release_ms: FloatParam,
    /// Smoothing of the gains across frequency in percent.
    #[id = "smoothing"]
    pub smoothing: FloatParam,
    #[id = "resolution"]
    pub resolution: EnumParam<ResolutionChoice>,
    /// One gain for all channels.
    #[id = "link"]
    pub link: BoolParam,
    #[id = "listen"]
    pub listen_removed: BoolParam,
}

impl Default for ONrParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(EDITOR_WIDTH, EDITOR_HEIGHT),
            profile: ProfileStore::default(),

            mode: EnumParam::new("Noise Source", ModeChoice::Profile),
            reduction_db: FloatParam::new(
                "Reduction",
                18.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 60.0,
                },
            )
            .with_step_size(0.1)
            .with_unit(" dB"),
            threshold_db: FloatParam::new(
                "Threshold",
                3.0,
                FloatRange::Linear {
                    min: -12.0,
                    max: 24.0,
                },
            )
            .with_step_size(0.1)
            .with_unit(" dB"),
            release_ms: FloatParam::new(
                "Release",
                150.0,
                FloatRange::Skewed {
                    min: 10.0,
                    max: 1_000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_step_size(1.0)
            .with_unit(" ms"),
            smoothing: FloatParam::new(
                "Smoothing",
                30.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_step_size(1.0)
            .with_unit(" %"),
            resolution: EnumParam::new("Resolution", ResolutionChoice::Medium),
            link: BoolParam::new("Stereo Link", true),
            listen_removed: BoolParam::new("Listen Removed", false),
        }
    }
}
