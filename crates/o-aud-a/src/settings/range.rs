//! Ranges, defaults and slider tapers of continuous settings.

/// How a slider's position maps to its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taper {
    Linear,
    /// Logarithmic, like an audio fader: the lower values get most of the travel, which
    /// suits what the eye and the ear perceive. The value at mid travel is about 9 % of
    /// the range.
    Log,
}

/// Steepness of [`Taper::Log`]: the value spans `LOG_TAPER_BASE` to one.
const LOG_TAPER_BASE: f32 = 101.0;

/// A continuous setting's range, default and taper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub taper: Taper,
}

impl Range {
    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.default
        }
    }

    /// Slider position from 0 to 1 of `value`.
    pub fn position(&self, value: f32) -> f32 {
        let fraction = (self.clamp(value) - self.min) / (self.max - self.min);
        match self.taper {
            Taper::Linear => fraction,
            Taper::Log => (1.0 + (LOG_TAPER_BASE - 1.0) * fraction).ln() / LOG_TAPER_BASE.ln(),
        }
    }

    /// Value at slider `position` from 0 to 1.
    pub fn value(&self, position: f32) -> f32 {
        let position = position.clamp(0.0, 1.0);
        let fraction = match self.taper {
            Taper::Linear => position,
            Taper::Log => (LOG_TAPER_BASE.powf(position) - 1.0) / (LOG_TAPER_BASE - 1.0),
        };
        self.min + fraction * (self.max - self.min)
    }
}

/// Shown level range of the spectrum, sonogram and 3D views in dB.
pub const LEVEL_RANGE_DB: Range = Range {
    min: 30.0,
    max: 150.0,
    default: 90.0,
    taper: Taper::Linear,
};
