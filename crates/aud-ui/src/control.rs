//! What the widgets edit: abstractions over plugin parameters and application settings.
//!
//! Widgets depend on [`Control`] and [`Switch`] only, so the same editor code drives
//! NIH-plug parameters in a plugin (see [`crate::param`]) and [`StoredValue`] settings
//! in a standalone application.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// A continuous value edited through its normalized 0..1 position.
pub trait Control {
    /// Stable identity, unique within an editor.
    fn key(&self) -> &str;
    fn normalized(&self) -> f32;
    fn default_normalized(&self) -> f32;
    /// Display text with unit for a normalized position.
    fn format(&self, normalized: f32) -> String;
    /// Starts a gesture, such as a drag, that ends with [`end_edit`](Self::end_edit).
    fn begin_edit(&self);
    fn set_normalized(&self, normalized: f32);
    fn end_edit(&self);
}

/// An on/off value.
pub trait Switch {
    /// Stable identity, unique within an editor.
    fn key(&self) -> &str;
    fn is_on(&self) -> bool;
    fn set_on(&self, on: bool);
}

/// Mapping between a plain value and its normalized position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueRange {
    Linear {
        min: f32,
        max: f32,
    },
    /// `factor` below one gives more resolution to low values; see [`skew_factor`].
    Skewed {
        min: f32,
        max: f32,
        factor: f32,
    },
}

/// Skew factor with the meaning of NIH-plug's `FloatRange::skew_factor`, so plugin and
/// application ranges agree.
pub fn skew_factor(skew: f32) -> f32 {
    2.0_f32.powf(skew)
}

impl ValueRange {
    pub fn normalize(&self, plain: f32) -> f32 {
        match *self {
            Self::Linear { min, max } => ((plain - min) / (max - min)).clamp(0.0, 1.0),
            Self::Skewed { min, max, factor } => {
                ((plain - min) / (max - min)).clamp(0.0, 1.0).powf(factor)
            }
        }
    }

    pub fn unnormalize(&self, normalized: f32) -> f32 {
        let normalized = normalized.clamp(0.0, 1.0);
        match *self {
            Self::Linear { min, max } => min + normalized * (max - min),
            Self::Skewed { min, max, factor } => {
                min + normalized.powf(factor.recip()) * (max - min)
            }
        }
    }
}

/// How a [`StoredValue`] is displayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Display {
    pub unit: &'static str,
    pub decimals: usize,
}

/// A thread-safe setting with a range, for applications without a plugin host.
pub struct StoredValue {
    key: &'static str,
    range: ValueRange,
    default: f32,
    display: Display,
    bits: AtomicU32,
}

impl StoredValue {
    pub fn new(key: &'static str, range: ValueRange, default: f32, display: Display) -> Self {
        Self {
            key,
            range,
            default,
            display,
            bits: AtomicU32::new(default.to_bits()),
        }
    }

    pub fn value(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    /// Sets the plain value, limited to the range.
    pub fn set_value(&self, value: f32) {
        let value = self.range.unnormalize(self.range.normalize(value));
        self.bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

impl Control for StoredValue {
    fn key(&self) -> &str {
        self.key
    }

    fn normalized(&self) -> f32 {
        self.range.normalize(self.value())
    }

    fn default_normalized(&self) -> f32 {
        self.range.normalize(self.default)
    }

    fn format(&self, normalized: f32) -> String {
        let value = self.range.unnormalize(normalized);
        let Display { unit, decimals } = self.display;
        format!("{value:.decimals$}{unit}")
    }

    fn begin_edit(&self) {}

    fn set_normalized(&self, normalized: f32) {
        self.set_value(self.range.unnormalize(normalized));
    }

    fn end_edit(&self) {}
}

/// A thread-safe on/off setting.
pub struct StoredSwitch {
    key: &'static str,
    on: AtomicBool,
}

impl StoredSwitch {
    pub fn new(key: &'static str, on: bool) -> Self {
        Self {
            key,
            on: AtomicBool::new(on),
        }
    }
}

impl Switch for StoredSwitch {
    fn key(&self) -> &str {
        self.key
    }

    fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::{Control, Display, StoredValue, ValueRange, skew_factor};

    #[test]
    fn skewed_ranges_round_trip() {
        let range = ValueRange::Skewed {
            min: 2.0,
            max: 250.0,
            factor: skew_factor(-1.5),
        };
        for plain in [2.0, 10.0, 30.0, 250.0] {
            let back = range.unnormalize(range.normalize(plain));
            assert!((back - plain).abs() < 1e-3, "{plain} became {back}");
        }
        assert!(
            range.normalize(30.0) > 30.0 / 250.0,
            "low values get more travel"
        );
    }

    #[test]
    fn stored_values_clamp_and_format() {
        let gain = StoredValue::new(
            "gain",
            ValueRange::Linear {
                min: -24.0,
                max: 24.0,
            },
            0.0,
            Display {
                unit: " dB",
                decimals: 1,
            },
        );
        assert_eq!(gain.default_normalized(), 0.5);
        gain.set_value(40.0);
        assert_eq!(gain.value(), 24.0);
        assert_eq!(gain.format(0.25), "-12.0 dB");
    }
}
