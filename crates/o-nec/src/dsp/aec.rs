//! Acoustic echo canceller.
//!
//! This is a partitioned-block frequency-domain adaptive filter (PBFDAF) using
//! overlap-save convolution with a gradient constraint, following the multidelay block
//! frequency-domain design (MDF) used by Speex and described by Valin in "On adjusting
//! the learning rate in frequency domain echo cancellation with double-talk" (2007):
//!
//! - The step size of each frequency bin is derived from an estimate of the residual
//!   echo, obtained from the correlation between the error and echo power spectra
//!   ("leakage"). This keeps adaptation fast during single talk and slows it down
//!   automatically during double talk, without a separate hard double-talk switch.
//! - A background filter adapts continuously, while the foreground filter that produces
//!   the output is only replaced when the background filter performs clearly better.
//!   A diverged background filter is restored from the foreground.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::dsp::FRAME;
use crate::dsp::level::{POWER_EPSILON, energy, power_to_db};

const BLOCK: usize = FRAME;
const FFT_LEN: usize = 2 * BLOCK;
const BINS: usize = BLOCK + 1;

/// Longest supported echo tail in milliseconds.
pub const MAX_TAIL_MS: u32 = 500;

/// Shortest supported echo tail in milliseconds.
pub const MIN_TAIL_MS: u32 = 50;

const MAX_PARTITIONS: usize = (MAX_TAIL_MS as usize * 48) / BLOCK;

/// Reference frames adapted with a fixed step size before switching to the
/// leakage-controlled step size.
const BOOTSTRAP_FRAMES: u32 = 120;

/// Fixed normalized step size during bootstrap.
const MU_BOOTSTRAP: f32 = 0.4;

/// Upper bound of the leakage-controlled normalized step size.
const MU_MAX: f32 = 0.5;

/// Mean reference power below which the reference is considered silent (about -70 dBFS).
const REFERENCE_SILENCE_POWER: f32 = 1.0e-7;

/// Per-bin smoothing of the reference power used for step normalization.
const REFERENCE_POWER_SMOOTHING: f32 = 0.3;

/// Smoothing of the per-bin error and echo power means used by the leakage estimator.
const SPECTRAL_MEAN_SMOOTHING: f32 = 0.1;

/// Base smoothing of the leakage correlation terms per frame.
const LEAKAGE_SMOOTHING: f32 = 0.02;

/// Smoothing of the energies used for ERLE reporting.
const ERLE_SMOOTHING: f32 = 0.1;

/// Diagnostic state of the echo canceller for metering.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EchoStats {
    /// Echo return loss enhancement (mic power over output power) in dB.
    pub erle_db: f32,
    /// Estimated fraction of the echo estimate power left in the output.
    pub leakage: f32,
    /// Whether the filter has passed its bootstrap phase.
    pub adapted: bool,
    /// Whether the reference carried signal in this frame.
    pub reference_active: bool,
    /// Whether near-end speech overlaps echo in this frame.
    pub double_talk: bool,
}

pub struct EchoCanceller {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    scratch_forward: Vec<Complex32>,
    scratch_inverse: Vec<Complex32>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    accumulator: Vec<Complex32>,
    previous_reference: Vec<f32>,
    reference_history: Vec<Vec<Complex32>>,
    newest: usize,
    foreground: Vec<Vec<Complex32>>,
    background: Vec<Vec<Complex32>>,
    partitions: usize,
    reference_power: Vec<f32>,
    step: Vec<f32>,
    error_power: Vec<f32>,
    echo_power: Vec<f32>,
    error_mean: Vec<f32>,
    echo_mean: Vec<f32>,
    correlation: f32,
    echo_variance: f32,
    echo_foreground: Vec<f32>,
    error_foreground: Vec<f32>,
    echo_background: Vec<f32>,
    error_background: Vec<f32>,
    active_frames: u32,
    better_frames: u32,
    diverged_frames: u32,
    mic_energy: f32,
    residual_energy: f32,
    stats: EchoStats,
}

impl Default for EchoCanceller {
    fn default() -> Self {
        Self::new()
    }
}

impl EchoCanceller {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FFT_LEN);
        let inverse = planner.plan_fft_inverse(FFT_LEN);
        let scratch_forward = forward.make_scratch_vec();
        let scratch_inverse = inverse.make_scratch_vec();
        let spectra = || vec![vec![Complex32::default(); BINS]; MAX_PARTITIONS];
        Self {
            forward,
            inverse,
            scratch_forward,
            scratch_inverse,
            time: vec![0.0; FFT_LEN],
            spectrum: vec![Complex32::default(); BINS],
            accumulator: vec![Complex32::default(); BINS],
            previous_reference: vec![0.0; BLOCK],
            reference_history: spectra(),
            newest: 0,
            foreground: spectra(),
            background: spectra(),
            partitions: tail_to_partitions(250),
            reference_power: vec![0.0; BINS],
            step: vec![0.0; BINS],
            error_power: vec![0.0; BINS],
            echo_power: vec![0.0; BINS],
            error_mean: vec![0.0; BINS],
            echo_mean: vec![0.0; BINS],
            correlation: 0.0,
            echo_variance: 1.0,
            echo_foreground: vec![0.0; BLOCK],
            error_foreground: vec![0.0; BLOCK],
            echo_background: vec![0.0; BLOCK],
            error_background: vec![0.0; BLOCK],
            active_frames: 0,
            better_frames: 0,
            diverged_frames: 0,
            mic_energy: 0.0,
            residual_energy: 0.0,
            stats: EchoStats::default(),
        }
    }

    /// Clears the learned echo path and all history.
    pub fn reset(&mut self) {
        for spectra in [
            &mut self.reference_history,
            &mut self.foreground,
            &mut self.background,
        ] {
            spectra
                .iter_mut()
                .for_each(|partition| partition.fill(Complex32::default()));
        }
        self.previous_reference.fill(0.0);
        self.reference_power.fill(0.0);
        self.error_mean.fill(0.0);
        self.echo_mean.fill(0.0);
        self.correlation = 0.0;
        self.echo_variance = 1.0;
        self.active_frames = 0;
        self.better_frames = 0;
        self.diverged_frames = 0;
        self.mic_energy = 0.0;
        self.residual_energy = 0.0;
        self.stats = EchoStats::default();
    }

    /// Sets the modeled echo tail. Shortening the tail discards the dropped partitions.
    pub fn set_tail_ms(&mut self, tail_ms: u32) {
        let partitions = tail_to_partitions(tail_ms);
        if partitions < self.partitions {
            for weights in [&mut self.foreground, &mut self.background] {
                weights[partitions..self.partitions]
                    .iter_mut()
                    .for_each(|partition| partition.fill(Complex32::default()));
            }
        }
        self.partitions = partitions;
    }

    pub fn stats(&self) -> EchoStats {
        self.stats
    }

    /// Removes the estimated echo of `reference` from one frame of `mic`.
    ///
    /// Writes the echo-cancelled signal to `output` and the echo estimate that was
    /// subtracted to `echo`, which drives the residual echo suppressor.
    pub fn process(
        &mut self,
        mic: &[f32],
        reference: &[f32],
        output: &mut [f32],
        echo: &mut [f32],
    ) {
        debug_assert_eq!(mic.len(), BLOCK);
        debug_assert_eq!(reference.len(), BLOCK);

        let reference_energy = energy(reference);
        let reference_active = reference_energy > REFERENCE_SILENCE_POWER * BLOCK as f32;
        self.push_reference(reference);

        convolve(
            &self.foreground[..self.partitions],
            &self.reference_history,
            self.newest,
            &mut self.accumulator,
        );
        self.inverse_to_tail(OutputSelect::Foreground);
        convolve(
            &self.background[..self.partitions],
            &self.reference_history,
            self.newest,
            &mut self.accumulator,
        );
        self.inverse_to_tail(OutputSelect::Background);

        for index in 0..BLOCK {
            self.error_foreground[index] = mic[index] - self.echo_foreground[index];
            self.error_background[index] = mic[index] - self.echo_background[index];
        }

        let mic_energy = energy(mic);
        let foreground_error = energy(&self.error_foreground);
        let background_error = energy(&self.error_background);

        if reference_active {
            self.adapt_background();
        }

        let use_background = self.select_filter(mic_energy, foreground_error, background_error);
        let (error, estimate, output_energy) = if use_background {
            (
                &self.error_background,
                &self.echo_background,
                background_error,
            )
        } else {
            (
                &self.error_foreground,
                &self.echo_foreground,
                foreground_error,
            )
        };
        output.copy_from_slice(error);
        echo.copy_from_slice(estimate);

        self.update_stats(mic_energy, output_energy, energy(echo), reference_active);
    }

    fn push_reference(&mut self, reference: &[f32]) {
        self.newest = (self.newest + MAX_PARTITIONS - 1) % MAX_PARTITIONS;
        self.time[..BLOCK].copy_from_slice(&self.previous_reference);
        self.time[BLOCK..].copy_from_slice(reference);
        self.previous_reference.copy_from_slice(reference);
        forward_fft(
            self.forward.as_ref(),
            &mut self.time,
            &mut self.reference_history[self.newest],
            &mut self.scratch_forward,
        );
        let newest = &self.reference_history[self.newest];
        for (power, bin) in self.reference_power.iter_mut().zip(newest) {
            *power += REFERENCE_POWER_SMOOTHING * (bin.norm_sqr() - *power);
        }
    }

    /// Transforms the accumulated echo spectrum back and stores the valid half.
    fn inverse_to_tail(&mut self, select: OutputSelect) {
        inverse_fft(
            self.inverse.as_ref(),
            &mut self.accumulator,
            &mut self.time,
            &mut self.scratch_inverse,
        );
        let target = match select {
            OutputSelect::Foreground => &mut self.echo_foreground,
            OutputSelect::Background => &mut self.echo_background,
        };
        let scale = 1.0 / FFT_LEN as f32;
        for (estimate, sample) in target.iter_mut().zip(&self.time[BLOCK..]) {
            *estimate = sample * scale;
        }
    }

    fn adapt_background(&mut self) {
        self.active_frames = self.active_frames.saturating_add(1);

        // Error spectrum of the zero-padded background error.
        self.time[..BLOCK].fill(0.0);
        self.time[BLOCK..].copy_from_slice(&self.error_background);
        forward_fft(
            self.forward.as_ref(),
            &mut self.time,
            &mut self.spectrum,
            &mut self.scratch_forward,
        );
        for (power, bin) in self.error_power.iter_mut().zip(&self.spectrum) {
            *power = bin.norm_sqr();
        }

        // Echo estimate spectrum, zero-padded the same way, for the leakage estimate.
        self.time[..BLOCK].fill(0.0);
        self.time[BLOCK..].copy_from_slice(&self.echo_background);
        forward_fft(
            self.forward.as_ref(),
            &mut self.time,
            &mut self.accumulator,
            &mut self.scratch_forward,
        );
        for (power, bin) in self.echo_power.iter_mut().zip(&self.accumulator) {
            *power = bin.norm_sqr();
        }

        self.update_leakage();
        self.update_step();

        for partition in 0..self.partitions {
            let history = &self.reference_history[(self.newest + partition) % MAX_PARTITIONS];
            for bin in 0..BINS {
                self.accumulator[bin] = history[bin].conj() * self.spectrum[bin] * self.step[bin];
            }
            // Gradient constraint: keep only the causal first half of the correlation.
            inverse_fft(
                self.inverse.as_ref(),
                &mut self.accumulator,
                &mut self.time,
                &mut self.scratch_inverse,
            );
            let scale = 1.0 / FFT_LEN as f32;
            self.time[..BLOCK]
                .iter_mut()
                .for_each(|sample| *sample *= scale);
            self.time[BLOCK..].fill(0.0);
            forward_fft(
                self.forward.as_ref(),
                &mut self.time,
                &mut self.accumulator,
                &mut self.scratch_forward,
            );
            for (weight, update) in self.background[partition].iter_mut().zip(&self.accumulator) {
                *weight += update;
            }
        }
    }

    fn update_leakage(&mut self) {
        let mut correlation = 0.0;
        let mut variance = 0.0;
        let mut error_sum = 0.0;
        let mut echo_sum = 0.0;
        for bin in 0..BINS {
            let error = self.error_power[bin];
            let echo = self.echo_power[bin];
            self.error_mean[bin] += SPECTRAL_MEAN_SMOOTHING * (error - self.error_mean[bin]);
            self.echo_mean[bin] += SPECTRAL_MEAN_SMOOTHING * (echo - self.echo_mean[bin]);
            let error_deviation = error - self.error_mean[bin];
            let echo_deviation = echo - self.echo_mean[bin];
            correlation += error_deviation * echo_deviation;
            variance += echo_deviation * echo_deviation;
            error_sum += error;
            echo_sum += echo;
        }
        // Update faster when the echo dominates the error, as in Speex's MDF.
        let rate = LEAKAGE_SMOOTHING * (echo_sum / (error_sum + POWER_EPSILON)).clamp(0.05, 1.0);
        self.correlation += rate * (correlation - self.correlation);
        self.echo_variance += rate * (variance - self.echo_variance);
        let leakage = self.correlation.max(0.0) / (self.echo_variance + POWER_EPSILON);
        self.stats.leakage = leakage.clamp(0.0005, 1.0);
    }

    fn update_step(&mut self) {
        let bootstrap = self.active_frames <= BOOTSTRAP_FRAMES;
        let partitions = self.partitions as f32;
        let regularization = FFT_LEN as f32 * 1.0e-7;
        for bin in 0..BINS {
            let mu = if bootstrap {
                MU_BOOTSTRAP
            } else {
                let residual = self.stats.leakage * self.echo_power[bin];
                (residual / (self.error_power[bin] + POWER_EPSILON)).min(MU_MAX)
            };
            self.step[bin] = mu / (partitions * self.reference_power[bin] + regularization);
        }
    }

    /// Decides which filter produces the output and synchronizes the two filters.
    /// Returns whether the background filter output should be used for this frame.
    fn select_filter(&mut self, mic: f32, foreground: f32, background: f32) -> bool {
        if background < 0.7 * foreground {
            self.better_frames += 1;
        } else {
            self.better_frames = 0;
        }
        if self.better_frames >= 2 {
            for (target, source) in self.foreground.iter_mut().zip(&self.background) {
                target.copy_from_slice(source);
            }
            self.better_frames = 0;
            self.diverged_frames = 0;
            return true;
        }
        if background > 4.0 * foreground && background > mic {
            for (target, source) in self.background.iter_mut().zip(&self.foreground) {
                target.copy_from_slice(source);
            }
        }
        if foreground > 4.0 * mic + POWER_EPSILON {
            self.diverged_frames += 1;
            if self.diverged_frames > 20 {
                self.reset();
            }
        } else {
            self.diverged_frames = 0;
        }
        false
    }

    fn update_stats(&mut self, mic: f32, output: f32, echo: f32, reference_active: bool) {
        self.stats.reference_active = reference_active;
        self.stats.adapted = self.active_frames > BOOTSTRAP_FRAMES;
        if reference_active {
            self.mic_energy += ERLE_SMOOTHING * (mic - self.mic_energy);
            self.residual_energy += ERLE_SMOOTHING * (output - self.residual_energy);
            let ratio = (self.mic_energy + POWER_EPSILON) / (self.residual_energy + POWER_EPSILON);
            self.stats.erle_db = power_to_db(ratio).max(0.0);
        }
        let expected_residual = self.stats.leakage * echo;
        self.stats.double_talk = reference_active
            && self.stats.adapted
            && echo > REFERENCE_SILENCE_POWER * BLOCK as f32
            && output > 4.0 * expected_residual + REFERENCE_SILENCE_POWER * BLOCK as f32;
    }
}

enum OutputSelect {
    Foreground,
    Background,
}

fn tail_to_partitions(tail_ms: u32) -> usize {
    let clamped = tail_ms.clamp(MIN_TAIL_MS, MAX_TAIL_MS) as usize;
    (clamped * 48).div_ceil(BLOCK).clamp(1, MAX_PARTITIONS)
}

fn convolve(
    weights: &[Vec<Complex32>],
    history: &[Vec<Complex32>],
    newest: usize,
    accumulator: &mut [Complex32],
) {
    accumulator.fill(Complex32::default());
    for (partition, weight) in weights.iter().enumerate() {
        let spectrum = &history[(newest + partition) % MAX_PARTITIONS];
        for ((sum, w), x) in accumulator.iter_mut().zip(weight).zip(spectrum) {
            *sum += w * x;
        }
    }
}

fn forward_fft(
    plan: &dyn RealToComplex<f32>,
    input: &mut [f32],
    output: &mut [Complex32],
    scratch: &mut [Complex32],
) {
    // Buffer lengths are fixed at construction, so the transform cannot fail.
    let _ = plan.process_with_scratch(input, output, scratch);
}

fn inverse_fft(
    plan: &dyn ComplexToReal<f32>,
    input: &mut [Complex32],
    output: &mut [f32],
    scratch: &mut [Complex32],
) {
    // A real signal has purely real DC and Nyquist bins; drop rounding residue there.
    input[0].im = 0.0;
    input[BINS - 1].im = 0.0;
    let _ = plan.process_with_scratch(input, output, scratch);
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{BLOCK, EchoCanceller};
    use crate::dsp::level::{energy, power_to_db};
    use crate::dsp::test_signal::Noise;

    /// Synthetic room: 30 ms bulk delay followed by a decaying sparse response.
    fn room_response() -> Vec<f32> {
        let mut response = vec![0.0; 48 * 120];
        let mut noise = Noise::new(99);
        for (index, tap) in response.iter_mut().enumerate().skip(48 * 30) {
            let age = (index - 48 * 30) as f32 / 48.0;
            *tap = 0.5 * noise.next() * (-age / 15.0).exp();
        }
        response[48 * 30] = 0.6;
        response
    }

    fn render_echo(reference: &[f32], response: &[f32]) -> Vec<f32> {
        (0..reference.len())
            .map(|n| {
                response
                    .iter()
                    .enumerate()
                    .take(n + 1)
                    .map(|(k, tap)| tap * reference[n - k])
                    .sum()
            })
            .collect()
    }

    fn run(canceller: &mut EchoCanceller, mic: &[f32], reference: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut output = vec![0.0; mic.len()];
        let mut echo = vec![0.0; mic.len()];
        for frame in 0..mic.len() / BLOCK {
            let range = frame * BLOCK..(frame + 1) * BLOCK;
            let (out, est) = (&mut output[range.clone()], &mut echo[range.clone()]);
            canceller.process(&mic[range.clone()], &reference[range], out, est);
        }
        (output, echo)
    }

    #[test]
    fn converges_on_single_talk_echo() {
        let seconds = 6;
        let mut noise = Noise::new(7);
        let reference: Vec<f32> = (0..48_000 * seconds).map(|_| 0.3 * noise.next()).collect();
        let mic = render_echo(&reference, &room_response());

        let mut canceller = EchoCanceller::new();
        canceller.set_tail_ms(150);
        let (output, _) = run(&mut canceller, &mic, &reference);

        let tail = 48_000 * (seconds - 1)..;
        let erle = power_to_db(energy(&mic[tail.clone()]) / energy(&output[tail]));
        assert!(erle > 25.0, "ERLE after convergence was {erle:.1} dB");
        assert!(canceller.stats().adapted);
        assert!(canceller.stats().erle_db > 20.0);
    }

    #[test]
    fn preserves_near_end_speech_during_double_talk() {
        let seconds = 8;
        let mut noise = Noise::new(11);
        let reference: Vec<f32> = (0..48_000 * seconds).map(|_| 0.3 * noise.next()).collect();
        let echo = render_echo(&reference, &room_response());
        // Near-end "speech": an amplitude-modulated tone burst in the last 3 seconds.
        let near: Vec<f32> = (0..reference.len())
            .map(|n| {
                if n < 48_000 * 5 {
                    return 0.0;
                }
                let t = n as f32 / 48_000.0;
                let envelope = 0.5 + 0.5 * (TAU * 3.0 * t).sin();
                0.2 * envelope * (TAU * 440.0 * t).sin()
            })
            .collect();
        let mic: Vec<f32> = echo.iter().zip(&near).map(|(e, s)| e + s).collect();

        let mut canceller = EchoCanceller::new();
        canceller.set_tail_ms(150);
        let (output, _) = run(&mut canceller, &mic, &reference);

        let window = 48_000 * 6..;
        let residual: Vec<f32> = output[window.clone()]
            .iter()
            .zip(&near[window.clone()])
            .map(|(out, speech)| out - speech)
            .collect();
        let speech_to_residual = power_to_db(energy(&near[window.clone()]) / energy(&residual));
        let echo_to_speech_in = power_to_db(energy(&echo[window.clone()]) / energy(&near[window]));
        assert!(
            speech_to_residual > 12.0,
            "near-end speech to residual {speech_to_residual:.1} dB (echo/speech in \
             {echo_to_speech_in:.1} dB)"
        );
    }

    #[test]
    fn passes_mic_through_without_reference() {
        let mut noise = Noise::new(3);
        let mic: Vec<f32> = (0..48_000).map(|_| 0.1 * noise.next()).collect();
        let reference = vec![0.0; mic.len()];
        let mut canceller = EchoCanceller::new();
        let (output, echo) = run(&mut canceller, &mic, &reference);
        assert_eq!(output, mic);
        assert!(echo.iter().all(|sample| *sample == 0.0));
    }
}
