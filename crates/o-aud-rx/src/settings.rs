//! Receiver settings and their ranges, shared by the plugin parameters and the
//! standalone application so both behave identically.

/// Target buffer latency range and default in milliseconds.
pub const LATENCY_MIN_MS: f32 = 2.0;
pub const LATENCY_MAX_MS: f32 = 250.0;
pub const LATENCY_DEFAULT_MS: f32 = 20.0;
/// Skew of the latency control: more travel for short latencies (NIH-plug skew units).
pub const LATENCY_SKEW: f32 = -1.5;

/// Output gain range in dB.
pub const GAIN_MIN_DB: f32 = -48.0;
pub const GAIN_MAX_DB: f32 = 12.0;

/// Settings read by the audio thread on every block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RxSettings {
    /// Output gain as a linear factor.
    pub gain: f32,
    pub mute: bool,
    /// Requested buffer latency in milliseconds. The receiver raises it automatically
    /// when the transmitter's or the device's blocks are too long for it.
    pub latency_ms: f32,
}

impl Default for RxSettings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            mute: false,
            latency_ms: LATENCY_DEFAULT_MS,
        }
    }
}

/// Converts decibels to a linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
