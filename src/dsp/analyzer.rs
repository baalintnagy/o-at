//! Spectrum analyzer feeding the editor display.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

use crate::dsp::level::{FLOOR_DB, periodic_hann, power_to_db};
use crate::dsp::{FRAME, INTERNAL_RATE};

/// Number of logarithmically spaced display bands.
pub const SPECTRUM_BANDS: usize = 160;

/// Lowest and highest displayed frequencies in Hz.
pub const SPECTRUM_MIN_HZ: f32 = 20.0;
pub const SPECTRUM_MAX_HZ: f32 = 20_000.0;

const FFT_LEN: usize = 2048;
const BINS: usize = FFT_LEN / 2 + 1;

/// Fall rate of the displayed spectrum in dB per 10 ms frame.
const RELEASE_DB: f32 = 1.2;

/// Fraction of a rise applied per frame.
const ATTACK: f32 = 0.6;

/// Centre frequency of a display band.
pub fn band_frequency(band: usize) -> f32 {
    let position = band as f32 / (SPECTRUM_BANDS - 1) as f32;
    SPECTRUM_MIN_HZ * (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).powf(position)
}

/// One analyzed signal with its own history and smoothed display bands.
pub struct SpectrumTap {
    history: Vec<f32>,
    bands: [f32; SPECTRUM_BANDS],
}

impl Default for SpectrumTap {
    fn default() -> Self {
        Self {
            history: vec![0.0; FFT_LEN],
            bands: [FLOOR_DB; SPECTRUM_BANDS],
        }
    }
}

impl SpectrumTap {
    pub fn bands(&self) -> &[f32; SPECTRUM_BANDS] {
        &self.bands
    }

    fn push(&mut self, frame: &[f32]) {
        self.history.copy_within(frame.len().., 0);
        let start = self.history.len() - frame.len();
        self.history[start..].copy_from_slice(frame);
    }
}

/// Shared FFT state; taps keep only their sample history.
pub struct Analyzer {
    forward: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<Complex32>,
    window: Vec<f32>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    power_db: Vec<f32>,
    band_bins: Vec<(f32, usize, usize)>,
    normalization: f32,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FFT_LEN);
        let scratch = forward.make_scratch_vec();
        let window = periodic_hann(FFT_LEN);
        let window_sum: f32 = window.iter().sum();
        let bin_hz = INTERNAL_RATE as f32 / FFT_LEN as f32;
        let band_bins = (0..SPECTRUM_BANDS)
            .map(|band| {
                let centre = band_frequency(band);
                let ratio = (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).powf(0.5 / SPECTRUM_BANDS as f32);
                let low = ((centre / ratio) / bin_hz).floor() as usize;
                let high = ((centre * ratio) / bin_hz).ceil() as usize;
                (
                    centre / bin_hz,
                    low.min(BINS - 1),
                    high.clamp(low + 1, BINS - 1),
                )
            })
            .collect();
        Self {
            forward,
            scratch,
            window,
            time: vec![0.0; FFT_LEN],
            spectrum: vec![Complex32::default(); BINS],
            power_db: vec![FLOOR_DB; BINS],
            band_bins,
            // A full-scale sine reads 0 dBFS.
            normalization: 4.0 / (window_sum * window_sum),
        }
    }

    /// Appends a frame to the tap's history and refreshes its display bands.
    pub fn analyze(&mut self, tap: &mut SpectrumTap, frame: &[f32]) {
        debug_assert_eq!(frame.len(), FRAME);
        tap.push(frame);
        for ((time, sample), weight) in self.time.iter_mut().zip(&tap.history).zip(&self.window) {
            *time = sample * weight;
        }
        // Buffer lengths are fixed at construction, so the transform cannot fail.
        let _ = self.forward.process_with_scratch(
            &mut self.time,
            &mut self.spectrum,
            &mut self.scratch,
        );
        for (power, bin) in self.power_db.iter_mut().zip(&self.spectrum) {
            *power = power_to_db(bin.norm_sqr() * self.normalization);
        }
        for (band, &(centre, low, high)) in self.band_bins.iter().enumerate() {
            let value = if high - low <= 2 {
                // Narrow bands at low frequencies: interpolate between neighbouring bins.
                let index = (centre.floor() as usize).min(BINS - 2);
                let fraction = (centre - index as f32).clamp(0.0, 1.0);
                self.power_db[index] * (1.0 - fraction) + self.power_db[index + 1] * fraction
            } else {
                self.power_db[low..=high]
                    .iter()
                    .fold(FLOOR_DB, |current, value| current.max(*value))
            };
            let shown = &mut tap.bands[band];
            *shown = if value > *shown {
                *shown + ATTACK * (value - *shown)
            } else {
                (*shown - RELEASE_DB).max(value)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{Analyzer, SPECTRUM_BANDS, SpectrumTap, band_frequency};
    use crate::dsp::FRAME;

    #[test]
    fn full_scale_sine_reads_zero_dbfs_at_its_band() {
        let mut analyzer = Analyzer::new();
        let mut tap = SpectrumTap::default();
        let frequency = 1_000.0;
        for frame in 0..40 {
            let samples: Vec<f32> = (0..FRAME)
                .map(|n| {
                    let t = (frame * FRAME + n) as f32 / 48_000.0;
                    (TAU * frequency * t).sin()
                })
                .collect();
            analyzer.analyze(&mut tap, &samples);
        }
        let band = (0..SPECTRUM_BANDS)
            .min_by(|a, b| {
                let da = (band_frequency(*a) - frequency).abs();
                let db = (band_frequency(*b) - frequency).abs();
                da.total_cmp(&db)
            })
            .unwrap_or_default();
        let level = tap.bands()[band];
        assert!(
            level > -1.5 && level < 0.5,
            "1 kHz band level {level:.2} dB"
        );
        assert!(tap.bands()[10] < -60.0, "low band should be quiet");
    }
}
