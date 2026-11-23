//! Metering data published from the audio thread to the editor.

use crate::dsp::level::FLOOR_DB;

/// Number of logarithmically spaced spectrum display bands.
pub const SPECTRUM_BANDS: usize = 192;

/// Lowest displayed frequency in Hz.
pub const SPECTRUM_LOW_HZ: f32 = 20.0;

/// Highest displayed frequency in Hz (limited to the Nyquist frequency).
pub const SPECTRUM_HIGH_HZ: f32 = 24_000.0;

/// Centre frequency of a display band in Hz.
pub fn band_hz(band: f32) -> f32 {
    SPECTRUM_LOW_HZ * (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).powf(band / SPECTRUM_BANDS as f32)
}

/// Where the noise estimate in use comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoiseSource {
    /// The stored noise profile.
    #[default]
    Profile,
    /// The continuous noise floor tracker.
    Adaptive,
    /// Profile mode without a stored profile: the tracker stands in.
    NoProfile,
    /// A profile is being learned; the signal passes unchanged.
    Learning,
}

/// Fixed-size snapshot of the processing state, copied into a triple buffer once per
/// frame, so it must not contain heap allocations.
#[derive(Clone, Debug)]
pub struct Telemetry {
    /// Input spectrum in dB, calibrated so a full-scale sine reads 0 dB.
    pub input_db: [f32; SPECTRUM_BANDS],
    /// Output spectrum in dB.
    pub output_db: [f32; SPECTRUM_BANDS],
    /// Noise estimate in use, threshold included, in dB.
    pub noise_db: [f32; SPECTRUM_BANDS],
    /// Number of bands with data: bands above the Nyquist frequency are unused.
    pub bands: usize,

    pub input_peak_db: f32,
    pub output_peak_db: f32,
    /// Smoothed level difference between output and input (zero or negative), in dB.
    pub reduction_db: f32,

    pub source: NoiseSource,
    /// Length of the passage learned so far, in seconds.
    pub learned_s: f32,
    pub has_profile: bool,

    pub host_rate: f32,
    pub fft_size: u32,
    pub latency_samples: u32,
    /// Incremented for every processed frame.
    pub sequence: u64,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            input_db: [FLOOR_DB; SPECTRUM_BANDS],
            output_db: [FLOOR_DB; SPECTRUM_BANDS],
            noise_db: [FLOOR_DB; SPECTRUM_BANDS],
            bands: 0,
            input_peak_db: FLOOR_DB,
            output_peak_db: FLOOR_DB,
            reduction_db: 0.0,
            source: NoiseSource::default(),
            learned_s: 0.0,
            has_profile: false,
            host_rate: 0.0,
            fft_size: 0,
            latency_samples: 0,
            sequence: 0,
        }
    }
}
