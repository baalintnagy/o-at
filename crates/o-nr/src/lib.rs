//! O-NR (Open Noise Reduction): a denoiser for any material. It learns a noise profile
//! from a noise-only passage, or tracks the noise floor continuously, and removes that
//! noise with a spectral gain estimator; see [`dsp`].

pub mod dsp;
mod editor;
pub mod params;
pub mod telemetry;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aud_ui::export_plugin;
use nih_plug::prelude::{
    AsyncExecutor, AudioIOLayout, AuxiliaryBuffers, Buffer, BufferConfig, ClapFeature, ClapPlugin,
    Editor, InitContext, MidiConfig, Params, Plugin, PortNames, ProcessContext, ProcessStatus,
    Vst3Plugin, Vst3SubCategory,
};
use triple_buffer::{Output, triple_buffer};

use crate::dsp::engine::{Engine, Settings};
use crate::params::ONrParams;
use crate::telemetry::Telemetry;

pub struct ONr {
    params: Arc<ONrParams>,
    /// Set by the editor's LEARN button while a profile is being learned. Not a
    /// parameter, so sessions never restore it.
    learn: Arc<AtomicBool>,
    engine: Engine,
    telemetry: Arc<Mutex<Output<Telemetry>>>,
    /// Latency last reported to the host in samples.
    reported_latency: u32,
}

impl Default for ONr {
    fn default() -> Self {
        let params = Arc::new(ONrParams::default());
        let (input, output) = triple_buffer(&Telemetry::default());
        Self {
            engine: Engine::new(params.profile.clone(), Some(input)),
            params,
            learn: Arc::new(AtomicBool::new(false)),
            telemetry: Arc::new(Mutex::new(output)),
            reported_latency: 0,
        }
    }
}

impl ONr {
    fn settings(&self) -> Settings {
        let params = &self.params;
        Settings {
            mode: params.mode.value().into(),
            learn: self.learn.load(Ordering::Relaxed),
            reduction_db: params.reduction_db.value(),
            threshold_db: params.threshold_db.value(),
            release_ms: params.release_ms.value(),
            smoothing: params.smoothing.value() / 100.0,
            resolution: params.resolution.value().into(),
            link: params.link.value(),
            listen_removed: params.listen_removed.value(),
        }
    }
}

impl Plugin for ONr {
    const NAME: &'static str = "O-NR";
    const VENDOR: &'static str = "o-at";
    const URL: &'static str = "https://github.com/baalintnagy/o-at";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            names: PortNames {
                layout: Some("Stereo"),
                main_input: Some("Input"),
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
                layout: Some("Mono"),
                main_input: Some("Input"),
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
        editor::create(
            self.params.clone(),
            self.learn.clone(),
            self.telemetry.clone(),
        )
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let channels = audio_io_layout
            .main_input_channels
            .map(NonZeroU32::get)
            .unwrap_or(1) as usize;
        let settings = self.settings();
        self.engine
            .configure(buffer_config.sample_rate, channels, settings.resolution);
        self.reported_latency = self.engine.latency_samples();
        context.set_latency_samples(self.reported_latency);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let settings = self.settings();
        self.engine.process(buffer.as_slice(), &settings);
        // Changing the resolution changes the latency.
        let latency = self.engine.latency_samples();
        if latency != self.reported_latency {
            self.reported_latency = latency;
            context.set_latency_samples(latency);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for ONr {
    const CLAP_ID: &'static str = "o-at.o-nr";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Open Noise Reduction with learned noise profiles");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Restoration,
    ];
}

impl Vst3Plugin for ONr {
    const VST3_CLASS_ID: [u8; 16] = *b"O-AT-O-NR-----01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Restoration];
}

export_plugin!(ONr);
