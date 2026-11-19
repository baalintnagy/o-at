//! Colors: the theme's accent for the controls, the diagrams' shared background and grid,
//! and heat maps.

use std::array;

use egui::Color32;

use crate::settings::{AnalyzerSettings, ColorMap, ColorScheme};
use crate::style::{HeatStyle, Paint, Rgb};

/// The theme's accent and the diagram background and grid, for one frame. View colors
/// come from each view's style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// Panel marks, selected controls and highlights.
    pub accent: Color32,
    /// Diagram backgrounds.
    pub plot: Color32,
    pub grid: Color32,
    /// Emphasized grid lines: zero, labelled frequencies.
    pub grid_strong: Color32,
}

/// Converts a stored color.
pub fn rgb([r, g, b]: Rgb) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// A stored color with its opacity.
pub fn paint(paint: Paint) -> Color32 {
    let [r, g, b] = paint.color;
    Color32::from_rgba_unmultiplied(r, g, b, (paint.opacity.clamp(0.0, 1.0) * 255.0) as u8)
}

/// `color` blended towards white by `amount` (0..1), keeping its opacity.
pub fn lighten(color: Color32, amount: f32) -> Color32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let blend = |value: u8| (f32::from(value) + (255.0 - f32::from(value)) * amount).round() as u8;
    Color32::from_rgba_unmultiplied(blend(r), blend(g), blend(b), a)
}

/// The accent of a built-in theme; CUSTOM has its own.
fn theme_accent(scheme: ColorScheme, custom: Rgb) -> Rgb {
    let hex = |value: u32| [(value >> 16) as u8, (value >> 8) as u8, value as u8];
    match scheme {
        ColorScheme::Oat => hex(0xc6e05a),
        ColorScheme::Phosphor => hex(0x4cff8a),
        ColorScheme::Sunset => hex(0xff8a3d),
        ColorScheme::Ocean => hex(0x3cd6c0),
        ColorScheme::Mono => hex(0xe6e9ef),
        // Okabe-Ito orange, distinguishable with color vision deficiencies.
        ColorScheme::Contrast => hex(0xe69f00),
        ColorScheme::Custom => custom,
    }
}

impl Colors {
    /// The colors the settings select.
    pub fn new(settings: &AnalyzerSettings) -> Self {
        let grid = paint(settings.plot.grid);
        Self {
            accent: rgb(theme_accent(settings.scheme, settings.accent)),
            plot: paint(settings.plot.background),
            grid,
            grid_strong: lighten(grid, 0.12),
        }
    }

    /// A theme's accent, as the starting point for a custom one.
    pub fn accent_of(scheme: ColorScheme, custom: Rgb) -> Rgb {
        theme_accent(scheme, custom)
    }
}

impl Default for Colors {
    fn default() -> Self {
        Self::new(&AnalyzerSettings::default())
    }
}

const EMBER: [Rgb; 6] = [
    [0x0b, 0x0d, 0x12],
    [0x2a, 0x0f, 0x52],
    [0x7d, 0x1f, 0x7a],
    [0xd9, 0x3f, 0x5b],
    [0xfb, 0x8b, 0x3d],
    [0xfc, 0xf3, 0xa6],
];
const VIRIDIS: [Rgb; 6] = [
    [0x10, 0x0b, 0x22],
    [0x44, 0x01, 0x54],
    [0x3b, 0x52, 0x8b],
    [0x21, 0x91, 0x8c],
    [0x5e, 0xc9, 0x62],
    [0xfd, 0xe7, 0x25],
];
const ICE: [Rgb; 6] = [
    [0x0b, 0x0d, 0x12],
    [0x10, 0x26, 0x4a],
    [0x1d, 0x5a, 0xb0],
    [0x5c, 0xa8, 0xff],
    [0xb8, 0xe6, 0xff],
    [0xff, 0xff, 0xff],
];
const GRAY: [Rgb; 2] = [[0x0b, 0x0d, 0x12], [0xff, 0xff, 0xff]];

/// Fraction of a heat map, from its quiet end, over which images fade into the diagram
/// background.
const IMAGE_FADE: f32 = 0.15;

/// Lookup table of 256 colors from quiet to loud.
#[derive(Clone, Debug)]
pub struct Palette {
    heat: HeatStyle,
    colors: [Color32; 256],
}

impl Default for Palette {
    fn default() -> Self {
        Self::new(HeatStyle::default())
    }
}

impl Palette {
    pub fn new(heat: HeatStyle) -> Self {
        let stops: &[Rgb] = match heat.map {
            ColorMap::Ember => &EMBER,
            ColorMap::Viridis => &VIRIDIS,
            ColorMap::Ice => &ICE,
            ColorMap::Gray => &GRAY,
            ColorMap::Custom => &heat.stops,
        };
        let segments = (stops.len() - 1) as f32;
        let colors = array::from_fn(|index| {
            let position = index as f32 / 255.0 * segments;
            let stop = (position.floor() as usize).min(stops.len() - 2);
            let fraction = position - stop as f32;
            let [r, g, b] = [0, 1, 2].map(|channel| {
                let from = f32::from(stops[stop][channel]);
                let to = f32::from(stops[stop + 1][channel]);
                (from + (to - from) * fraction).round() as u8
            });
            Color32::from_rgb(r, g, b)
        });
        Self { heat, colors }
    }

    pub fn heat(&self) -> HeatStyle {
        self.heat
    }

    /// Color of `fraction` from 0 (quiet) to 1 (loud).
    pub fn color(&self, fraction: f32) -> Color32 {
        self.colors[(fraction.clamp(0.0, 1.0) * 255.0).round() as usize]
    }

    /// Color of `fraction` for images drawn over the diagram background: the quietest
    /// part of the map fades out, so silence shows the background color.
    pub fn image_color(&self, fraction: f32) -> Color32 {
        let fraction = fraction.clamp(0.0, 1.0);
        let opacity = (fraction / IMAGE_FADE).min(1.0);
        self.color(fraction).gamma_multiply(opacity)
    }
}

#[cfg(test)]
mod tests {
    use egui::Color32;

    use super::{Colors, Palette, paint, rgb};
    use crate::settings::{AnalyzerSettings, ColorMap, ColorScheme};
    use crate::style::{HeatStyle, Paint};

    #[test]
    fn palettes_run_from_dark_to_bright() {
        let palette = Palette::new(HeatStyle {
            map: ColorMap::Gray,
            ..HeatStyle::default()
        });
        assert_eq!(palette.color(1.0), Color32::WHITE);
        assert_eq!(palette.color(2.0), Color32::WHITE);
        assert_eq!(palette.color(-0.5), palette.color(0.0));
        let custom = Palette::new(HeatStyle {
            map: ColorMap::Custom,
            stops: [[0, 0, 0], [255, 0, 0], [255, 255, 255]],
        });
        let [red, green, blue, _] = custom.color(0.5).to_array();
        assert!(red == 255 && green <= 2 && blue <= 2, "the middle stop");
    }

    #[test]
    fn the_theme_only_sets_the_accent() {
        let mut settings = AnalyzerSettings {
            scheme: ColorScheme::Sunset,
            ..AnalyzerSettings::default()
        };
        let sunset = Colors::new(&settings);
        assert_eq!(sunset.accent, rgb([0xff, 0x8a, 0x3d]));
        settings.scheme = ColorScheme::Oat;
        let oat = Colors::new(&settings);
        assert_eq!(sunset.plot, oat.plot, "diagrams do not follow the theme");
        settings.scheme = ColorScheme::Custom;
        settings.accent = [1, 2, 3];
        assert_eq!(Colors::new(&settings).accent, rgb([1, 2, 3]));
        settings.plot.grid = Paint {
            color: [10, 20, 30],
            opacity: 0.5,
        };
        assert_eq!(Colors::new(&settings).grid, paint(settings.plot.grid));
    }
}
