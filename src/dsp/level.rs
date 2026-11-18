//! Level conversion and small signal helpers shared by the DSP stages.

use std::f32::consts::TAU;

/// Lowest level reported anywhere in the plugin, in dBFS.
pub const FLOOR_DB: f32 = -120.0;

/// Smallest power value used to avoid divisions by zero and `log10(0)`.
pub const POWER_EPSILON: f32 = 1.0e-12;

pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub fn gain_to_db(gain: f32) -> f32 {
    (20.0 * gain.abs().max(1.0e-6).log10()).max(FLOOR_DB)
}

pub fn power_to_db(power: f32) -> f32 {
    (10.0 * power.max(POWER_EPSILON).log10()).max(FLOOR_DB)
}

/// Sum of squared samples.
pub fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|sample| sample * sample).sum()
}

/// Mean squared sample value.
pub fn mean_power(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    energy(samples) / samples.len() as f32
}

/// Largest absolute sample value.
pub fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0, |current, sample| current.max(sample.abs()))
}

/// Coefficient of a one-pole smoother reaching ~63% of a step after `time_ms`, when
/// updated `updates_per_second` times per second.
pub fn one_pole_coefficient(time_ms: f32, updates_per_second: f32) -> f32 {
    if time_ms <= 0.0 {
        return 0.0;
    }
    (-1000.0 / (time_ms * updates_per_second)).exp()
}

/// Periodic Hann window, which sums to one when overlapped by half its length.
pub fn periodic_hann(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| {
            let phase = TAU * index as f32 / length as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{db_to_gain, gain_to_db, periodic_hann, power_to_db};

    #[test]
    fn decibel_round_trip() {
        for db in [-60.0, -6.0, 0.0, 12.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1.0e-3);
        }
        assert!((power_to_db(0.5) - -3.0103).abs() < 1.0e-3);
    }

    #[test]
    fn periodic_hann_overlaps_to_unity() {
        let window = periodic_hann(960);
        for index in 0..480 {
            let sum = window[index] + window[index + 480];
            assert!((sum - 1.0).abs() < 1.0e-5);
        }
    }
}
