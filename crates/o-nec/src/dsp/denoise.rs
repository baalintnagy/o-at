//! Speech-preserving noise suppression with a choice of two models:
//!
//! - **RNNoise** (via `nnnoiseless`): a small recurrent network, light on CPU, with one
//!   frame of delay.
//! - **DeepFilterNet3** (via libDF, see [`crate::dsp::deep_filter`]): a larger model that
//!   also removes non-stationary noise, with more delay and CPU use.
//!
//! RNNoise always runs, also when suppression is disabled or DeepFilterNet is selected,
//! so that its voice activity estimate stays available to the gate. With DeepFilterNet,
//! RNNoise's output also stands in, time aligned, for frames the DeepFilterNet worker
//! does not deliver in time. Each model keeps a constant delay whether suppression is
//! enabled or not. The reduction amount limits attenuation by mixing in the
//! time-aligned dry signal.

use std::cell::Cell;

use nih_plug::util::permit_alloc;
use nnnoiseless::DenoiseState;

use crate::dsp::FRAME;
use crate::dsp::deep_filter::{DeepFilter, MODEL_DELAY, Pacing};
use crate::dsp::delay::DelayLine;
use crate::dsp::level::db_to_gain;

/// Latency of RNNoise's analysis/synthesis in samples.
pub const RNNOISE_DELAY: usize = FRAME;

/// Reduction amount at which the dry signal is removed completely.
pub const FULL_REDUCTION_DB: f32 = 100.0;

/// RNNoise expects 16-bit PCM magnitudes in `f32`.
const PCM_SCALE: f32 = 32_768.0;

/// Frames for which a missed DeepFilterNet result is reported (0.5 s).
const LATE_HOLD_FRAMES: u32 = 50;

thread_local! {
    /// Whether nnnoiseless's FFT cache is initialized on this thread.
    static FFT_CACHE_READY: Cell<bool> = const { Cell::new(false) };
}

/// Noise suppression model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoiseModel {
    #[default]
    RnNoise,
    DeepFilterNet,
}

/// State of the DeepFilterNet model, for the editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModelStatus {
    /// The selected model is running normally.
    #[default]
    Running,
    /// DeepFilterNet is loading; RNNoise stands in.
    Loading,
    /// DeepFilterNet recently missed frames; RNNoise stood in for them.
    Late,
    /// DeepFilterNet could not be started; RNNoise stands in.
    Failed,
}

pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    /// Throwaway state used to initialize nnnoiseless's FFT cache on a new thread.
    warm_up: Box<DenoiseState<'static>>,
    input: [f32; FRAME],
    output: [f32; FRAME],
    scratch: [f32; FRAME],
    wet: [f32; FRAME],
    dry: [f32; FRAME],
    deep_filter: Option<DeepFilter>,
    /// Model used for the previous frame.
    model: NoiseModel,
    /// Whether DeepFilterNet received the previous frame.
    deep_filter_streaming: bool,
    /// Input delayed by the model's delay, for the reduction mix and the bypass.
    dry_line: DelayLine,
    /// RNNoise output delayed to DeepFilterNet's delay.
    fallback_line: DelayLine,
    /// Voice probability delayed to DeepFilterNet's delay, in frames.
    voice_line: DelayLine,
    dry_mix: f32,
    voice_probability: f32,
    late_frames: u32,
    frames_since_restart: usize,
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl Denoiser {
    /// Creates a denoiser. DeepFilterNet is only available after
    /// [`Denoiser::configure`]; until then RNNoise stands in for it.
    pub fn new() -> Self {
        let deep_delay = deep_filter_delay(None);
        Self {
            state: DenoiseState::new(),
            warm_up: DenoiseState::new(),
            input: [0.0; FRAME],
            output: [0.0; FRAME],
            scratch: [0.0; FRAME],
            wet: [0.0; FRAME],
            dry: [0.0; FRAME],
            deep_filter: None,
            model: NoiseModel::RnNoise,
            deep_filter_streaming: false,
            dry_line: DelayLine::new(deep_delay, RNNOISE_DELAY),
            fallback_line: DelayLine::new(deep_delay - RNNOISE_DELAY, 0),
            voice_line: DelayLine::new(deep_delay / FRAME, 0),
            dry_mix: 1.0,
            voice_probability: 0.0,
            late_frames: 0,
            frames_since_restart: 0,
        }
    }

    /// Starts DeepFilterNet's worker with `pipeline_frames` frames of scheduling slack.
    /// Allocates; call it outside the audio callback.
    pub fn configure(&mut self, pipeline_frames: usize, pacing: Pacing) {
        match self.deep_filter.as_mut() {
            Some(deep_filter) => deep_filter.set_pipeline(pipeline_frames, pacing),
            None => self.deep_filter = Some(DeepFilter::new(pipeline_frames, pacing)),
        }
        let deep_delay = deep_filter_delay(self.deep_filter.as_ref());
        self.dry_line.set_capacity(deep_delay);
        self.fallback_line.set_capacity(deep_delay - RNNOISE_DELAY);
        self.voice_line.set_capacity(deep_delay / FRAME);
        self.apply_delays(self.model);
        self.deep_filter_streaming = false;
    }

    /// Delay of the stage in samples when running `model`.
    pub fn delay(&self, model: NoiseModel) -> usize {
        match model {
            NoiseModel::RnNoise => RNNOISE_DELAY,
            NoiseModel::DeepFilterNet => deep_filter_delay(self.deep_filter.as_ref()),
        }
    }

    /// Clears the delay lines. Hosts may call this on the audio thread, so the models'
    /// state is kept rather than reallocated; it follows the new input within a few
    /// frames.
    pub fn reset(&mut self) {
        self.dry_line.clear();
        self.fallback_line.clear();
        self.voice_line.clear();
        self.voice_probability = 0.0;
        self.deep_filter_streaming = false;
    }

    /// Voice probability (0..1) of the most recently analyzed frame.
    pub fn voice_probability(&self) -> f32 {
        self.voice_probability
    }

    /// State of the selected model.
    pub fn status(&self) -> ModelStatus {
        if self.model == NoiseModel::RnNoise {
            return ModelStatus::Running;
        }
        match self.deep_filter.as_ref() {
            None => ModelStatus::Failed,
            Some(deep_filter) if deep_filter.has_failed() => ModelStatus::Failed,
            Some(deep_filter) if !deep_filter.is_ready() => ModelStatus::Loading,
            Some(_) if self.late_frames > 0 => ModelStatus::Late,
            Some(_) => ModelStatus::Running,
        }
    }

    /// Processes one frame in place and returns the voice probability aligned with the
    /// output.
    ///
    /// `reduction_db` limits the attenuation: 0 dB passes the delayed input, and
    /// [`FULL_REDUCTION_DB`] or more outputs the model's result unchanged.
    pub fn process(
        &mut self,
        frame: &mut [f32],
        enabled: bool,
        reduction_db: f32,
        model: NoiseModel,
    ) -> f32 {
        debug_assert_eq!(frame.len(), FRAME);
        if model != self.model {
            self.model = model;
            self.apply_delays(model);
        }

        for (scaled, sample) in self.input.iter_mut().zip(frame.iter()) {
            *scaled = sample * PCM_SCALE;
        }
        self.warm_up_thread();
        let voice = self.state.process_frame(&mut self.output, &self.input);
        for (wet, sample) in self.wet.iter_mut().zip(&self.output) {
            *wet = sample / PCM_SCALE;
        }

        if model == NoiseModel::DeepFilterNet {
            self.fallback_line.process_in_place(&mut self.wet);
            self.voice_probability = delay_value(&mut self.voice_line, voice);
            self.run_deep_filter(frame, enabled);
        } else {
            self.voice_probability = voice;
            self.deep_filter_streaming = false;
            self.late_frames = 0;
        }
        self.dry_line.process(frame, &mut self.dry);

        let target_dry = if !enabled {
            1.0
        } else if reduction_db >= FULL_REDUCTION_DB {
            0.0
        } else {
            db_to_gain(-reduction_db)
        };
        let start_dry = self.dry_mix;
        let step = (target_dry - start_dry) / FRAME as f32;
        for (index, sample) in frame.iter_mut().enumerate() {
            let dry_mix = start_dry + step * (index + 1) as f32;
            *sample = self.wet[index] * (1.0 - dry_mix) + self.dry[index] * dry_mix;
        }
        self.dry_mix = target_dry;
        self.voice_probability
    }

    /// Replaces the fallback in `self.wet` with DeepFilterNet's result when available.
    /// The worker only runs while suppression is enabled.
    fn run_deep_filter(&mut self, frame: &[f32], enabled: bool) {
        self.late_frames = self.late_frames.saturating_sub(1);
        let Some(deep_filter) = self.deep_filter.as_mut() else {
            return;
        };
        if !enabled {
            self.deep_filter_streaming = false;
            return;
        }
        if !self.deep_filter_streaming {
            deep_filter.reset();
            self.deep_filter_streaming = true;
            self.frames_since_restart = 0;
        }
        let delivered = deep_filter.process(frame, &mut self.wet);
        let filling = self.frames_since_restart <= deep_filter.pipeline_frames();
        self.frames_since_restart = self.frames_since_restart.saturating_add(1);
        if !delivered && !filling && deep_filter.is_ready() {
            self.late_frames = LATE_HOLD_FRAMES;
        }
    }

    fn apply_delays(&mut self, model: NoiseModel) {
        let delay = self.delay(model);
        self.dry_line.set_delay(delay);
        self.fallback_line.set_delay(delay - RNNOISE_DELAY);
        // RNNoise's probability describes input that its output emits one frame later.
        self.voice_line.set_delay(delay / FRAME - 1);
        self.deep_filter_streaming = false;
    }

    /// nnnoiseless plans its FFTs in a thread-local cache (through `easyfft`) on first
    /// use, which allocates once per thread. Hosts may move processing between threads,
    /// so fill the cache for every FFT size with a non-silent frame on a spare state,
    /// explicitly permitting only these allocations. Later allocations remain errors in
    /// debug builds.
    fn warm_up_thread(&mut self) {
        if FFT_CACHE_READY.with(Cell::get) {
            return;
        }
        for (index, sample) in self.scratch.iter_mut().enumerate() {
            *sample = if index % 2 == 0 { 1_000.0 } else { -1_000.0 };
        }
        let (warm_up, output, input) = (&mut self.warm_up, &mut self.output, &self.scratch);
        permit_alloc(|| warm_up.process_frame(output, input));
        FFT_CACHE_READY.with(|ready| ready.set(true));
    }
}

/// DeepFilterNet's stage delay, with a one-frame pipeline before configuration.
fn deep_filter_delay(deep_filter: Option<&DeepFilter>) -> usize {
    deep_filter.map_or(MODEL_DELAY + FRAME, DeepFilter::delay)
}

fn delay_value(line: &mut DelayLine, value: f32) -> f32 {
    let mut delayed = [0.0];
    line.process(&[value], &mut delayed);
    delayed[0]
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{Denoiser, NoiseModel, RNNOISE_DELAY};
    use crate::dsp::FRAME;
    use crate::dsp::deep_filter::Pacing;
    use crate::dsp::level::{energy, power_to_db};
    use crate::dsp::test_signal::Noise;

    fn coloured_noise(seed: u32) -> Vec<f32> {
        let mut noise = Noise::new(seed);
        let mut lowpassed = 0.0;
        (0..48_000 * 3)
            .map(|_| {
                lowpassed = 0.95 * lowpassed + 0.05 * noise.next();
                0.3 * lowpassed
            })
            .collect()
    }

    /// A voiced, vowel-like signal: twelve harmonics of a pitch gliding with vibrato
    /// around 140 Hz, under a slow syllable envelope. The pitch keeps changing so that
    /// the signal never repeats and cross-correlation has a single peak.
    fn vowel(length: usize) -> Vec<f32> {
        let mut phase = 0.0_f32;
        (0..length)
            .map(|n| {
                let t = n as f32 / 48_000.0;
                let pitch = 110.0 + 30.0 * t + 15.0 * (TAU * 1.3 * t).sin();
                phase = (phase + TAU * pitch / 48_000.0) % TAU;
                let envelope = 0.6 + 0.4 * (TAU * 3.0 * t).sin();
                let voiced: f32 = (1..=12)
                    .map(|harmonic| (phase * harmonic as f32).sin() / harmonic as f32)
                    .sum();
                0.15 * envelope * voiced
            })
            .collect()
    }

    fn run(
        denoiser: &mut Denoiser,
        input: &[f32],
        reduction_db: f32,
        model: NoiseModel,
    ) -> Vec<f32> {
        let mut output = input.to_vec();
        for frame in output.chunks_mut(FRAME) {
            denoiser.process(frame, true, reduction_db, model);
        }
        output
    }

    fn reduction_after_one_second(input: &[f32], output: &[f32]) -> f32 {
        let tail = 48_000..;
        power_to_db(energy(&output[tail.clone()]) / energy(&input[tail]))
    }

    /// Lag of the strongest correlation between `input` and `output`.
    fn delay_of(input: &[f32], output: &[f32], max_lag: usize) -> usize {
        let window = &input[24_000..48_000];
        (0..max_lag)
            .map(|lag| {
                let correlation: f32 = window
                    .iter()
                    .zip(&output[24_000 + lag..])
                    .map(|(a, b)| a * b)
                    .sum();
                (lag, correlation)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0, |(lag, _)| lag)
    }

    /// RNNoise's own synthesis delay must match the dry path, so that partial
    /// reduction settings do not comb-filter.
    #[test]
    fn rnnoise_delay_matches_dry_path() {
        let mut denoiser = Denoiser::new();
        let length = FRAME * 8;
        // Quiet enough that RNNoise treats it as silence and skips the gains.
        let mut input = vec![0.0; length];
        input[FRAME * 3 + 17] = 1.0e-6;
        let wet = run(&mut denoiser, &input, 100.0, NoiseModel::RnNoise);
        let peak = wet
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(index, _)| index)
            .unwrap_or_default();
        assert_eq!(peak, FRAME * 3 + 17 + RNNOISE_DELAY);
    }

    /// RNNoise targets realistic, coloured noise (fans, hum, traffic). Loud flat white
    /// noise is a known weak spot of its model and is not asserted here.
    #[test]
    fn rnnoise_removes_coloured_stationary_noise() {
        let input = coloured_noise(5);
        let output = run(&mut Denoiser::new(), &input, 100.0, NoiseModel::RnNoise);
        let reduction = reduction_after_one_second(&input, &output);
        assert!(reduction < -25.0, "noise reduction {reduction:.1} dB");
    }

    #[test]
    fn reduction_amount_limits_attenuation() {
        let input = coloured_noise(8);
        let output = run(&mut Denoiser::new(), &input, 12.0, NoiseModel::RnNoise);
        let reduction = reduction_after_one_second(&input, &output);
        assert!(
            (reduction - -12.0).abs() < 1.5,
            "limited reduction {reduction:.1} dB"
        );
    }

    #[test]
    fn disabled_denoiser_is_a_pure_delay() {
        for model in [NoiseModel::RnNoise, NoiseModel::DeepFilterNet] {
            let mut denoiser = Denoiser::new();
            denoiser.configure(2, Pacing::Offline);
            let delay = denoiser.delay(model);
            let input: Vec<f32> = (0..FRAME * 12)
                .map(|n| (n as f32 * 0.03).sin() * 0.4)
                .collect();
            let mut output = input.clone();
            for frame in output.chunks_mut(FRAME) {
                denoiser.process(frame, false, 100.0, model);
            }
            for index in delay..input.len() {
                let expected = input[index - delay];
                assert!(
                    (output[index] - expected).abs() < 1.0e-6,
                    "{model:?} {index}"
                );
            }
        }
    }

    #[test]
    fn deep_filter_removes_coloured_stationary_noise() {
        let input = coloured_noise(5);
        let mut denoiser = Denoiser::new();
        denoiser.configure(2, Pacing::Offline);
        let output = run(&mut denoiser, &input, 100.0, NoiseModel::DeepFilterNet);
        let reduction = reduction_after_one_second(&input, &output);
        assert!(reduction < -25.0, "noise reduction {reduction:.1} dB");
    }

    /// DeepFilterNet's output must line up with the dry path at its reported delay, so
    /// that partial reduction settings do not comb-filter. The model treats synthetic
    /// steady tones as noise, so the probe is a short transient over a quiet noise floor,
    /// which it passes.
    #[test]
    fn deep_filter_is_aligned_with_dry_path() {
        for pipeline in [1, 3] {
            let mut noise = Noise::new(3);
            let mut input: Vec<f32> = (0..48_000 * 2).map(|_| 0.001 * noise.next()).collect();
            let burst = 72_017;
            for (index, sample) in input[burst..burst + 240].iter_mut().enumerate() {
                let decay = 1.0 - index as f32 / 240.0;
                *sample += 0.5 * (index as f32 * 0.2).sin() * decay;
            }
            let mut denoiser = Denoiser::new();
            denoiser.configure(pipeline, Pacing::Offline);
            let output = run(&mut denoiser, &input, 100.0, NoiseModel::DeepFilterNet);
            let delay = denoiser.delay(NoiseModel::DeepFilterNet);

            let probe = &input[burst..burst + 240];
            let measured = (0..delay + 4 * FRAME)
                .map(|lag| {
                    let correlation: f32 = probe
                        .iter()
                        .zip(&output[burst + lag..])
                        .map(|(a, b)| a * b)
                        .sum();
                    (lag, correlation)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map_or(0, |(lag, _)| lag);
            assert_eq!(measured, delay, "pipeline {pipeline}");
        }
    }

    /// Without the worker's results, RNNoise stands in at the same delay. RNNoise's
    /// filtering shifts the correlation peak of voiced sound by a few samples.
    #[test]
    fn missing_deep_filter_results_fall_back_to_aligned_rnnoise() {
        let input = vowel(48_000 * 2);
        let mut denoiser = Denoiser::new();
        let output = run(&mut denoiser, &input, 100.0, NoiseModel::DeepFilterNet);
        let delay = denoiser.delay(NoiseModel::DeepFilterNet);
        let measured = delay_of(&input, &output, delay + FRAME);
        assert!(
            measured.abs_diff(delay) <= 4,
            "measured {measured}, delay {delay}"
        );
    }
}
