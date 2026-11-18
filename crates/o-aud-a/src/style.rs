//! Diagram colors: a shared background and grid for every diagram, and colors tailored to
//! each view. The theme (accent of buttons, tabs and sliders) is separate and never
//! colors a diagram.
//!
//! Stored with the settings by field name: never rename fields.

use serde::{Deserialize, Serialize};

use crate::layout::Module;
use crate::settings::ColorMap;

/// An sRGB color.
pub type Rgb = [u8; 3];

/// `0xRRGGBB` as a color.
const fn hex(value: u32) -> Rgb {
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

/// A color with an opacity from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Paint {
    pub color: Rgb,
    pub opacity: f32,
}

impl Default for Paint {
    fn default() -> Self {
        paint(0xffffff, 1.0)
    }
}

const fn paint(color: u32, opacity: f32) -> Paint {
    Paint {
        color: hex(color),
        opacity,
    }
}

/// A heat map: a built-in one, or the three custom stops from quiet to loud.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeatStyle {
    pub map: ColorMap,
    pub stops: [Rgb; 3],
}

impl Default for HeatStyle {
    fn default() -> Self {
        Self {
            map: ColorMap::Ember,
            stops: [hex(0x0b0d12), hex(0xd93f5b), hex(0xfcf3a6)],
        }
    }
}

const fn heat(map: ColorMap) -> HeatStyle {
    HeatStyle {
        map,
        stops: [hex(0x0b0d12), hex(0xd93f5b), hex(0xfcf3a6)],
    }
}

/// Background and grid lines of every diagram.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlotStyle {
    pub background: Paint,
    pub grid: Paint,
}

impl Default for PlotStyle {
    fn default() -> Self {
        Self {
            background: paint(0x13151a, 1.0),
            grid: paint(0x2b313c, 1.0),
        }
    }
}

const LIME: u32 = 0xc6e05a;
const BLUE: u32 = 0x5ca8ff;
const VIOLET: u32 = 0xa48bfa;
const LIGHT: u32 = 0xd6dbe4;
const RED: u32 = 0xff5c7a;
const YELLOW: u32 = 0xf5d04c;

/// Spectrum curves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumStyle {
    /// Left, mid or the single curve.
    pub first: Paint,
    pub right: Paint,
    pub side: Paint,
    /// Opacity of the area below the curves.
    pub fill: f32,
    pub peak: Paint,
}

impl Default for SpectrumStyle {
    fn default() -> Self {
        Self {
            first: paint(LIME, 1.0),
            right: paint(BLUE, 1.0),
            side: paint(VIOLET, 1.0),
            fill: 0.16,
            peak: paint(LIGHT, 0.45),
        }
    }
}

/// The sonogram image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SonogramStyle {
    pub heat: HeatStyle,
}

/// The spectral view's image and waveforms.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectralStyle {
    pub heat: HeatStyle,
    /// Waveform of left, mid or the single signal.
    pub first: Rgb,
    pub right: Rgb,
    pub side: Rgb,
}

impl Default for SpectralStyle {
    fn default() -> Self {
        Self {
            heat: HeatStyle::default(),
            first: hex(LIME),
            right: hex(BLUE),
            side: hex(VIOLET),
        }
    }
}

/// Oscilloscope traces and the trigger marker.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScopeStyle {
    pub first: Paint,
    pub right: Paint,
    pub side: Paint,
    pub trigger: Paint,
}

impl Default for ScopeStyle {
    fn default() -> Self {
        Self {
            first: paint(LIME, 1.0),
            right: paint(BLUE, 1.0),
            side: paint(VIOLET, 1.0),
            trigger: paint(LIGHT, 1.0),
        }
    }
}

/// The stereo imager.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StereoStyle {
    /// Goniometer samples.
    pub scope: Paint,
    /// Correlation in phase, the mid level and the balance.
    pub mid: Rgb,
    pub side: Rgb,
    pub balance: Rgb,
    /// Bands of the image by frequency.
    pub bands: Paint,
    /// Opposite phase: negative correlation.
    pub opposite: Rgb,
}

impl Default for StereoStyle {
    fn default() -> Self {
        Self {
            scope: paint(LIME, 0.75),
            mid: hex(LIME),
            side: hex(VIOLET),
            balance: hex(BLUE),
            bands: paint(LIME, 0.85),
            opposite: hex(RED),
        }
    }
}

/// The loudness meter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessStyle {
    pub short_term: Paint,
    pub momentary: Paint,
    /// Readouts and bars within a loudness unit of the target.
    pub on_target: Rgb,
    pub too_loud: Rgb,
    pub too_quiet: Rgb,
}

impl Default for LoudnessStyle {
    fn default() -> Self {
        Self {
            short_term: paint(LIME, 1.0),
            momentary: paint(LIGHT, 0.45),
            on_target: hex(LIME),
            too_loud: hex(YELLOW),
            too_quiet: hex(BLUE),
        }
    }
}

/// The 3D views.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceStyle {
    pub heat: HeatStyle,
    /// Opposite phase in the stereogram.
    pub opposite: Rgb,
}

impl Default for SurfaceStyle {
    fn default() -> Self {
        Self {
            heat: heat(ColorMap::Viridis),
            opposite: hex(RED),
        }
    }
}

/// Each view's diagram colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewStyles {
    pub spectrum: SpectrumStyle,
    pub sonogram: SonogramStyle,
    pub spectral: SpectralStyle,
    pub oscilloscope: ScopeStyle,
    pub stereo: StereoStyle,
    pub loudness: LoudnessStyle,
    pub surface: SurfaceStyle,
}

impl ViewStyles {
    /// Restores one view's colors.
    pub fn reset(&mut self, module: Module) {
        match module {
            Module::Spectrum => self.spectrum = SpectrumStyle::default(),
            Module::Sonogram => self.sonogram = SonogramStyle::default(),
            Module::Spectral => self.spectral = SpectralStyle::default(),
            Module::Oscilloscope => self.oscilloscope = ScopeStyle::default(),
            Module::Stereo => self.stereo = StereoStyle::default(),
            Module::Loudness => self.loudness = LoudnessStyle::default(),
            Module::Surface => self.surface = SurfaceStyle::default(),
        }
    }

    /// The heat map of a view that has one.
    pub fn heat_mut(&mut self, module: Module) -> Option<&mut HeatStyle> {
        match module {
            Module::Sonogram => Some(&mut self.sonogram.heat),
            Module::Spectral => Some(&mut self.spectral.heat),
            Module::Surface => Some(&mut self.surface.heat),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ViewStyles, hex};
    use crate::layout::Module;
    use crate::settings::ColorMap;

    #[test]
    fn views_have_their_own_colors_and_reset_alone() {
        let mut styles = ViewStyles::default();
        assert_eq!(hex(0x123456), [0x12, 0x34, 0x56]);
        assert_ne!(styles.surface.heat.map, styles.sonogram.heat.map);
        styles.spectrum.first.color = [1, 2, 3];
        styles.stereo.mid = [4, 5, 6];
        if let Some(heat) = styles.heat_mut(Module::Sonogram) {
            heat.map = ColorMap::Ice;
        }
        assert!(styles.heat_mut(Module::Loudness).is_none());
        styles.reset(Module::Spectrum);
        assert_eq!(styles.spectrum, ViewStyles::default().spectrum);
        assert_eq!(styles.stereo.mid, [4, 5, 6]);
        assert_eq!(styles.sonogram.heat.map, ColorMap::Ice);
    }
}
