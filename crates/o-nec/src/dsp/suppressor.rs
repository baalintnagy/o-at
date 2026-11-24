//! Residual echo suppressor.
//!
//! A linear echo canceller leaves residual echo from loudspeaker non-linearities, echo
//! path changes and tails beyond the modeled length. This stage estimates the residual
//! echo spectrum from the canceller's echo estimate and leakage and attenuates it with
//! a spectral Wiener-style gain, using a sqrt-Hann STFT with 50% overlap.
//!
//! The stage always adds exactly [`SUPPRESSOR_DELAY`] samples of latency, also when it
//! is idle, so that the reported plugin latency stays constant.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::dsp::FRAME;
use crate::dsp::level::{POWER_EPSILON, db_to_gain, periodic_hann};

const HOP: usize = FRAME;
const WINDOW: usize = 2 * HOP;
const BINS: usize = HOP + 1;

/// Latency of the suppressor in samples.
pub const SUPPRESSOR_DELAY: usize = HOP;

/// Lowest gain applied to a bin dominated by residual echo.
const GAIN_FLOOR_DB: f32 = -45.0;

/// Decay of the residual echo estimate per frame, modeling reverberation beyond the
/// linear filter.
const ECHO_DECAY: f32 = 0.55;

/// Gain smoothing when the gain rises (echo stops) and falls (echo starts).
const GAIN_RISE: f32 = 0.5;
const GAIN_FALL: f32 = 0.9;

pub struct EchoSuppressor {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    scratch_forward: Vec<Complex32>,
    scratch_inverse: Vec<Complex32>,
    window: Vec<f32>,
    signal_history: Vec<f32>,
    echo_history: Vec<f32>,
    time: Vec<f32>,
    signal_spectrum: Vec<Complex32>,
    echo_spectrum: Vec<Complex32>,
    residual: Vec<f32>,
    gains: Vec<f32>,
    overlap: Vec<f32>,
    suppression_db: f32,
}

impl Default for EchoSuppressor {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoSuppressor {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(WINDOW);
        let inverse = planner.plan_fft_inverse(WINDOW);
        let scratch_forward = forward.make_scratch_vec();
        let scratch_inverse = inverse.make_scratch_vec();
        let window = periodic_hann(WINDOW).into_iter().map(f32::sqrt).collect();
        Self {
            forward,
            inverse,
            scratch_forward,
            scratch_inverse,
            window,
            signal_history: vec![0.0; HOP],
            echo_history: vec![0.0; HOP],
            time: vec![0.0; WINDOW],
            signal_spectrum: vec![Complex32::default(); BINS],
            echo_spectrum: vec![Complex32::default(); BINS],
            residual: vec![0.0; BINS],
            gains: vec![1.0; BINS],
            overlap: vec![0.0; HOP],
            suppression_db: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.signal_history.fill(0.0);
        self.echo_history.fill(0.0);
        self.residual.fill(0.0);
        self.gains.fill(1.0);
        self.overlap.fill(0.0);
        self.suppression_db = 0.0;
    }

    /// Average attenuation applied in the last frame, in dB (zero or negative).
    pub fn suppression_db(&self) -> f32 {
        self.suppression_db
    }

    /// Processes one frame in place.
    ///
    /// `echo` is the canceller's echo estimate for the frame, `leakage` its residual
    /// estimate, and `strength` scales the suppression from 0 (off) to 1 (aggressive).
    pub fn process(&mut self, frame: &mut [f32], echo: &[f32], leakage: f32, strength: f32) {
        debug_assert_eq!(frame.len(), HOP);
        analyze(
            self.forward.as_ref(),
            &self.window,
            &mut self.signal_history,
            frame,
            &mut self.time,
            &mut self.signal_spectrum,
            &mut self.scratch_forward,
        );
        analyze(
            self.forward.as_ref(),
            &self.window,
            &mut self.echo_history,
            echo,
            &mut self.time,
            &mut self.echo_spectrum,
            &mut self.scratch_forward,
        );

        // Overestimate the residual for stronger settings; even a perfect linear filter
        // leaves non-linear echo that correlates with the estimate.
        let overdrive = strength * strength * 6.0 + strength * 2.0;
        let residual_scale = (leakage + 0.02 * strength) * overdrive;
        let floor = db_to_gain(GAIN_FLOOR_DB * strength);
        let mut gain_sum = 0.0;
        for bin in 0..BINS {
            let echo_power = self.echo_spectrum[bin].norm_sqr() * residual_scale;
            self.residual[bin] = echo_power.max(self.residual[bin] * ECHO_DECAY);
            let signal_power = self.signal_spectrum[bin].norm_sqr();
            let target = (1.0 - self.residual[bin] / (signal_power + POWER_EPSILON)).max(floor);
            let smoothing = if target > self.gains[bin] {
                GAIN_RISE
            } else {
                GAIN_FALL
            };
            self.gains[bin] = target + smoothing * (self.gains[bin] - target);
            if strength <= 0.0 {
                self.gains[bin] = 1.0;
            }
            self.signal_spectrum[bin] *= self.gains[bin];
            gain_sum += self.gains[bin];
        }
        self.suppression_db = 20.0 * (gain_sum / BINS as f32).max(1.0e-6).log10();

        self.signal_spectrum[0].im = 0.0;
        self.signal_spectrum[BINS - 1].im = 0.0;
        let _ = self.inverse.process_with_scratch(
            &mut self.signal_spectrum,
            &mut self.time,
            &mut self.scratch_inverse,
        );
        let scale = 1.0 / WINDOW as f32;
        for index in 0..HOP {
            frame[index] = self.overlap[index] + self.time[index] * self.window[index] * scale;
            self.overlap[index] = self.time[index + HOP] * self.window[index + HOP] * scale;
        }
    }
}

fn analyze(
    plan: &dyn RealToComplex<f32>,
    window: &[f32],
    history: &mut [f32],
    frame: &[f32],
    time: &mut [f32],
    spectrum: &mut [Complex32],
    scratch: &mut [Complex32],
) {
    for index in 0..HOP {
        time[index] = history[index] * window[index];
        time[index + HOP] = frame[index] * window[index + HOP];
    }
    history.copy_from_slice(frame);
    // Buffer lengths are fixed at construction, so the transform cannot fail.
    let _ = plan.process_with_scratch(time, spectrum, scratch);
}

#[cfg(test)]
mod tests {
    use super::{EchoSuppressor, HOP, SUPPRESSOR_DELAY};
    use crate::dsp::level::{energy, power_to_db};

    #[test]
    fn idle_suppressor_is_a_pure_delay() {
        let mut suppressor = EchoSuppressor::new();
        let input: Vec<f32> = (0..HOP * 20)
            .map(|n| (n as f32 * 0.05).sin() * 0.5)
            .collect();
        let silence = vec![0.0; HOP];
        let mut output = input.clone();
        for frame in output.chunks_mut(HOP) {
            suppressor.process(frame, &silence, 0.0, 1.0);
        }
        for index in SUPPRESSOR_DELAY + HOP..input.len() {
            let expected = input[index - SUPPRESSOR_DELAY];
            assert!((output[index] - expected).abs() < 1.0e-4, "sample {index}");
        }
    }

    #[test]
    fn attenuates_signal_matching_the_echo_estimate() {
        let mut suppressor = EchoSuppressor::new();
        let input: Vec<f32> = (0..HOP * 50)
            .map(|n| (n as f32 * 0.11).sin() * 0.3)
            .collect();
        let mut output = input.clone();
        for (frame, echo) in output.chunks_mut(HOP).zip(input.chunks(HOP)) {
            suppressor.process(frame, echo, 0.3, 0.6);
        }
        let tail = HOP * 30..;
        let attenuation = power_to_db(energy(&output[tail.clone()]) / energy(&input[tail]));
        assert!(attenuation < -20.0, "attenuation {attenuation:.1} dB");
    }
}
