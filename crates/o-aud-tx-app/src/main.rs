//! Standalone O-AUD-TX: sends an audio input, or everything an output device plays, to a
//! named audio bus.
//!
//! Usage: `o-aud-tx [--bus NAME] [--input NAME | --loopback NAME]`. `--input` sends an
//! input device; `--loopback` captures an output device. The device name `default`
//! follows the system default. Without arguments, the bus and source of the previous run
//! are used.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod capture;
mod settings;

use std::env;
use std::sync::{Arc, PoisonError};

use aud_ui::name_window::system_scale;
use aud_ui::window_icon;
use baseview::gl::GlConfig;
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::{EguiWindow, GraphicsConfig};
use o_aud_tx::link::transmitter;
use o_aud_tx::telemetry::TxTelemetry;
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::app::{App, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::capture::Capture;
use crate::settings::{AppSettings, Source};

fn main() {
    enable_dpi_awareness();
    let settings = Arc::new(AppSettings::load());
    apply_arguments(&settings);

    let telemetry = Arc::new(TxTelemetry::default());
    // The capture takes a port per stream, so this first one is not used.
    let (link, _unused) = transmitter(Arc::clone(&settings.bus_name), Arc::clone(&telemetry));
    let link = Arc::new(link);
    let capture = Capture::start(Arc::clone(&link), Arc::clone(&settings));
    let app = App::new(Arc::clone(&settings), link, telemetry, capture);

    let options = WindowOpenOptions {
        title: String::from("O-AUD-TX"),
        size: Size::new(f64::from(WINDOW_WIDTH), f64::from(WINDOW_HEIGHT)),
        scale: WindowScalePolicy::ScaleFactor(system_scale()),
        gl_config: Some(GlConfig::default()),
    };
    window_icon::apply_when_open("O-AUD-TX");
    EguiWindow::open_blocking(
        options,
        GraphicsConfig::default(),
        app,
        |context, _, _| App::setup(context),
        |context, _, app| app.update(context),
    );

    if let Err(error) = settings.save() {
        eprintln!("O-AUD-TX could not save its settings: {error}");
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

/// Applies `--bus NAME`, `--input NAME` and `--loopback NAME` from the command line.
fn apply_arguments(settings: &AppSettings) {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let device = |name: &str| (!name.eq_ignore_ascii_case("default")).then(|| name.to_owned());
    for pair in arguments.windows(2) {
        match pair[0].as_str() {
            "--bus" => {
                *settings
                    .bus_name
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = pair[1].clone();
            }
            "--input" => settings.set_source(Source {
                loopback: false,
                device: device(&pair[1]),
            }),
            "--loopback" => settings.set_source(Source {
                loopback: true,
                device: device(&pair[1]),
            }),
            _ => {}
        }
    }
}
