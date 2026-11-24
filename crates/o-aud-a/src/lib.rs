//! O-AUD-A: a VST3 and CLAP audio analyzer with a detailed spectrum, a sonogram, a stereo
//! imager, a loudness meter and 3D spectrum and stereo field views.
//!
//! The plugin passes its audio through unchanged. [`capture`] copies the input into a
//! lock-free ring; the [`analysis`] worker measures it off the audio thread and the
//! [`view`] draws the results.

pub mod analysis;
pub mod capture;
pub mod layout;
pub mod platforms;
pub mod settings;
pub mod sources;
pub mod style;
pub mod view;
pub mod window;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use aud_ui::export_plugin;
use aud_ui::theme;
use egui::{CentralPanel, Frame};
use nih_plug::prelude::{
    AsyncExecutor, AudioIOLayout, AuxiliaryBuffers, Buffer, BufferConfig, ClapFeature, ClapPlugin,
    Editor, InitContext, MidiConfig, Params, Plugin, PortNames, ProcessContext, ProcessStatus,
    Vst3Plugin, Vst3SubCategory,
};

use crate::analysis::Analyzer;
use crate::capture::{Capture, capture};
use crate::settings::AnalyzerSettings;
use crate::sources::SourceList;
use crate::view::{MIN_HEIGHT, MIN_WIDTH, VIEW_HEIGHT, VIEW_WIDTH, ViewState};
use crate::window::{EguiEditor, WindowState};

/// Silences the output, for the standalone application, which would otherwise play its
/// input back.
static SILENT_OUTPUT: AtomicBool = AtomicBool::new(false);

/// Makes every instance output silence instead of its input.
pub fn set_silent_output(silent: bool) {
    SILENT_OUTPUT.store(silent, Ordering::Relaxed);
}

/// Stored plugin state. The analyzer has no automatable parameters; persist keys are
/// stored in sessions, never rename them.
#[derive(Params)]
pub struct AnalyzerParams {
    /// The editor size, stored as `nih_plug_egui` stored it.
    #[persist = "editor-state"]
    pub editor_state: Arc<WindowState>,
    /// View and analysis settings.
    #[persist = "settings"]
    pub settings: Arc<RwLock<AnalyzerSettings>>,
    /// The buses analyzed next to the input.
    #[persist = "sources"]
    pub sources: Arc<RwLock<SourceList>>,
}

impl Default for AnalyzerParams {
    fn default() -> Self {
        Self {
            editor_state: WindowState::new(
                (VIEW_WIDTH as u32, VIEW_HEIGHT as u32),
                (MIN_WIDTH as u32, MIN_HEIGHT as u32),
            ),
            settings: Arc::new(RwLock::new(AnalyzerSettings::default())),
            sources: Arc::new(RwLock::new(SourceList::default())),
        }
    }
}

pub struct OAudA {
    params: Arc<AnalyzerParams>,
    capture: Capture,
    analyzer: Analyzer,
}

impl Default for OAudA {
    fn default() -> Self {
        let params = Arc::new(AnalyzerParams::default());
        let (capture, consumer, stream) = capture();
        let analyzer = Analyzer::start(
            consumer,
            stream,
            Arc::clone(&params.settings),
            Arc::clone(&params.sources),
        );
        Self {
            params,
            capture,
            analyzer,
        }
    }
}

impl Plugin for OAudA {
    const NAME: &'static str = "O-AUD-A";
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
        let shared = Arc::clone(self.analyzer.shared());
        let window = Arc::clone(&self.params.editor_state);
        let state = Mutex::new(ViewState::default());
        Some(Box::new(EguiEditor::new(
            Arc::clone(&self.params.editor_state),
            "O-AUD-A",
            theme::apply,
            move |context| {
                let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
                CentralPanel::default()
                    .frame(Frame::NONE.fill(theme::BACKGROUND))
                    .show(context, |ui| {
                        let rect = ui.max_rect();
                        view::draw(ui, rect, &mut state, &shared, &window);
                    });
            },
        )))
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let channels = audio_io_layout
            .main_input_channels
            .map_or(2, NonZeroU32::get);
        self.capture
            .set_format(buffer_config.sample_rate.round() as u32, channels);
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let channels = buffer.as_slice();
        self.capture.push(channels);
        if SILENT_OUTPUT.load(Ordering::Relaxed) {
            channels.iter_mut().for_each(|channel| channel.fill(0.0));
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for OAudA {
    const CLAP_ID: &'static str = "o-at.o-aud-a";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Spectrum, sonogram, stereo, loudness and 3D audio analyzer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Analyzer,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for OAudA {
    const VST3_CLASS_ID: [u8; 16] = *b"O-AT-O-AUD-A--01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

export_plugin!(OAudA);
