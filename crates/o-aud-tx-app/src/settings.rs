//! Application settings: the routes, each with its send controls, bus and audio source,
//! saved between runs in a small text file.
//!
//! The file holds a `selected = N` line, then one `[route]` section per route. A file of
//! an earlier version, without sections, holds a single route.

use std::collections::HashMap;
use std::env;
use std::fmt::Write;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use aud_ui::control::{Display, StoredSwitch, StoredValue, Switch, ValueRange};
use o_aud_tx::settings::{GAIN_MAX_DB, GAIN_MIN_DB, db_to_gain};

const FILE_NAME: &str = "settings.ini";
/// Starts a route's section in the settings file.
const ROUTE_HEADER: &str = "[route]";

/// Where the sent audio comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// An input device; `None` follows the system default input.
    Input(Option<String>),
    /// What an output device plays (WASAPI loopback); `None` follows the system default
    /// output.
    Loopback(Option<String>),
    /// What one application and its child processes play (WASAPI process loopback),
    /// named by its executable, for example `firefox.exe`. Windows only.
    Application(String),
}

impl Default for Source {
    fn default() -> Self {
        Self::Input(None)
    }
}

impl Source {
    /// The value of the `source` key in the settings file.
    fn kind(&self) -> &'static str {
        match self {
            Self::Input(_) => "input",
            Self::Loopback(_) => "loopback",
            Self::Application(_) => "application",
        }
    }

    /// The device or executable name, the value of the `device` key.
    fn name(&self) -> Option<&str> {
        match self {
            Self::Input(device) | Self::Loopback(device) => device.as_deref(),
            Self::Application(executable) => Some(executable),
        }
    }

    /// Parses the `source` and `device` keys; unknown kinds are an input.
    fn parse(kind: Option<&str>, name: Option<&str>) -> Self {
        let device = name.map(str::to_owned);
        match (kind, name) {
            (Some("loopback"), _) => Self::Loopback(device),
            (Some("application"), Some(executable)) => Self::Application(executable.to_owned()),
            _ => Self::Input(device),
        }
    }
}

/// One route's settings, shared by the editor, the link and the audio callback. All
/// fields are thread-safe.
pub struct RouteSettings {
    pub gain_db: StoredValue,
    pub send: StoredSwitch,
    /// The sent bus, as entered; shared with the transmitter link.
    pub bus_name: Arc<RwLock<String>>,
    source: RwLock<Source>,
}

impl Default for RouteSettings {
    fn default() -> Self {
        Self {
            gain_db: StoredValue::new(
                "gain",
                ValueRange::Linear {
                    min: GAIN_MIN_DB,
                    max: GAIN_MAX_DB,
                },
                0.0,
                Display {
                    unit: " dB",
                    decimals: 1,
                },
            ),
            send: StoredSwitch::new("send", true),
            bus_name: Arc::new(RwLock::new(String::new())),
            source: RwLock::new(Source::default()),
        }
    }
}

impl RouteSettings {
    /// The send gain as a linear factor; zero while sending is off. Real-time safe.
    pub fn gain(&self) -> f32 {
        if self.send.is_on() {
            db_to_gain(self.gain_db.value())
        } else {
            0.0
        }
    }

    pub fn source(&self) -> Source {
        self.source
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_source(&self, source: Source) {
        *self.source.write().unwrap_or_else(PoisonError::into_inner) = source;
    }

    pub fn bus(&self) -> String {
        self.bus_name
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_bus(&self, name: &str) {
        *self
            .bus_name
            .write()
            .unwrap_or_else(PoisonError::into_inner) = name.to_owned();
    }

    /// A route from one section's keys; missing or unreadable values keep their
    /// defaults.
    fn from_values(values: &HashMap<&str, &str>) -> Self {
        let settings = Self::default();
        if let Some(gain) = values.get("gain_db").and_then(|value| value.parse().ok()) {
            settings.gain_db.set_value(gain);
        }
        if let Some(send) = values.get("send") {
            settings.send.set_on(*send == "true");
        }
        if let Some(bus) = values.get("bus") {
            settings.set_bus(bus);
        }
        settings.set_source(Source::parse(
            values.get("source").copied(),
            values.get("device").copied(),
        ));
        settings
    }

    /// Appends this route's keys.
    fn write_values(&self, text: &mut String) {
        let source = self.source();
        // Writing to a String cannot fail.
        let _ = write!(
            text,
            "bus = {}\ngain_db = {}\nsend = {}\nsource = {}\n",
            self.bus(),
            round_to_hundredths(self.gain_db.value()),
            self.send.is_on(),
            source.kind()
        );
        if let Some(device) = source.name() {
            let _ = writeln!(text, "device = {device}");
        }
    }
}

/// The saved routes and the one shown in the editor.
pub struct SavedRoutes {
    /// At least one route.
    pub routes: Vec<RouteSettings>,
    /// Index into `routes`.
    pub selected: usize,
}

impl SavedRoutes {
    /// Loads the saved routes; without a readable file, one default route.
    pub fn load() -> Self {
        let text = path()
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_default();
        Self::parse(&text)
    }

    fn parse(text: &str) -> Self {
        let mut header: HashMap<&str, &str> = HashMap::new();
        let mut sections: Vec<HashMap<&str, &str>> = Vec::new();
        for line in text.lines().map(str::trim) {
            if line == ROUTE_HEADER {
                sections.push(HashMap::new());
            } else if let Some((key, value)) = line.split_once('=') {
                let values = sections.last_mut().unwrap_or(&mut header);
                values.insert(key.trim(), value.trim());
            }
        }
        let routes: Vec<RouteSettings> = if sections.is_empty() {
            // An earlier version's file, or none: its keys are the only route.
            vec![RouteSettings::from_values(&header)]
        } else {
            sections.iter().map(RouteSettings::from_values).collect()
        };
        let selected = header
            .get("selected")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
            .min(routes.len() - 1);
        Self { routes, selected }
    }
}

/// The settings file's text for `routes`, with `selected` shown in the editor.
fn file_text<'a>(routes: impl IntoIterator<Item = &'a RouteSettings>, selected: usize) -> String {
    let mut text = format!("selected = {selected}\n");
    for route in routes {
        text.push('\n');
        text.push_str(ROUTE_HEADER);
        text.push('\n');
        route.write_values(&mut text);
    }
    text
}

/// Saves `routes` for the next run.
pub fn save<'a>(
    routes: impl IntoIterator<Item = &'a RouteSettings>,
    selected: usize,
) -> io::Result<()> {
    let path = path().ok_or_else(|| io::Error::other("no configuration directory"))?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(path, file_text(routes, selected))
}

/// Rounds away float noise from range conversions, for a readable settings file.
fn round_to_hundredths(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
}

/// Location of the settings file in the user's configuration directory.
fn path() -> Option<PathBuf> {
    let directory = if cfg!(windows) {
        env::var_os("APPDATA").map(|base| PathBuf::from(base).join("O-AUD-TX"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|base| base.join("o-aud-tx"))
    };
    directory.map(|directory| directory.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use aud_ui::control::Switch;

    use super::{RouteSettings, SavedRoutes, Source, file_text};

    #[test]
    fn sources_round_trip_through_their_keys() {
        let sources = [
            Source::Input(None),
            Source::Input(Some(String::from("Microphone"))),
            Source::Loopback(None),
            Source::Loopback(Some(String::from("Speakers"))),
            Source::Application(String::from("firefox.exe")),
        ];
        for source in sources {
            assert_eq!(Source::parse(Some(source.kind()), source.name()), source);
        }
        // Files of earlier versions and damaged ones still give a source.
        assert_eq!(Source::parse(None, None), Source::Input(None));
        assert_eq!(
            Source::parse(Some("application"), None),
            Source::Input(None)
        );
    }

    #[test]
    fn a_file_without_sections_is_one_route() {
        let saved = SavedRoutes::parse(
            "bus = Studio\ngain_db = -3\nsend = false\nsource = loopback\ndevice = Speakers\n",
        );
        assert_eq!(saved.routes.len(), 1);
        assert_eq!(saved.selected, 0);
        let route = &saved.routes[0];
        assert_eq!(route.bus(), "Studio");
        assert_eq!(route.gain_db.value(), -3.0);
        assert!(!route.send.is_on());
        assert_eq!(
            route.source(),
            Source::Loopback(Some(String::from("Speakers")))
        );
    }

    #[test]
    fn no_file_is_one_default_route() {
        let saved = SavedRoutes::parse("");
        assert_eq!(saved.routes.len(), 1);
        assert_eq!(saved.routes[0].source(), Source::Input(None));
    }

    #[test]
    fn routes_round_trip_through_the_file() {
        let routes = [
            (Source::Application(String::from("brave.exe")), "Bus 1"),
            (Source::Input(Some(String::from("Microphone"))), "Bus 2"),
            (Source::Application(String::from("vlc.exe")), "Bus 3"),
        ]
        .map(|(source, bus)| {
            let route = RouteSettings::default();
            route.set_source(source);
            route.set_bus(bus);
            route
        });
        routes[1].gain_db.set_value(6.5);
        let saved = SavedRoutes::parse(&file_text(&routes, 2));
        assert_eq!(saved.selected, 2);
        assert_eq!(saved.routes.len(), 3);
        for (loaded, route) in saved.routes.iter().zip(&routes) {
            assert_eq!(loaded.bus(), route.bus());
            assert_eq!(loaded.source(), route.source());
            assert_eq!(loaded.gain_db.value(), route.gain_db.value());
        }
    }

    #[test]
    fn a_selection_past_the_routes_is_clamped() {
        let saved = SavedRoutes::parse("selected = 7\n\n[route]\nbus = A\n\n[route]\nbus = B\n");
        assert_eq!(saved.selected, 1);
    }
}
