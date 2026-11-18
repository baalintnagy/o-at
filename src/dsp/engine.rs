//! The 48 kHz processing chain:
//!
//! input gain → high-pass → echo canceller → residual echo suppressor → noise suppression
//! (RNNoise or DeepFilterNet) → voice gate → speech leveler → output gain.

use biquad::{Biquad, Coefficients, DirectForm2Transposed, Q_BUTTERWORTH_F32, ToHertz, Type};
use triple_buffer::Input;

use crate::dsp::aec::EchoCanceller;
use crate::dsp::analyzer::{Analyzer, SpectrumTap};
use crate::dsp::deep_filter::Pacing;
use crate::dsp::delay::DelayLine;
use crate::dsp::denoise::{Denoiser, NoiseModel};
use crate::dsp::gate::{Gate, GateSettings, GateTrigger};
use crate::dsp::level::{FLOOR_DB, POWER_EPSILON, energy, gain_to_db, peak, power_to_db};
use crate::dsp::leveler::{LEVELER_DELAY, Leveler, LevelerSettings};
use crate::dsp::suppressor::{EchoSuppressor, SUPPRESSOR_DELAY};
use crate::dsp::{FRAME, INTERNAL_RATE};
use crate::telemetry::{HISTORY_LEN, Telemetry};

/// Peak meter fall rate in dB per frame.
const PEAK_RELEASE_DB: f32 = 0.9;

/// Smoothing of the displayed voice probability when it rises and falls.
const VOICE_DISPLAY_RISE: f32 = 0.6;
const VOICE_DISPLAY_FALL: f32 = 0.12;

/// Smoothing of the displayed reduction per frame.
const REDUCTION_SMOOTHING: f32 = 0.15;

/// Delay of the stages whose delay does not depend on the noise model, in samples.
const FIXED_DELAY: usize = SUPPRESSOR_DELAY + LEVELER_DELAY;

/// Settings applied to the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineSettings {
    pub input_gain: f32,
    pub output_gain: f32,
    pub high_pass_hz: Option<f32>,
    pub denoise_enabled: bool,
    pub noise_model: NoiseModel,
    pub reduction_db: f32,
    pub echo_enabled: bool,
    pub echo_tail_ms: u32,
    /// Residual echo suppression strength (0..1).
    pub echo_suppression: f32,
    pub gate: GateSettings,
    pub leveler: LevelerSettings,
    /// Outputs what the chain removes instead of what it keeps.
    pub listen_removed: bool,
    /// Computes spectra for the editor.
    pub analyze: bool,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            input_gain: 1.0,
            output_gain: 1.0,
            high_pass_hz: Some(80.0),
            denoise_enabled: true,
            noise_model: NoiseModel::RnNoise,
            reduction_db: 100.0,
            echo_enabled: true,
            echo_tail_ms: 250,
            echo_suppression: 0.5,
            gate: GateSettings {
                trigger: GateTrigger::Off,
                voice_threshold: 0.6,
                level_threshold_db: -50.0,
                range_db: 40.0,
                hold_ms: 200.0,
                release_ms: 150.0,
            },
            leveler: LevelerSettings::default(),
            listen_removed: false,
            analyze: false,
        }
    }
}

pub struct Engine {
    settings: EngineSettings,
    high_pass: DirectForm2Transposed<f32>,
    high_pass_hz: f32,
    high_pass_active: bool,
    echo: EchoCanceller,
    suppressor: EchoSuppressor,
    denoiser: Denoiser,
    gate: Gate,
    leveler: Leveler,
    analyzer: Analyzer,
    tap_input: SpectrumTap,
    tap_output: SpectrumTap,
    tap_reference: SpectrumTap,
    /// Input delayed by the chain's delay, for the editor and Listen Removed.
    dry_line: DelayLine,
    dry: [f32; FRAME],
    echo_estimate: [f32; FRAME],
    cancelled: [f32; FRAME],
    input_gain: f32,
    output_gain: f32,
    telemetry: Telemetry,
    publisher: Option<Input<Telemetry>>,
}

impl Engine {
    pub fn new(publisher: Option<Input<Telemetry>>) -> Self {
        let settings = EngineSettings::default();
        let high_pass_hz = settings.high_pass_hz.unwrap_or(80.0);
        let denoiser = Denoiser::new();
        let dry_line = DelayLine::new(
            FIXED_DELAY + denoiser.delay(NoiseModel::DeepFilterNet),
            FIXED_DELAY + denoiser.delay(settings.noise_model),
        );
        Self {
            settings,
            high_pass: DirectForm2Transposed::<f32>::new(high_pass_coefficients(high_pass_hz)),
            high_pass_hz,
            high_pass_active: false,
            echo: EchoCanceller::new(),
            suppressor: EchoSuppressor::new(),
            denoiser,
            gate: Gate::new(),
            leveler: Leveler::new(),
            analyzer: Analyzer::new(),
            tap_input: SpectrumTap::default(),
            tap_output: SpectrumTap::default(),
            tap_reference: SpectrumTap::default(),
            dry_line,
            dry: [0.0; FRAME],
            echo_estimate: [0.0; FRAME],
            cancelled: [0.0; FRAME],
            input_gain: settings.input_gain,
            output_gain: settings.output_gain,
            telemetry: Telemetry::default(),
            publisher,
        }
    }

    pub fn reset(&mut self) {
        self.high_pass.reset_state();
        self.echo.reset();
        self.suppressor.reset();
        self.denoiser.reset();
        self.gate.reset();
        self.leveler.reset();
        self.dry_line.clear();
    }

    /// Prepares DeepFilterNet with `pipeline_frames` frames of scheduling slack.
    /// Allocates; call it outside the audio callback.
    pub fn configure(&mut self, pipeline_frames: usize, pacing: Pacing) {
        self.denoiser.configure(pipeline_frames, pacing);
        self.dry_line
            .set_capacity(FIXED_DELAY + self.denoiser.delay(NoiseModel::DeepFilterNet));
    }

    /// Latency of the chain at 48 kHz for the current settings in samples, excluding
    /// host-side blocking and resampling.
    pub fn delay(&self) -> usize {
        FIXED_DELAY + self.denoiser.delay(self.settings.noise_model)
    }

    pub fn set_settings(&mut self, settings: EngineSettings) {
        self.settings = settings;
    }

    /// Records host information shown by the editor.
    pub fn set_host_info(&mut self, host_rate: f32, latency_samples: u32) {
        self.telemetry.host_rate = host_rate;
        self.telemetry.latency_samples = latency_samples;
    }

    /// Processes one 10 ms frame of `mic` in place, using `reference` as echo reference.
    pub fn process_frame(&mut self, mic: &mut [f32], reference: &[f32]) {
        debug_assert_eq!(mic.len(), FRAME);
        let settings = self.settings;

        ramp_gain(mic, &mut self.input_gain, settings.input_gain);
        self.dry_line.set_delay(self.delay());
        self.dry_line.process(mic, &mut self.dry);

        self.apply_high_pass(mic, settings.high_pass_hz);

        let mut leakage = 0.0;
        if settings.echo_enabled {
            self.echo.set_tail_ms(settings.echo_tail_ms);
            self.echo
                .process(mic, reference, &mut self.cancelled, &mut self.echo_estimate);
            mic.copy_from_slice(&self.cancelled);
            leakage = self.echo.stats().leakage;
        } else {
            self.echo_estimate.fill(0.0);
        }
        let suppression = if settings.echo_enabled {
            settings.echo_suppression
        } else {
            0.0
        };
        self.suppressor
            .process(mic, &self.echo_estimate, leakage, suppression);

        let voice = self.denoiser.process(
            mic,
            settings.denoise_enabled,
            settings.reduction_db,
            settings.noise_model,
        );
        let level_db = power_to_db(energy(mic) / FRAME as f32);
        self.gate.process(mic, &settings.gate, voice, level_db);
        // Leveling would hide what the other stages remove, so Listen Removed pauses it.
        let leveler = LevelerSettings {
            enabled: settings.leveler.enabled && !settings.listen_removed,
            ..settings.leveler
        };
        self.leveler.process(mic, &leveler, voice);

        if settings.listen_removed {
            for (sample, dry) in mic.iter_mut().zip(&self.dry) {
                *sample = dry - *sample;
            }
        }
        ramp_gain(mic, &mut self.output_gain, settings.output_gain);

        self.update_telemetry(mic, reference, voice);
    }

    fn apply_high_pass(&mut self, frame: &mut [f32], frequency: Option<f32>) {
        let Some(frequency) = frequency else {
            self.high_pass_active = false;
            return;
        };
        if !self.high_pass_active {
            self.high_pass.reset_state();
            self.high_pass_active = true;
        }
        if (frequency - self.high_pass_hz).abs() > 0.1 {
            self.high_pass_hz = frequency;
            self.high_pass
                .update_coefficients(high_pass_coefficients(frequency));
        }
        for sample in frame.iter_mut() {
            *sample = self.high_pass.run(*sample);
        }
    }

    fn update_telemetry(&mut self, output: &[f32], reference: &[f32], voice: f32) {
        let telemetry = &mut self.telemetry;
        let input_power = energy(&self.dry) / FRAME as f32;
        let output_power = energy(output) / FRAME as f32;

        telemetry.input_peak_db = fall(telemetry.input_peak_db, gain_to_db(peak(&self.dry)));
        telemetry.output_peak_db = fall(telemetry.output_peak_db, gain_to_db(peak(output)));
        telemetry.reference_peak_db =
            fall(telemetry.reference_peak_db, gain_to_db(peak(reference)));

        if input_power > 1.0e-9 {
            let ratio = (output_power + POWER_EPSILON) / (input_power + POWER_EPSILON);
            let reduction = power_to_db(ratio).min(0.0);
            telemetry.reduction_db += REDUCTION_SMOOTHING * (reduction - telemetry.reduction_db);
        }

        let stats = self.echo.stats();
        let rate = if voice > telemetry.voice_probability {
            VOICE_DISPLAY_RISE
        } else {
            VOICE_DISPLAY_FALL
        };
        telemetry.voice_probability += rate * (voice - telemetry.voice_probability);
        telemetry.gate_open = self.gate.is_open();
        telemetry.gate_gain_db = gain_to_db(self.gate.gain());
        telemetry.leveler_gain_db = self.leveler.gain_db();
        telemetry.limiter_db = self.leveler.limiter_db();
        telemetry.speech_level_db = self.leveler.speech_level_db().unwrap_or(FLOOR_DB);
        telemetry.erle_db = stats.erle_db;
        telemetry.echo_leakage = stats.leakage;
        telemetry.echo_adapted = stats.adapted;
        telemetry.reference_active = stats.reference_active;
        telemetry.double_talk = stats.double_talk;
        telemetry.echo_suppression_db = self.suppressor.suppression_db();
        telemetry.noise_model = self.settings.noise_model;
        telemetry.noise_status = self.denoiser.status();

        let head = telemetry.history_head;
        telemetry.voice_history[head] = telemetry.voice_probability;
        telemetry.gate_history[head] = self.gate.gain();
        telemetry.leveler_history_db[head] = telemetry.leveler_gain_db + telemetry.limiter_db;
        telemetry.input_history_db[head] = power_to_db(input_power);
        telemetry.output_history_db[head] = power_to_db(output_power);
        telemetry.echo_history[head] = stats.reference_active && self.settings.echo_enabled;
        telemetry.history_head = (head + 1) % HISTORY_LEN;
        telemetry.sequence = telemetry.sequence.wrapping_add(1);

        if !self.settings.analyze {
            return;
        }
        self.analyzer.analyze(&mut self.tap_input, &self.dry);
        self.analyzer.analyze(&mut self.tap_output, output);
        self.analyzer.analyze(&mut self.tap_reference, reference);
        telemetry.spectrum_input = *self.tap_input.bands();
        telemetry.spectrum_output = *self.tap_output.bands();
        telemetry.spectrum_reference = *self.tap_reference.bands();

        if let Some(publisher) = self.publisher.as_mut() {
            publisher.input_buffer_mut().clone_from(telemetry);
            publisher.publish();
        }
    }
}

fn high_pass_coefficients(frequency: f32) -> Coefficients<f32> {
    let nyquist_guard = INTERNAL_RATE as f32 * 0.45;
    Coefficients::<f32>::from_params(
        Type::HighPass,
        (INTERNAL_RATE as f32).hz(),
        frequency.clamp(10.0, nyquist_guard).hz(),
        Q_BUTTERWORTH_F32,
    )
    .unwrap_or(Coefficients {
        a1: 0.0,
        a2: 0.0,
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
    })
}

/// Applies a gain ramping linearly from `current` to `target` over the frame.
fn ramp_gain(frame: &mut [f32], current: &mut f32, target: f32) {
    let step = (target - *current) / frame.len() as f32;
    for (index, sample) in frame.iter_mut().enumerate() {
        *sample *= *current + step * (index + 1) as f32;
    }
    *current = target;
}

fn fall(shown: f32, value: f32) -> f32 {
    value.max(shown - PEAK_RELEASE_DB).max(FLOOR_DB)
}

/// Settings under which the chain is a pure delay.
#[cfg(test)]
pub(crate) fn neutral_settings() -> EngineSettings {
    EngineSettings {
        high_pass_hz: None,
        denoise_enabled: false,
        echo_enabled: false,
        ..EngineSettings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{Engine, EngineSettings, neutral_settings};
    use crate::dsp::FRAME;
    use crate::dsp::deep_filter::Pacing;
    use crate::dsp::denoise::NoiseModel;

    #[test]
    fn neutral_settings_delay_the_signal() {
        for noise_model in [NoiseModel::RnNoise, NoiseModel::DeepFilterNet] {
            let mut engine = Engine::new(None);
            engine.configure(2, Pacing::Offline);
            engine.set_settings(EngineSettings {
                noise_model,
                ..neutral_settings()
            });
            let delay = engine.delay();

            let input: Vec<f32> = (0..FRAME * 16)
                .map(|n| (n as f32 * 0.07).sin() * 0.3)
                .collect();
            let reference = vec![0.0; FRAME];
            let mut output = input.clone();
            for frame in output.chunks_mut(FRAME) {
                engine.process_frame(frame, &reference);
            }
            for index in delay + FRAME..input.len() {
                let expected = input[index - delay];
                assert!(
                    (output[index] - expected).abs() < 1.0e-4,
                    "{noise_model:?} {index}"
                );
            }
        }
    }

    #[test]
    fn listen_removed_is_silent_when_nothing_is_removed() {
        let mut engine = Engine::new(None);
        engine.set_settings(EngineSettings {
            listen_removed: true,
            ..neutral_settings()
        });

        let reference = vec![0.0; FRAME];
        let mut peak: f32 = 0.0;
        for frame_index in 0..10 {
            let mut frame: Vec<f32> = (0..FRAME)
                .map(|n| ((frame_index * FRAME + n) as f32 * 0.07).sin() * 0.3)
                .collect();
            engine.process_frame(&mut frame, &reference);
            if frame_index > 3 {
                peak = frame
                    .iter()
                    .fold(peak, |current, sample| current.max(sample.abs()));
            }
        }
        assert!(peak < 1.0e-3, "residual {peak}");
    }
}
