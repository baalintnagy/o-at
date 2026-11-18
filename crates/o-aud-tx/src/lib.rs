//! O-AUD-TX: a VST3 and CLAP plugin that sends a track's audio to a named audio bus,
//! where O-AUD-RX receivers in other hosts or the standalone O-AUD-RX application play it.

pub mod link;
pub mod port;
pub mod telemetry;
mod view;

use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};

use aud_ui::param::{ParamControl, ParamSwitch};
use aud_ui::theme;
use nih_plug::prelude::{
    AsyncExecutor, AudioIOLayout, AuxiliaryBuffers, BoolParam, Buffer, BufferConfig, ClapFeature,
    ClapPlugin, Editor, FloatParam, FloatRange, InitContext, MidiConfig, Params, Plugin, PortNames,
    ProcessContext, ProcessStatus, Vst3Plugin, Vst3SubCategory, formatters, nih_export_clap,
};
use nih_plug_egui::egui::{CentralPanel, Frame};
use nih_plug_egui::{EguiState, create_egui_editor};

use crate::link::{TxLink, transmitter};
use crate::port::TxPort;
use crate::telemetry::TxTelemetry;
use crate::view::{TxControls, TxViewState, VIEW_HEIGHT, VIEW_WIDTH};

/// Send gain range in dB.
pub const GAIN_MIN_DB: f32 = -48.0;
pub const GAIN_MAX_DB: f32 = 12.0;

/// Host-visible parameters. IDs and persist keys are stored in sessions; never rename them.
#[derive(Params)]
pub struct TxParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// The bus name as entered; empty until a free name is assigned.
    #[persist = "bus-name"]
    pub bus_name: Arc<RwLock<String>>,

    #[id = "gain"]
    pub gain_db: FloatParam,
    /// Sends to the bus; off sends silence, so receivers stay locked.
    #[id = "send"]
    pub send: BoolParam,
    /// Keeps the track's own output.
    #[id = "thru"]
    pub thru: BoolParam,
}

impl Default for TxParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(VIEW_WIDTH as u32, VIEW_HEIGHT as u32),
            bus_name: Arc::new(RwLock::new(String::new())),
            gain_db: FloatParam::new(
                "Send Gain",
                0.0,
                FloatRange::Linear {
                    min: GAIN_MIN_DB,
                    max: GAIN_MAX_DB,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            send: BoolParam::new("Send", true),
            thru: BoolParam::new("Thru", true),
        }
    }
}

pub struct OAudTx {
    params: Arc<TxParams>,
    link: Arc<TxLink>,
    port: TxPort,
    telemetry: Arc<TxTelemetry>,
}

impl Default for OAudTx {
    fn default() -> Self {
        let params = Arc::new(TxParams::default());
        let telemetry = Arc::new(TxTelemetry::default());
        let (link, port) = transmitter(Arc::clone(&params.bus_name), Arc::clone(&telemetry));
        Self {
            params,
            link: Arc::new(link),
            port,
            telemetry,
        }
    }
}

impl Plugin for OAudTx {
    const NAME: &'static str = "O-AUD-TX";
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
            TxViewState::default(),
            |context, _| theme::apply(context),
            move |context, setter, state| {
                let gain = ParamControl::new(&params.gain_db, setter);
                let send = ParamSwitch::new(&params.send, setter);
                let thru = ParamSwitch::new(&params.thru, setter);
                let controls = TxControls {
                    gain: &gain,
                    send: &send,
                    thru: &thru,
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
            .main_input_channels
            .map_or(2, NonZeroU32::get) as usize;
        self.port.set_format(rate, channels);
        self.link.set_format(rate, channels);
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let gain = if self.params.send.value() {
            db_to_gain(self.params.gain_db.value())
        } else {
            0.0
        };
        let channels = buffer.as_slice();
        self.port.process(channels, gain);
        if !self.params.thru.value() {
            channels.iter_mut().for_each(|channel| channel.fill(0.0));
        }
        ProcessStatus::Normal
    }
}

/// Converts decibels to a linear gain.
fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

impl ClapPlugin for OAudTx {
    const CLAP_ID: &'static str = "o-at.o-aud-tx";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Sends audio to a named O-AUD-RX bus");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for OAudTx {
    const VST3_CLASS_ID: [u8; 16] = *b"O-AT-O-AUD-TX-01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_clap!(OAudTx);
// The VST3 bindings' COM macros expand to a trailing semicolon in expression position.
#[allow(semicolon_in_expressions_from_non_local_macros)]
mod vst3_export {
    use super::OAudTx;
    use nih_plug::nih_export_vst3;

    nih_export_vst3!(OAudTx);
}
