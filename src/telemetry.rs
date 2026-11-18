//! Metering data published from the audio thread to the editor.

use crate::dsp::analyzer::SPECTRUM_BANDS;
use crate::dsp::denoise::{ModelStatus, NoiseModel};
use crate::dsp::level::FLOOR_DB;

/// Number of 10 ms frames kept in the activity history (4 seconds).
pub const HISTORY_LEN: usize = 400;

/// Fixed-size snapshot of the processing state. It is copied into a triple buffer once
/// per frame, so it must not contain heap allocations.
#[derive(Clone, Debug)]
pub struct Telemetry {
    /// Time-aligned input spectrum in dBFS.
    pub spectrum_input: [f32; SPECTRUM_BANDS],
    /// Output spectrum in dBFS.
    pub spectrum_output: [f32; SPECTRUM_BANDS],
    /// Echo reference (sidechain) spectrum in dBFS.
    pub spectrum_reference: [f32; SPECTRUM_BANDS],

    /// Ring buffers of per-frame values; `history_head` is the oldest entry.
    pub voice_history: [f32; HISTORY_LEN],
    pub gate_history: [f32; HISTORY_LEN],
    /// Total gain of the speech leveler in dB.
    pub leveler_history_db: [f32; HISTORY_LEN],
    pub input_history_db: [f32; HISTORY_LEN],
    pub output_history_db: [f32; HISTORY_LEN],
    pub echo_history: [bool; HISTORY_LEN],
    pub history_head: usize,

    pub input_peak_db: f32,
    pub output_peak_db: f32,
    pub reference_peak_db: f32,

    pub voice_probability: f32,
    pub gate_open: bool,
    pub gate_gain_db: f32,
    /// Gain of the speech leveler's gain control in dB.
    pub leveler_gain_db: f32,
    /// Attenuation of the speech leveler's peak limiter in dB (zero or negative).
    pub limiter_db: f32,
    /// Estimated speech level in dBFS, or the floor before speech was heard.
    pub speech_level_db: f32,
    /// Smoothed level difference between output and time-aligned input (zero or negative).
    pub reduction_db: f32,

    pub erle_db: f32,
    pub echo_suppression_db: f32,
    pub echo_leakage: f32,
    pub echo_adapted: bool,
    pub reference_active: bool,
    pub double_talk: bool,

    pub noise_model: NoiseModel,
    pub noise_status: ModelStatus,

    pub host_rate: f32,
    pub latency_samples: u32,
    /// Incremented for every processed frame.
    pub sequence: u64,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            spectrum_input: [FLOOR_DB; SPECTRUM_BANDS],
            spectrum_output: [FLOOR_DB; SPECTRUM_BANDS],
            spectrum_reference: [FLOOR_DB; SPECTRUM_BANDS],
            voice_history: [0.0; HISTORY_LEN],
            gate_history: [1.0; HISTORY_LEN],
            leveler_history_db: [0.0; HISTORY_LEN],
            input_history_db: [FLOOR_DB; HISTORY_LEN],
            output_history_db: [FLOOR_DB; HISTORY_LEN],
            echo_history: [false; HISTORY_LEN],
            history_head: 0,
            input_peak_db: FLOOR_DB,
            output_peak_db: FLOOR_DB,
            reference_peak_db: FLOOR_DB,
            voice_probability: 0.0,
            gate_open: true,
            gate_gain_db: 0.0,
            leveler_gain_db: 0.0,
            limiter_db: 0.0,
            speech_level_db: FLOOR_DB,
            reduction_db: 0.0,
            erle_db: 0.0,
            echo_suppression_db: 0.0,
            echo_leakage: 0.0,
            echo_adapted: false,
            reference_active: false,
            double_talk: false,
            noise_model: NoiseModel::RnNoise,
            noise_status: ModelStatus::Running,
            host_rate: 0.0,
            latency_samples: 0,
            sequence: 0,
        }
    }
}

impl Telemetry {
    /// Iterates a history ring from the oldest to the newest entry.
    pub fn chronological<T: Copy>(&self, ring: &[T; HISTORY_LEN]) -> impl Iterator<Item = T> {
        let (newer, older) = ring.split_at(self.history_head);
        older.iter().chain(newer).copied()
    }
}
