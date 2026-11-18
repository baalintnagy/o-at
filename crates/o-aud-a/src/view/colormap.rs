//! Colors: the trace and accent colors of a scheme, and the heat maps of the sonogram,
//! spectral and 3D views.

use std::array;

use aud_ui::theme::{GRID, GRID_STRONG};
use egui::Color32;

use crate::settings::{AnalyzerSettings, ColorMap, ColorScheme, CustomColors, Rgb};

/// Plot background of the built-in schemes.
const PLOT: Rgb = [0x13, 0x15, 0x1a];

/// A heat map: a built-in one, or the custom stops.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heat {
    pub map: ColorMap,
    pub custom: [Rgb; 3],
}

impl Default for Heat {
    fn default() -> Self {
        Self {
            map: ColorMap::default(),
            custom: CustomColors::default().heat,
        }
    }
}

/// The colors of every view for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// Panel marks, selected controls and highlights.
    pub accent: Color32,
    /// The first trace: left, mid or a single signal.
    pub primary: Color32,
    /// The second trace: right.
    pub secondary: Color32,
    /// The side signal.
    pub side: Color32,
    /// Plot backgrounds.
    pub plot: Color32,
    pub grid: Color32,
    /// Emphasized grid lines: zero, labelled frequencies.
    pub grid_strong: Color32,
    pub heat: Heat,
}

/// Converts a stored color.
pub fn rgb([r, g, b]: Rgb) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The trace roles of a built-in scheme: accent, primary, secondary and side.
fn preset(scheme: ColorScheme) -> [Rgb; 4] {
    let hex = |value: u32| [(value >> 16) as u8, (value >> 8) as u8, value as u8];
    let values = match scheme {
        ColorScheme::Oat | ColorScheme::Custom => [0xc6e05a, 0xc6e05a, 0x5ca8ff, 0xa48bfa],
        ColorScheme::Phosphor => [0x4cff8a, 0x4cff8a, 0xf5d04c, 0x3cd6c0],
        ColorScheme::Sunset => [0xff8a3d, 0xff8a3d, 0xff5c9a, 0xf5d04c],
        ColorScheme::Ocean => [0x3cd6c0, 0x3cd6c0, 0x5ca8ff, 0x9b8cff],
        ColorScheme::Mono => [0xe6e9ef, 0xe6e9ef, 0x8a93a3, 0xb0b6c2],
        // Okabe-Ito colors, distinguishable with color vision deficiencies.
        ColorScheme::Contrast => [0xe69f00, 0xe69f00, 0x56b4e9, 0xcc79a7],
    };
    values.map(hex)
}

/// `color` blended towards white by `amount` (0..1).
pub fn lighten(color: Color32, amount: f32) -> Color32 {
    let [r, g, b, _] = color.to_array();
    let blend = |value: u8| (f32::from(value) + (255.0 - f32::from(value)) * amount).round() as u8;
    Color32::from_rgb(blend(r), blend(g), blend(b))
}

impl Colors {
    /// The colors the settings select.
    pub fn new(settings: &AnalyzerSettings) -> Self {
        let custom = &settings.custom;
        let heat = Heat {
            map: settings.colormap,
            custom: custom.heat,
        };
        if settings.scheme == ColorScheme::Custom {
            let grid = rgb(custom.grid);
            return Self {
                accent: rgb(custom.accent),
                primary: rgb(custom.primary),
                secondary: rgb(custom.secondary),
                side: rgb(custom.side),
                plot: rgb(custom.plot),
                grid,
                grid_strong: lighten(grid, 0.1),
                heat,
            };
        }
        let [accent, primary, secondary, side] = preset(settings.scheme).map(rgb);
        Self {
            accent,
            primary,
            secondary,
            side,
            plot: rgb(PLOT),
            grid: GRID,
            grid_strong: GRID_STRONG,
            heat,
        }
    }

    /// The built-in scheme's colors as custom colors, as a starting point for editing.
    pub fn custom_from(scheme: ColorScheme, current: &CustomColors) -> CustomColors {
        if scheme == ColorScheme::Custom {
            return *current;
        }
        let [accent, primary, secondary, side] = preset(scheme);
        CustomColors {
            accent,
            primary,
            secondary,
            side,
            plot: PLOT,
            grid: GRID.to_array()[..3].try_into().unwrap_or(current.grid),
            heat: current.heat,
        }
    }

    /// The second trace's color for a pair of curves: side next to mid, else secondary.
    pub fn second(&self, side: bool) -> Color32 {
        if side { self.side } else { self.secondary }
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

/// Lookup table of 256 colors from quiet to loud.
#[derive(Clone, Debug)]
pub struct Palette {
    heat: Heat,
    colors: [Color32; 256],
}

impl Default for Palette {
    fn default() -> Self {
        Self::new(Heat::default())
    }
}

impl Palette {
    pub fn new(heat: Heat) -> Self {
        let stops: &[Rgb] = match heat.map {
            ColorMap::Ember => &EMBER,
            ColorMap::Viridis => &VIRIDIS,
            ColorMap::Ice => &ICE,
            ColorMap::Gray => &GRAY,
            ColorMap::Custom => &heat.custom,
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

    pub fn heat(&self) -> Heat {
        self.heat
    }

    /// Color of `fraction` from 0 (quiet) to 1 (loud).
    pub fn color(&self, fraction: f32) -> Color32 {
        self.colors[(fraction.clamp(0.0, 1.0) * 255.0).round() as usize]
    }
}

#[cfg(test)]
mod tests {
    use egui::Color32;

    use super::{Colors, Heat, Palette, rgb};
    use crate::settings::{AnalyzerSettings, ColorMap, ColorScheme, CustomColors};

    #[test]
    fn palettes_run_from_dark_to_bright() {
        let palette = Palette::new(Heat {
            map: ColorMap::Gray,
            ..Heat::default()
        });
        assert_eq!(palette.color(1.0), Color32::WHITE);
        assert_eq!(palette.color(2.0), Color32::WHITE);
        assert_eq!(palette.color(-0.5), palette.color(0.0));
        let custom = Palette::new(Heat {
            map: ColorMap::Custom,
            custom: [[0, 0, 0], [255, 0, 0], [255, 255, 255]],
        });
        let [red, green, blue, _] = custom.color(0.5).to_array();
        assert!(red == 255 && green <= 2 && blue <= 2, "the middle stop");
    }

    #[test]
    fn custom_schemes_use_their_colors_and_start_from_a_preset() {
        let mut settings = AnalyzerSettings::default();
        let start = Colors::custom_from(ColorScheme::Sunset, &CustomColors::default());
        assert_eq!(start.accent, [0xff, 0x8a, 0x3d]);
        settings.custom = CustomColors {
            secondary: [1, 2, 3],
            ..start
        };
        settings.scheme = ColorScheme::Custom;
        let colors = Colors::new(&settings);
        assert_eq!(colors.secondary, rgb([1, 2, 3]));
        assert_eq!(colors.accent, rgb([0xff, 0x8a, 0x3d]));
    }
}
