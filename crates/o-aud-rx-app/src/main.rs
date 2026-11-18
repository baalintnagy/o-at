//! Standalone O-AUD-RX: plays a named audio bus on any output device.
//!
//! Usage: `o-aud-rx [--bus NAME] [--device NAME]`. Without arguments, the bus and device
//! of the previous run are used.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod output;
mod settings;

use std::env;
use std::sync::{Arc, PoisonError};

use aud_ui::name_window::system_scale;
use aud_ui::window_icon;
use baseview::gl::GlConfig;
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::{EguiWindow, GraphicsConfig};
use o_aud_rx::link::receiver;
use o_aud_rx::telemetry::RxTelemetry;
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::app::{App, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::output::Output;
use crate::settings::AppSettings;

fn main() {
    enable_dpi_awareness();
    let settings = Arc::new(AppSettings::load());
    apply_arguments(&settings);

    let telemetry = Arc::new(RxTelemetry::default());
    // The output builds a receiver per stream, so this first one is not used.
    let (link, _unused) = receiver(Arc::clone(&settings.bus_name), Arc::clone(&telemetry));
    let link = Arc::new(link);
    let output = Output::start(Arc::clone(&link), Arc::clone(&settings));
    let app = App::new(Arc::clone(&settings), link, telemetry, output);

    let options = WindowOpenOptions {
        title: String::from("O-AUD-RX"),
        size: Size::new(f64::from(WINDOW_WIDTH), f64::from(WINDOW_HEIGHT)),
        scale: WindowScalePolicy::ScaleFactor(system_scale()),
        gl_config: Some(GlConfig::default()),
    };
    window_icon::apply_when_open("O-AUD-RX");
    EguiWindow::open_blocking(
        options,
        GraphicsConfig::default(),
        app,
        |context, _, _| App::setup(context),
        |context, _, app| app.update(context),
    );

    if let Err(error) = settings.save() {
        eprintln!("O-AUD-RX could not save its settings: {error}");
    }
}

/// Makes the process DPI aware, so windows are sized for the display. A plugin must not
/// change its host's awareness, which is why this is the application's job.
#[cfg(windows)]
fn enable_dpi_awareness() {
    // SAFETY: A plain Win32 call without pointers; baseview requests the same awareness.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(not(windows))]
fn enable_dpi_awareness() {}

/// Applies `--bus NAME` and `--device NAME` from the command line.
fn apply_arguments(settings: &AppSettings) {
    let arguments: Vec<String> = env::args().skip(1).collect();
    for pair in arguments.windows(2) {
        match pair[0].as_str() {
            "--bus" => {
                *settings
                    .bus_name
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = pair[1].clone();
            }
            "--device" => settings.set_device(Some(pair[1].clone())),
            _ => {}
        }
    }
}
