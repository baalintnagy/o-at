//! Application settings: the send controls and the audio source, saved between runs in
//! a small text file.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use aud_ui::control::{Display, StoredSwitch, StoredValue, Switch, ValueRange};
use o_aud_tx::settings::{GAIN_MAX_DB, GAIN_MIN_DB, db_to_gain};

const FILE_NAME: &str = "settings.ini";

/// Where the sent audio comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// Captures what an output device plays (WASAPI loopback) instead of an input.
    pub loopback: bool,
    /// Device name; `None` follows the system default input or output.
    pub device: Option<String>,
}

impl Source {
    /// The value of the `source` key in the settings file.
    fn kind(&self) -> &'static str {
        if self.loopback { "loopback" } else { "input" }
    }
}

/// Settings shared by the editor and the audio callback. All fields are thread-safe.
pub struct AppSettings {
    pub gain_db: StoredValue,
    pub send: StoredSwitch,
    /// The sent bus, as entered; shared with the transmitter link.
    pub bus_name: Arc<RwLock<String>>,
    source: RwLock<Source>,
}

impl Default for AppSettings {
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

impl AppSettings {
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

    /// Loads the saved settings; missing or unreadable values keep their defaults.
    pub fn load() -> Self {
        let settings = Self::default();
        let Some(text) = path().and_then(|path| fs::read_to_string(path).ok()) else {
            return settings;
        };
        let values: HashMap<&str, &str> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim(), value.trim()))
            .collect();
        if let Some(gain) = values.get("gain_db").and_then(|value| value.parse().ok()) {
            settings.gain_db.set_value(gain);
        }
        if let Some(send) = values.get("send") {
            settings.send.set_on(*send == "true");
        }
        if let Some(bus) = values.get("bus") {
            *settings
                .bus_name
                .write()
                .unwrap_or_else(PoisonError::into_inner) = (*bus).to_owned();
        }
        settings.set_source(Source {
            loopback: values.get("source") == Some(&"loopback"),
            device: values.get("device").map(|device| (*device).to_owned()),
        });
        settings
    }

    /// Saves the settings for the next run.
    pub fn save(&self) -> io::Result<()> {
        let path = path().ok_or_else(|| io::Error::other("no configuration directory"))?;
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let bus = self
            .bus_name
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let source = self.source();
        let mut text = format!(
            "bus = {bus}\ngain_db = {}\nsend = {}\nsource = {}\n",
            round_to_hundredths(self.gain_db.value()),
            self.send.is_on(),
            source.kind()
        );
        if let Some(device) = source.device {
            text.push_str(&format!("device = {device}\n"));
        }
        fs::write(path, text)
    }
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
