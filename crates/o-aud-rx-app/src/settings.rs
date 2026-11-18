//! Application settings: the receiver controls and the output device, saved between
//! runs in a small text file.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use aud_ui::control::{Display, StoredSwitch, StoredValue, Switch, ValueRange, skew_factor};
use o_aud_rx::settings::{
    GAIN_MAX_DB, GAIN_MIN_DB, LATENCY_DEFAULT_MS, LATENCY_MAX_MS, LATENCY_MIN_MS, LATENCY_SKEW,
    RxSettings, db_to_gain,
};

const FILE_NAME: &str = "settings.ini";

/// Settings shared by the editor and the audio callback. All fields are thread-safe.
pub struct AppSettings {
    pub latency_ms: StoredValue,
    pub gain_db: StoredValue,
    pub mute: StoredSwitch,
    /// The received bus, as entered; shared with the receiver link.
    pub bus_name: Arc<RwLock<String>>,
    /// Output device name; `None` follows the system default.
    pub device: RwLock<Option<String>>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            latency_ms: StoredValue::new(
                "latency",
                ValueRange::Skewed {
                    min: LATENCY_MIN_MS,
                    max: LATENCY_MAX_MS,
                    factor: skew_factor(LATENCY_SKEW),
                },
                LATENCY_DEFAULT_MS,
                Display {
                    unit: " ms",
                    decimals: 1,
                },
            ),
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
            mute: StoredSwitch::new("mute", false),
            bus_name: Arc::new(RwLock::new(String::new())),
            device: RwLock::new(None),
        }
    }
}

impl AppSettings {
    /// Settings for the audio thread. Real-time safe.
    pub fn receiver_settings(&self) -> RxSettings {
        RxSettings {
            gain: db_to_gain(self.gain_db.value()),
            mute: self.mute.is_on(),
            latency_ms: self.latency_ms.value(),
        }
    }

    pub fn device(&self) -> Option<String> {
        self.device
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_device(&self, device: Option<String>) {
        *self.device.write().unwrap_or_else(PoisonError::into_inner) = device;
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
        let number = |key: &str| values.get(key).and_then(|value| value.parse::<f32>().ok());
        if let Some(latency) = number("latency_ms") {
            settings.latency_ms.set_value(latency);
        }
        if let Some(gain) = number("gain_db") {
            settings.gain_db.set_value(gain);
        }
        if let Some(mute) = values.get("mute") {
            settings.mute.set_on(*mute == "true");
        }
        if let Some(bus) = values.get("bus") {
            *settings
                .bus_name
                .write()
                .unwrap_or_else(PoisonError::into_inner) = (*bus).to_owned();
        }
        settings.set_device(values.get("device").map(|device| (*device).to_owned()));
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
        let mut text = format!(
            "bus = {bus}\nlatency_ms = {}\ngain_db = {}\nmute = {}\n",
            round_to_hundredths(self.latency_ms.value()),
            round_to_hundredths(self.gain_db.value()),
            self.mute.is_on()
        );
        if let Some(device) = self.device() {
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
        env::var_os("APPDATA").map(|base| PathBuf::from(base).join("O-AUD-RX"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|base| base.join("o-aud-rx"))
    };
    directory.map(|directory| directory.join(FILE_NAME))
}
