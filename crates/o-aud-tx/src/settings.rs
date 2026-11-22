//! Transmitter settings and their ranges, shared by the plugin parameters and the
//! standalone application so both behave identically.

/// Send gain range in dB.
pub const GAIN_MIN_DB: f32 = -48.0;
pub const GAIN_MAX_DB: f32 = 12.0;

/// Converts decibels to a linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
