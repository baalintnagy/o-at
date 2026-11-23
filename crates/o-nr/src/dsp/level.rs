//! Level conversions.

/// Lowest level reported, in dB.
pub const FLOOR_DB: f32 = -140.0;

/// Power ratio of a level in dB.
pub fn db_to_power(db: f32) -> f32 {
    10.0_f32.powf(0.1 * db)
}

/// Level in dB of a power ratio, limited to [`FLOOR_DB`].
pub fn power_to_db(power: f32) -> f32 {
    (10.0 * power.max(1e-30).log10()).max(FLOOR_DB)
}

/// Amplitude ratio of a level in dB.
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(0.05 * db)
}

/// Level in dB of an amplitude ratio, limited to [`FLOOR_DB`].
pub fn gain_to_db(gain: f32) -> f32 {
    (20.0 * gain.abs().max(1e-30).log10()).max(FLOOR_DB)
}
