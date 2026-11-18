//! Host-visible parameters. Parameter IDs are part of saved sessions; never rename them.

use std::sync::Arc;

use nih_plug::prelude::{
    BoolParam, Enum, EnumParam, FloatParam, FloatRange, IntParam, IntRange, Params, formatters,
};
use nih_plug_egui::EguiState;

use crate::dsp::aec::{MAX_TAIL_MS, MIN_TAIL_MS};
use crate::dsp::denoise::{FULL_REDUCTION_DB, NoiseModel};
use crate::dsp::gate::GateTrigger;
use crate::dsp::leveler::LevelerSpeed;

/// Logical editor size in pixels.
pub const EDITOR_WIDTH: u32 = 1200;
pub const EDITOR_HEIGHT: u32 = 680;

#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateMode {
    #[name = "Off"]
    Off,
    #[name = "Voice"]
    Voice,
    #[name = "Level"]
    Level,
}

/// Noise suppression model choice.
#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseModelChoice {
    /// RNNoise: light on CPU, 10 ms of delay.
    #[name = "RNNoise"]
    RnNoise,
    /// DeepFilterNet3: stronger on non-stationary noise, more delay and CPU.
    #[name = "DeepFilter"]
    DeepFilterNet,
}

/// How quickly the speech leveler follows the speaker's level.
#[derive(Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelerSpeedChoice {
    #[name = "Slow"]
    Slow,
    #[name = "Medium"]
    Medium,
    #[name = "Fast"]
    Fast,
}

impl From<LevelerSpeedChoice> for LevelerSpeed {
    fn from(choice: LevelerSpeedChoice) -> Self {
        match choice {
            LevelerSpeedChoice::Slow => LevelerSpeed::Slow,
            LevelerSpeedChoice::Medium => LevelerSpeed::Medium,
            LevelerSpeedChoice::Fast => LevelerSpeed::Fast,
        }
    }
}

impl From<NoiseModelChoice> for NoiseModel {
    fn from(choice: NoiseModelChoice) -> Self {
        match choice {
            NoiseModelChoice::RnNoise => NoiseModel::RnNoise,
            NoiseModelChoice::DeepFilterNet => NoiseModel::DeepFilterNet,
        }
    }
}

impl From<GateMode> for GateTrigger {
    fn from(mode: GateMode) -> Self {
        match mode {
            GateMode::Off => GateTrigger::Off,
            GateMode::Voice => GateTrigger::Voice,
            GateMode::Level => GateTrigger::Level,
        }
    }
}

#[derive(Params)]
pub struct ONecParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[id = "in_gain"]
    pub input_gain_db: FloatParam,
    #[id = "hpf_on"]
    pub high_pass_enabled: BoolParam,
    #[id = "hpf_freq"]
    pub high_pass_hz: FloatParam,

    #[id = "ns_on"]
    pub denoise_enabled: BoolParam,
    #[id = "ns_amount"]
    pub reduction_db: FloatParam,
    #[id = "ns_model"]
    pub noise_model: EnumParam<NoiseModelChoice>,

    #[id = "aec_on"]
    pub echo_enabled: BoolParam,
    #[id = "aec_tail"]
    pub echo_tail_ms: IntParam,
    #[id = "aec_res"]
    pub echo_suppression: FloatParam,

    #[id = "gate_mode"]
    pub gate_mode: EnumParam<GateMode>,
    #[id = "gate_vad"]
    pub gate_voice_threshold: FloatParam,
    #[id = "gate_level"]
    pub gate_level_db: FloatParam,
    #[id = "gate_range"]
    pub gate_range_db: FloatParam,
    #[id = "gate_hold"]
    pub gate_hold_ms: FloatParam,
    #[id = "gate_release"]
    pub gate_release_ms: FloatParam,

    #[id = "lev_on"]
    pub leveler_enabled: BoolParam,
    /// Speech level the leveler aims for, in dBFS.
    #[id = "lev_target"]
    pub leveler_target_db: FloatParam,
    #[id = "lev_amount"]
    pub leveler_amount: FloatParam,
    #[id = "lev_boost"]
    pub leveler_max_boost_db: FloatParam,
    #[id = "lev_cut"]
    pub leveler_max_cut_db: FloatParam,
    #[id = "lev_speed"]
    pub leveler_speed: EnumParam<LevelerSpeedChoice>,

    #[id = "out_gain"]
    pub output_gain_db: FloatParam,
    #[id = "listen"]
    pub listen_removed: BoolParam,
}

impl Default for ONecParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(EDITOR_WIDTH, EDITOR_HEIGHT),

            input_gain_db: gain_param("Input Gain"),
            high_pass_enabled: BoolParam::new("High-Pass", true),
            high_pass_hz: FloatParam::new(
                "High-Pass Frequency",
                80.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 400.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            denoise_enabled: BoolParam::new("Noise Suppression", true),
            reduction_db: FloatParam::new(
                "Noise Reduction",
                FULL_REDUCTION_DB,
                FloatRange::Linear {
                    min: 0.0,
                    max: FULL_REDUCTION_DB,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(Arc::new(|value| {
                if value >= FULL_REDUCTION_DB {
                    String::from("Max")
                } else {
                    format!("{value:.0}")
                }
            })),
            noise_model: EnumParam::new("Noise Model", NoiseModelChoice::RnNoise),

            echo_enabled: BoolParam::new("Echo Cancellation", true),
            echo_tail_ms: IntParam::new(
                "Echo Tail",
                250,
                IntRange::Linear {
                    min: MIN_TAIL_MS as i32,
                    max: MAX_TAIL_MS as i32,
                },
            )
            .with_unit(" ms"),
            echo_suppression: percent_param("Echo Suppression", 50.0),

            gate_mode: EnumParam::new("Gate", GateMode::Voice),
            gate_voice_threshold: percent_param("Voice Threshold", 50.0),
            gate_level_db: FloatParam::new(
                "Level Threshold",
                -50.0,
                FloatRange::Linear {
                    min: -80.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            gate_range_db: FloatParam::new(
                "Gate Range",
                40.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 80.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            gate_hold_ms: time_param("Gate Hold", 250.0, 0.0, 1000.0),
            gate_release_ms: time_param("Gate Release", 150.0, 10.0, 1000.0),

            leveler_enabled: BoolParam::new("Speech Leveler", false),
            leveler_target_db: FloatParam::new(
                "Leveler Target",
                -20.0,
                FloatRange::Linear {
                    min: -40.0,
                    max: -6.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            leveler_amount: percent_param("Leveler Amount", 80.0),
            leveler_max_boost_db: range_param("Leveler Max Boost", 12.0),
            leveler_max_cut_db: range_param("Leveler Max Cut", 12.0),
            leveler_speed: EnumParam::new("Leveler Speed", LevelerSpeedChoice::Medium),

            output_gain_db: gain_param("Output Gain"),
            listen_removed: BoolParam::new("Listen Removed", false),
        }
    }
}

fn gain_param(name: &str) -> FloatParam {
    FloatParam::new(
        name,
        0.0,
        FloatRange::Linear {
            min: -24.0,
            max: 24.0,
        },
    )
    .with_unit(" dB")
    .with_value_to_string(formatters::v2s_f32_rounded(1))
}

/// A gain limit of 0–30 dB.
fn range_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Linear {
            min: 0.0,
            max: 30.0,
        },
    )
    .with_unit(" dB")
    .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn percent_param(name: &str, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Linear {
            min: 0.0,
            max: 100.0,
        },
    )
    .with_unit(" %")
    .with_value_to_string(formatters::v2s_f32_rounded(0))
}

fn time_param(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-1.0),
        },
    )
    .with_unit(" ms")
    .with_value_to_string(formatters::v2s_f32_rounded(0))
}
