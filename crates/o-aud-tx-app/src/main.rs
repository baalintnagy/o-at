//! Standalone O-AUD-TX: sends audio inputs, what output devices play, or what single
//! applications play, each to a named audio bus. Every source and bus pair is a route;
//! one window runs any number of routes.
//!
//! Usage: `o-aud-tx [--bus NAME] [--input NAME | --loopback NAME | --application EXE] ...`.
//! `--input` sends an input device; `--loopback` captures an output device. The device
//! name `default` follows the system default. `--application` captures one application
//! and its child processes by executable name, for example `firefox.exe` (Windows only).
//! A bus and a source make one route; repeat them for more routes, for example
//! `--bus Browser --application brave.exe --bus Voice --input default`. The command line
//! sets the saved routes in order and adds routes as needed. Without arguments, the
//! routes of the previous run are used.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod application;
mod capture;
mod route;
mod settings;

use std::env;
use std::sync::{Arc, Mutex, PoisonError};

use aud_ui::name_window::system_scale;
use aud_ui::window_icon;
use baseview::gl::GlConfig;
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::{EguiWindow, GraphicsConfig};
#[cfg(windows)]
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

use crate::app::{App, Snapshot, WINDOW_HEIGHT, WINDOW_WIDTH};
use crate::settings::{RouteSettings, SavedRoutes, Source, save};

fn main() {
    enable_dpi_awareness();
    let saved = SavedRoutes::load();
    let mut routes = saved.routes;
    apply_arguments(&mut routes, parse_arguments(env::args().skip(1)));
    let snapshot = Arc::new(Mutex::new(Snapshot {
        routes: routes.into_iter().map(Arc::new).collect(),
        selected: saved.selected,
    }));
    let app = App::new(Arc::clone(&snapshot));

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

    let snapshot = snapshot.lock().unwrap_or_else(PoisonError::into_inner);
    let routes = snapshot.routes.iter().map(|route| &**route);
    if let Err(error) = save(routes, snapshot.selected) {
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

/// One route as given on the command line.
#[derive(Debug, Default, PartialEq, Eq)]
struct RouteArguments {
    bus: Option<String>,
    source: Option<Source>,
}

/// Groups `--bus NAME`, `--input NAME`, `--loopback NAME` and `--application EXE` into
/// routes: a second bus or a second source starts the next route.
fn parse_arguments(arguments: impl IntoIterator<Item = String>) -> Vec<RouteArguments> {
    let mut routes = Vec::new();
    let mut current = RouteArguments::default();
    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        if !matches!(
            flag.as_str(),
            "--bus" | "--input" | "--loopback" | "--application"
        ) {
            continue;
        }
        let Some(value) = arguments.next() else {
            break;
        };
        let taken = if flag == "--bus" {
            current.bus.is_some()
        } else {
            current.source.is_some()
        };
        if taken {
            routes.push(current);
            current = RouteArguments::default();
        }
        match flag.as_str() {
            "--bus" => current.bus = Some(value),
            "--input" => current.source = Some(Source::Input(device(value))),
            "--loopback" => current.source = Some(Source::Loopback(device(value))),
            _ => current.source = Some(Source::Application(value)),
        }
    }
    if current != RouteArguments::default() {
        routes.push(current);
    }
    routes
}

/// A device name from the command line; `default` follows the system default.
fn device(name: String) -> Option<String> {
    (!name.eq_ignore_ascii_case("default")).then_some(name)
}

/// Applies the command line's routes to the saved ones in order, adding routes as needed.
fn apply_arguments(routes: &mut Vec<RouteSettings>, arguments: Vec<RouteArguments>) {
    for (index, arguments) in arguments.into_iter().enumerate() {
        if index == routes.len() {
            routes.push(RouteSettings::default());
        }
        let route = &routes[index];
        if let Some(bus) = arguments.bus {
            route.set_bus(&bus);
        }
        if let Some(source) = arguments.source {
            route.set_source(source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RouteArguments, apply_arguments, parse_arguments};
    use crate::settings::{RouteSettings, Source};

    fn parse(line: &str) -> Vec<RouteArguments> {
        parse_arguments(line.split(' ').map(str::to_owned))
    }

    fn route(bus: Option<&str>, source: Option<Source>) -> RouteArguments {
        RouteArguments {
            bus: bus.map(str::to_owned),
            source,
        }
    }

    #[test]
    fn one_bus_and_source_are_one_route() {
        assert_eq!(
            parse("--bus Studio --loopback default"),
            [route(Some("Studio"), Some(Source::Loopback(None)))]
        );
        // The order within a route does not matter.
        assert_eq!(
            parse("--input Mic --bus Voice"),
            [route(
                Some("Voice"),
                Some(Source::Input(Some(String::from("Mic"))))
            )]
        );
    }

    #[test]
    fn repeated_flags_start_routes() {
        let application = |name: &str| Some(Source::Application(name.to_owned()));
        assert_eq!(
            parse(
                "--bus B1 --application brave.exe --bus B2 --input Mic --bus B3 --application vlc.exe"
            ),
            [
                route(Some("B1"), application("brave.exe")),
                route(Some("B2"), Some(Source::Input(Some(String::from("Mic"))))),
                route(Some("B3"), application("vlc.exe")),
            ]
        );
        assert_eq!(
            parse("--application a.exe --application b.exe"),
            [
                route(None, application("a.exe")),
                route(None, application("b.exe"))
            ]
        );
    }

    #[test]
    fn unknown_flags_and_a_missing_value_are_ignored() {
        assert_eq!(parse("--verbose --bus"), []);
    }

    #[test]
    fn arguments_set_saved_routes_and_add_new_ones() {
        let mut routes = vec![RouteSettings::default()];
        routes[0].set_bus("Saved");
        apply_arguments(
            &mut routes,
            vec![
                route(None, Some(Source::Loopback(None))),
                route(Some("Second"), None),
            ],
        );
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].bus(), "Saved");
        assert_eq!(routes[0].source(), Source::Loopback(None));
        assert_eq!(routes[1].bus(), "Second");
        assert_eq!(routes[1].source(), Source::Input(None));
    }
}
