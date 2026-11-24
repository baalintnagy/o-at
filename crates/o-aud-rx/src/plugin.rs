//! The O-AUD-RX VST3 and CLAP plugin: plays a bus inside a host, mixed with the track's
//! input.

use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};

use aud_ui::export_plugin;
use aud_ui::param::{ParamControl, ParamSwitch};
use aud_ui::theme;
use nih_plug::prelude::{
    AsyncExecutor, AudioIOLayout, AuxiliaryBuffers, BoolParam, Buffer, BufferConfig, ClapFeature,
    ClapPlugin, Editor, FloatParam, FloatRange, InitContext, MidiConfig, Params, Plugin, PortNames,
    ProcessContext, ProcessStatus, Vst3Plugin, Vst3SubCategory, formatters,
};
use nih_plug_egui::egui::{CentralPanel, Frame};
use nih_plug_egui::{EguiState, create_egui_editor};

use crate::engine::Receiver;
use crate::link::{RxLink, receiver};
use crate::settings::{
    GAIN_MAX_DB, GAIN_MIN_DB, LATENCY_DEFAULT_MS, LATENCY_MAX_MS, LATENCY_MIN_MS, LATENCY_SKEW,
    RxSettings, db_to_gain,
};
use crate::telemetry::RxTelemetry;
use crate::view::{self, RxControls, RxViewState, VIEW_HEIGHT, VIEW_WIDTH};

/// Host-visible parameters. IDs and persist keys are stored in sessions; never rename them.
#[derive(Params)]
pub struct RxParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// The received bus, as entered.
    #[persist = "bus-name"]
    pub bus_name: Arc<RwLock<String>>,

    #[id = "latency"]
    pub latency_ms: FloatParam,
    #[id = "gain"]
    pub gain_db: FloatParam,
    #[id = "mute"]
    pub mute: BoolParam,
    /// Passes the track's input through, mixed with the bus.
    #[id = "thru"]
    pub input_thru: BoolParam,
}

impl Default for RxParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(VIEW_WIDTH as u32, VIEW_HEIGHT as u32),
            bus_name: Arc::new(RwLock::new(String::new())),
            latency_ms: FloatParam::new(
                "Latency",
                LATENCY_DEFAULT_MS,
                FloatRange::Skewed {
                    min: LATENCY_MIN_MS,
                    max: LATENCY_MAX_MS,
                    factor: FloatRange::skew_factor(LATENCY_SKEW),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            gain_db: FloatParam::new(
                "Gain",
                0.0,
                FloatRange::Linear {
                    min: GAIN_MIN_DB,
                    max: GAIN_MAX_DB,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            mute: BoolParam::new("Mute", false),
            input_thru: BoolParam::new("Input Thru", true),
        }
    }
}

pub struct OAudRx {
    params: Arc<RxParams>,
    link: Arc<RxLink>,
    receiver: Receiver,
    telemetry: Arc<RxTelemetry>,
    /// Bus audio per output channel, sized in `initialize`.
    scratch: Vec<Vec<f32>>,
}

impl Default for OAudRx {
    fn default() -> Self {
        let params = Arc::new(RxParams::default());
        let telemetry = Arc::new(RxTelemetry::default());
        let (link, receiver) = receiver(Arc::clone(&params.bus_name), Arc::clone(&telemetry));
        Self {
            params,
            link: Arc::new(link),
            receiver,
            telemetry,
            scratch: Vec::new(),
        }
    }
}

impl OAudRx {
    fn settings(&self) -> RxSettings {
        RxSettings {
            gain: db_to_gain(self.params.gain_db.value()),
            mute: self.params.mute.value(),
            latency_ms: self.params.latency_ms.value(),
        }
    }
}

impl Plugin for OAudRx {
    const NAME: &'static str = "O-AUD-RX";
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
        let params = Arc::clone(&self.params);
        let link = Arc::clone(&self.link);
        let telemetry = Arc::clone(&self.telemetry);
        create_egui_editor(
            self.params.editor_state.clone(),
            RxViewState::default(),
            |context, _| theme::apply(context),
            move |context, setter, state| {
                let latency = ParamControl::new(&params.latency_ms, setter);
                let gain = ParamControl::new(&params.gain_db, setter);
                let mute = ParamSwitch::new(&params.mute, setter);
                let controls = RxControls {
                    latency: &latency,
                    gain: &gain,
                    mute: &mute,
                };
                CentralPanel::default()
                    .frame(Frame::NONE.fill(theme::BACKGROUND))
                    .show(context, |ui| {
                        let rect = ui.max_rect();
                        view::draw(ui, rect, state, &controls, &link, &telemetry);
                    });
                context.request_repaint();
            },
        )
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let rate = buffer_config.sample_rate.round() as u32;
        let channels = audio_io_layout
            .main_output_channels
            .map_or(2, NonZeroU32::get) as usize;
        let max_block = buffer_config.max_buffer_size.max(1) as usize;
        self.scratch = vec![vec![0.0; max_block]; channels];
        self.receiver.set_device_rate(rate);
        self.link.set_device_rate(rate);
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let settings = self.settings();
        let thru = self.params.input_thru.value();
        let capacity = self.scratch.first().map_or(0, Vec::len);
        if capacity == 0 {
            return ProcessStatus::Normal;
        }
        let total = buffer.samples();
        let used = self.scratch.len().min(2);
        let channels = buffer.as_slice();
        let mut offset = 0;
        while offset < total {
            let count = (total - offset).min(capacity);
            let mut scratch: [&mut [f32]; 2] = [&mut [], &mut []];
            for (slot, channel) in scratch.iter_mut().zip(self.scratch.iter_mut()) {
                *slot = &mut channel[..count];
            }
            self.receiver.process(&mut scratch[..used], &settings);
            for (output, bus) in channels.iter_mut().zip(&scratch[..used]) {
                let output = &mut output[offset..offset + count];
                if thru {
                    output
                        .iter_mut()
                        .zip(bus.iter())
                        .for_each(|(out, sample)| *out += sample);
                } else {
                    output.copy_from_slice(bus);
                }
            }
            offset += count;
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for OAudRx {
    const CLAP_ID: &'static str = "o-at.o-aud-rx";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Plays a named O-AUD-TX audio bus");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for OAudRx {
    const VST3_CLASS_ID: [u8; 16] = *b"O-AT-O-AUD-RX-01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

export_plugin!(OAudRx);
