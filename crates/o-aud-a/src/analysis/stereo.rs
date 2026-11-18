//! Stereo image measurements: phase correlation, balance and mid/side levels over time,
//! and correlation and panning per frequency band.

use std::f32::consts::FRAC_PI_4;

use crate::analysis::fft::{BandBins, BandMap, FftTap, power_to_db};

/// Time constant of the broadband stereo measurements in seconds.
const STEREO_TIME_S: f32 = 0.3;
/// Time constant of the per-band measurements in seconds.
const BAND_TIME_S: f32 = 0.25;

/// Third-octave bands from 20 Hz to 20 kHz.
pub const STEREO_BANDS: usize = 30;
pub const STEREO_LOW_HZ: f32 = 20.0;
pub const STEREO_HIGH_HZ: f32 = 20_000.0;

/// Equal-power pan position of two channel powers: −1 left, 0 centre, +1 right.
pub fn pan_position(left_power: f32, right_power: f32) -> f32 {
    if left_power + right_power <= 1e-20 {
        return 0.0;
    }
    right_power.sqrt().atan2(left_power.sqrt()) / FRAC_PI_4 - 1.0
}

/// Correlation of two channels from their powers and the real cross power.
pub fn correlation(left_power: f32, right_power: f32, cross: f32) -> f32 {
    let norm = (left_power * right_power).sqrt();
    if norm <= 1e-20 {
        0.0
    } else {
        (cross / norm).clamp(-1.0, 1.0)
    }
}

/// Broadband stereo readings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StereoReading {
    /// Phase correlation from −1 (opposite) to +1 (mono).
    pub correlation: f32,
    /// Right level minus left level in dB.
    pub balance_db: f32,
    /// Mid and side levels in dBFS (RMS, a full-scale sine reads −3 dB).
    pub mid_db: f32,
    pub side_db: f32,
}

/// Exponentially averaged channel powers, updated per frame.
#[derive(Clone, Debug)]
pub struct StereoMeter {
    coefficient: f32,
    left: f32,
    right: f32,
    cross: f32,
    mid: f32,
    side: f32,
}

impl StereoMeter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            coefficient: 1.0 - (-1.0 / (STEREO_TIME_S * sample_rate)).exp(),
            left: 0.0,
            right: 0.0,
            cross: 0.0,
            mid: 0.0,
            side: 0.0,
        }
    }

    pub fn process(&mut self, [left, right]: [f32; 2]) {
        let mid = 0.5 * (left + right);
        let side = 0.5 * (left - right);
        let k = self.coefficient;
        self.left += k * (left * left - self.left);
        self.right += k * (right * right - self.right);
        self.cross += k * (left * right - self.cross);
        self.mid += k * (mid * mid - self.mid);
        self.side += k * (side * side - self.side);
    }

    pub fn reading(&self) -> StereoReading {
        let balance_db = if self.left + self.right <= 1e-12 {
            0.0
        } else {
            (power_to_db(self.right) - power_to_db(self.left)).clamp(-40.0, 40.0)
        };
        StereoReading {
            correlation: correlation(self.left, self.right, self.cross),
            balance_db,
            mid_db: power_to_db(self.mid),
            side_db: power_to_db(self.side),
        }
    }
}

/// Correlation, pan position and level per third-octave band.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandStereo {
    pub correlation: [f32; STEREO_BANDS],
    pub pan: [f32; STEREO_BANDS],
    /// Band power in dB.
    pub level_db: [f32; STEREO_BANDS],
}

/// Averages band stereo measurements from successive FFTs.
#[derive(Clone, Debug, Default)]
pub struct BandStereoMeter {
    map: BandMap,
    left: [f32; STEREO_BANDS],
    right: [f32; STEREO_BANDS],
    cross: [f32; STEREO_BANDS],
}

impl BandStereoMeter {
    /// Prepares the bands for FFTs of `bins` bins of `bin_hz` Hz.
    pub fn configure(&mut self, bins: usize, bin_hz: f32) {
        self.map = BandMap::new(
            STEREO_BANDS,
            (
                STEREO_LOW_HZ,
                STEREO_HIGH_HZ.min(bin_hz * (bins - 1) as f32),
            ),
            true,
            bins,
            bin_hz,
        );
        self.left = [0.0; STEREO_BANDS];
        self.right = [0.0; STEREO_BANDS];
        self.cross = [0.0; STEREO_BANDS];
    }

    /// Adds an FFT taken `interval_s` seconds after the previous one.
    pub fn update(&mut self, tap: &FftTap, interval_s: f32) {
        let k = 1.0 - (-interval_s / BAND_TIME_S).exp();
        for (band, bins) in self.map.bands.iter().enumerate() {
            let (first, last) = match *bins {
                BandBins::Range(first, last) => (first, last),
                BandBins::Between(position) => {
                    let bin = position.round() as usize;
                    (bin, bin)
                }
            };
            let (left, right, cross) = (first..=last)
                .map(|bin| tap.stereo_power(bin))
                .fold((0.0, 0.0, 0.0), |(left, right, cross), (l, r, c)| {
                    (left + l, right + r, cross + c)
                });
            self.left[band] += k * (left - self.left[band]);
            self.right[band] += k * (right - self.right[band]);
            self.cross[band] += k * (cross - self.cross[band]);
        }
    }

    pub fn write(&self, output: &mut BandStereo) {
        for band in 0..STEREO_BANDS {
            let (left, right) = (self.left[band], self.right[band]);
            output.correlation[band] = correlation(left, right, self.cross[band]);
            output.pan[band] = pan_position(left, right);
            output.level_db[band] = power_to_db(0.5 * (left + right));
        }
    }
}

/// Centre frequency of a stereo band in Hz.
pub fn stereo_band_hz(band: usize) -> f32 {
    STEREO_LOW_HZ * (STEREO_HIGH_HZ / STEREO_LOW_HZ).powf((band as f32 + 0.5) / STEREO_BANDS as f32)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{StereoMeter, correlation, pan_position};

    #[test]
    fn pan_and_correlation_cover_their_range() {
        assert_eq!(pan_position(1.0, 0.0), -1.0);
        assert!((pan_position(1.0, 1.0)).abs() < 1e-6);
        assert!((pan_position(0.0, 1.0) - 1.0).abs() < 1e-6);
        assert_eq!(correlation(1.0, 1.0, -1.0), -1.0);
    }

    #[test]
    fn meter_reads_mono_wide_and_opposite_signals() {
        let rate = 48_000.0;
        for (sign, expected) in [(1.0, 1.0), (-1.0, -1.0)] {
            let mut meter = StereoMeter::new(rate);
            for n in 0..48_000 {
                let value = (TAU * 440.0 * n as f32 / rate).sin();
                meter.process([value, sign * value]);
            }
            let reading = meter.reading();
            assert!((reading.correlation - expected).abs() < 1e-3);
            assert!(reading.balance_db.abs() < 0.01);
        }
        let mut meter = StereoMeter::new(rate);
        for n in 0..48_000 {
            let t = n as f32 / rate;
            meter.process([(TAU * 440.0 * t).sin(), (TAU * 440.0 * t).cos()]);
        }
        assert!(
            meter.reading().correlation.abs() < 0.05,
            "quadrature is uncorrelated"
        );
    }
}
