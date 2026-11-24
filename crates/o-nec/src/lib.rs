//! O-NEC (Open Noise and Echo Cancelation): a voice cleanup plugin combining an
//! adaptive acoustic echo canceller, residual echo suppression, RNNoise-based noise
//! suppression and a voice-activity gate.

pub mod dsp;
mod editor;
pub mod params;
pub mod telemetry;

use std::num::NonZeroU32;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use aud_ui::export_plugin;
use nih_plug::prelude::{AsyncExecutor, Params};
use nih_plug::prelude::{
    AudioIOLayout, AuxiliaryBuffers, Buffer, BufferConfig, ClapFeature, ClapPlugin, Editor,
    InitContext, MidiConfig, Plugin, PortNames, ProcessContext, ProcessMode, ProcessStatus,
    Vst3Plugin, Vst3SubCategory, new_nonzero_u32, nih_log,
};
use triple_buffer::{Output, triple_buffer};

use crate::dsp::deep_filter::Pacing;
use crate::dsp::engine::{Engine, EngineSettings};
use crate::dsp::gate::GateSettings;
use crate::dsp::level::db_to_gain;
use crate::dsp::leveler::LevelerSettings;
use crate::dsp::stream::StreamProcessor;
use crate::params::ONecParams;
use crate::telemetry::Telemetry;

pub struct ONec {
    params: Arc<ONecParams>,
    stream: StreamProcessor,
    telemetry: Arc<Mutex<Output<Telemetry>>>,
    mic: Vec<f32>,
    reference: Vec<f32>,
    output: Vec<f32>,
    /// Latency last reported to the host in samples.
    reported_latency: u32,
}

impl Default for ONec {
    fn default() -> Self {
        let (input, output) = triple_buffer(&Telemetry::default());
        Self {
            params: Arc::new(ONecParams::default()),
            stream: StreamProcessor::new(Engine::new(Some(input))),
            telemetry: Arc::new(Mutex::new(output)),
            mic: Vec::new(),
            reference: Vec::new(),
            output: Vec::new(),
            reported_latency: 0,
        }
    }
}

impl ONec {
    fn engine_settings(&self) -> EngineSettings {
        let params = &self.params;
        EngineSettings {
            input_gain: db_to_gain(params.input_gain_db.value()),
            output_gain: db_to_gain(params.output_gain_db.value()),
            high_pass_hz: params
                .high_pass_enabled
                .value()
                .then(|| params.high_pass_hz.value()),
            denoise_enabled: params.denoise_enabled.value(),
            noise_model: params.noise_model.value().into(),
            reduction_db: params.reduction_db.value(),
            echo_enabled: params.echo_enabled.value(),
            echo_tail_ms: params.echo_tail_ms.value().max(0) as u32,
            echo_suppression: params.echo_suppression.value() / 100.0,
            gate: GateSettings {
                trigger: params.gate_mode.value().into(),
                voice_threshold: params.gate_voice_threshold.value() / 100.0,
                level_threshold_db: params.gate_level_db.value(),
                range_db: params.gate_range_db.value(),
                hold_ms: params.gate_hold_ms.value(),
                release_ms: params.gate_release_ms.value(),
            },
            leveler: LevelerSettings {
                enabled: params.leveler_enabled.value(),
                target_db: params.leveler_target_db.value(),
                amount: params.leveler_amount.value() / 100.0,
                max_boost_db: params.leveler_max_boost_db.value(),
                max_cut_db: params.leveler_max_cut_db.value(),
                speed: params.leveler_speed.value().into(),
            },
            listen_removed: params.listen_removed.value(),
            analyze: params.editor_state.is_open(),
        }
    }
}

impl Plugin for ONec {
    const NAME: &'static str = "O-NEC";
    const VENDOR: &'static str = "o-at";
    const URL: &'static str = "https://github.com/baalintnagy/o-at";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames {
                layout: Some("Stereo"),
                main_input: Some("Microphone"),
                main_output: Some("Output"),
                aux_inputs: &["Echo Reference"],
                aux_outputs: &[],
            },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            aux_input_ports: &[new_nonzero_u32(1)],
            names: PortNames {
                layout: Some("Mono"),
                main_input: Some("Microphone"),
                main_output: Some("Output"),
                aux_inputs: &["Echo Reference"],
                aux_outputs: &[],
            },
            ..AudioIOLayout::const_default()
        },
        // Fallbacks for hosts without sidechain support: noise suppression and gate only.
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            names: PortNames {
                layout: Some("Stereo, no echo reference"),
                main_input: Some("Microphone"),
                main_output: Some("Output"),
                aux_inputs: &[],
                aux_outputs: &[],
            },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            names: PortNames {
                layout: Some("Mono, no echo reference"),
                main_input: Some("Microphone"),
                main_output: Some("Output"),
                aux_inputs: &[],
                aux_outputs: &[],
            },
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.telemetry.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let host_rate = buffer_config.sample_rate.round() as u32;
        let max_buffer = buffer_config.max_buffer_size.max(1) as usize;
        let pacing = match buffer_config.process_mode {
            ProcessMode::Realtime => Pacing::Realtime,
            ProcessMode::Buffered | ProcessMode::Offline => Pacing::Offline,
        };
        let settings = self.engine_settings();
        self.stream.engine_mut().set_settings(settings);
        if let Err(error) = self.stream.configure(host_rate, max_buffer, pacing) {
            nih_log!("O-NEC cannot run at {host_rate} Hz: {error:?}");
            return false;
        }
        self.mic = vec![0.0; max_buffer];
        self.reference = vec![0.0; max_buffer];
        self.output = vec![0.0; max_buffer];
        self.reported_latency = self.stream.latency_samples();
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.stream.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let settings = self.engine_settings();
        self.stream.engine_mut().set_settings(settings);
        // Switching the noise model changes the latency.
        let latency = self.stream.latency_samples();
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
        }

        let total = buffer.samples();
        let capacity = self.output.len();
        if capacity == 0 {
            return ProcessStatus::Normal;
        }
        let reference = aux
            .inputs
            .first_mut()
            .map(|sidechain| sidechain.as_slice_immutable());
        let channels = buffer.as_slice();

        let mut offset = 0;
        while offset < total {
            let count = (total - offset).min(capacity);
            let range = offset..offset + count;
            mix_to_mono(channels, range.clone(), &mut self.mic[..count]);
            match reference {
                Some(sidechain) if !sidechain.is_empty() => {
                    mix_to_mono(sidechain, range.clone(), &mut self.reference[..count]);
                }
                _ => self.reference[..count].fill(0.0),
            }
            self.stream.process(
                &self.mic[..count],
                &self.reference[..count],
                &mut self.output[..count],
            );
            for channel in channels.iter_mut() {
                channel[range.clone()].copy_from_slice(&self.output[..count]);
            }
            offset += count;
        }
        ProcessStatus::Normal
    }
}

fn mix_to_mono<C: AsRef<[f32]>>(channels: &[C], range: Range<usize>, target: &mut [f32]) {
    let Some(first) = channels.first() else {
        target.fill(0.0);
        return;
    };
    target.copy_from_slice(&first.as_ref()[range.clone()]);
    if channels.len() == 1 {
        return;
    }
    for channel in &channels[1..] {
        for (sum, sample) in target.iter_mut().zip(&channel.as_ref()[range.clone()]) {
            *sum += sample;
        }
    }
    let scale = 1.0 / channels.len() as f32;
    target.iter_mut().for_each(|sample| *sample *= scale);
}

impl ClapPlugin for ONec {
    const CLAP_ID: &'static str = "o-at.o-nec";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Open Noise and Echo Cancelation for voice");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Restoration,
    ];
}

impl Vst3Plugin for ONec {
    const VST3_CLASS_ID: [u8; 16] = *b"O-AT-O-NEC-Voice";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

export_plugin!(ONec);
