//! Color menus: the THEME panel for the controls and the shared diagram background and
//! grid, and the menu a right click on a diagram opens with that view's colors.

use aud_ui::theme::{PANEL, TEXT, TEXT_DIM, TEXT_FAINT, stroke};
use egui::color_picker::{Alpha, color_edit_button_srgba};
use egui::{
    Area, Color32, ComboBox, Context, CornerRadius, Frame, Grid, Id, Key, Margin, Order, Pos2,
    Rect, RichText, Slider, Ui, vec2,
};

use crate::layout::Module;
use crate::settings::{
    AnalyzerSettings, Choice, ColorMap, ColorScheme, Range, SonogramSettings, SpectralSettings,
    Taper,
};
use crate::style::{HeatStyle, Paint, PlotStyle, Rgb, ViewStyles};
use crate::view::colormap::{Colors, rgb};

/// Width of the menus' sliders in logical pixels.
const SLIDER_WIDTH: f32 = 130.0;

/// Opacity of a paint: logarithmic, so faint values are easy to set.
const OPACITY: Range = Range {
    min: 0.0,
    max: 1.0,
    default: 1.0,
    taper: Taper::Log,
};

/// An open right-click menu: the view and where it opened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewMenu {
    pub module: Module,
    pub at: Pos2,
}

/// One line of a color menu.
enum Entry<'a> {
    Heading(&'static str),
    Paint(&'static str, &'a mut Paint),
    Color(&'static str, &'a mut Rgb),
    Heat(&'a mut HeatStyle),
    Amount(&'static str, &'a mut f32, Range, fn(f32) -> String),
}

fn percent(value: f32) -> String {
    format!("{:.0} %", value * 100.0)
}

fn decibels(value: f32) -> String {
    format!("{value:+.0} dB")
}

fn factor(value: f32) -> String {
    format!("{value:.2}×")
}

/// The diagram background and grid, shared by every view.
fn plot_entries(plot: &mut PlotStyle) -> [Entry<'_>; 3] {
    [
        Entry::Heading("ALL DIAGRAMS"),
        Entry::Paint("Background", &mut plot.background),
        Entry::Paint("Grid", &mut plot.grid),
    ]
}

/// Brightness and contrast of a sonogram image.
fn tone_entries(sonogram: &mut SonogramSettings) -> [Entry<'_>; 2] {
    [
        Entry::Amount(
            "Brightness",
            &mut sonogram.brightness_db,
            SonogramSettings::BRIGHTNESS_DB,
            decibels,
        ),
        Entry::Amount(
            "Contrast",
            &mut sonogram.contrast,
            SonogramSettings::CONTRAST,
            factor,
        ),
    ]
}

/// What a view's menu offers: its own elements, then the shared background and grid.
fn entries(module: Module, settings: &mut AnalyzerSettings) -> Vec<Entry<'_>> {
    let AnalyzerSettings {
        styles,
        plot,
        sonogram,
        spectral,
        ..
    } = settings;
    let ViewStyles {
        spectrum,
        sonogram: sonogram_style,
        spectral: spectral_style,
        oscilloscope,
        stereo,
        loudness,
        surface,
    } = styles;
    let mut list = match module {
        Module::Spectrum => vec![
            Entry::Heading("SPECTRUM"),
            Entry::Paint("Left · mid curve", &mut spectrum.first),
            Entry::Paint("Right curve", &mut spectrum.right),
            Entry::Paint("Side curve", &mut spectrum.side),
            Entry::Amount("Fill below curves", &mut spectrum.fill, OPACITY, percent),
            Entry::Paint("Peak hold", &mut spectrum.peak),
        ],
        Module::Sonogram => {
            let mut list = vec![
                Entry::Heading("SONOGRAM"),
                Entry::Heat(&mut sonogram_style.heat),
            ];
            list.extend(tone_entries(sonogram));
            list
        }
        Module::Spectral => {
            let mut list = vec![
                Entry::Heading("SONOGRAM LAYER"),
                Entry::Heat(&mut spectral_style.heat),
                Entry::Amount(
                    "Opacity",
                    &mut spectral.sonogram_opacity,
                    SpectralSettings::OPACITY,
                    percent,
                ),
            ];
            list.extend(tone_entries(sonogram));
            list.extend([
                Entry::Heading("WAVEFORM LAYER"),
                Entry::Color("Left · mid", &mut spectral_style.first),
                Entry::Color("Right", &mut spectral_style.right),
                Entry::Color("Side", &mut spectral_style.side),
                Entry::Amount(
                    "Opacity",
                    &mut spectral.waveform_opacity,
                    SpectralSettings::OPACITY,
                    percent,
                ),
            ]);
            list
        }
        Module::Oscilloscope => vec![
            Entry::Heading("SCOPE"),
            Entry::Paint("Left · mid trace", &mut oscilloscope.first),
            Entry::Paint("Right trace", &mut oscilloscope.right),
            Entry::Paint("Side trace", &mut oscilloscope.side),
            Entry::Paint("Trigger marker", &mut oscilloscope.trigger),
        ],
        Module::Stereo => vec![
            Entry::Heading("STEREO"),
            Entry::Paint("Goniometer", &mut stereo.scope),
            Entry::Color("Mid · in phase", &mut stereo.mid),
            Entry::Color("Side", &mut stereo.side),
            Entry::Color("Balance", &mut stereo.balance),
            Entry::Paint("Bands", &mut stereo.bands),
            Entry::Color("Opposite phase", &mut stereo.opposite),
        ],
        Module::Loudness => vec![
            Entry::Heading("LOUDNESS"),
            Entry::Paint("Short-term", &mut loudness.short_term),
            Entry::Paint("Momentary", &mut loudness.momentary),
            Entry::Color("On target", &mut loudness.on_target),
            Entry::Color("Too loud", &mut loudness.too_loud),
            Entry::Color("Too quiet", &mut loudness.too_quiet),
        ],
        Module::Surface => vec![
            Entry::Heading("3D"),
            Entry::Heat(&mut surface.heat),
            Entry::Color("Opposite phase", &mut surface.opposite),
        ],
    };
    list.extend(plot_entries(plot));
    list
}

/// A color button with a picker.
fn swatch(ui: &mut Ui, value: &mut Rgb) {
    let mut color = rgb(*value);
    if color_edit_button_srgba(ui, &mut color, Alpha::Opaque).changed() {
        let [r, g, b, _] = color.to_array();
        *value = [r, g, b];
    }
}

/// A slider over `range`, logarithmic where the range says so.
fn amount(ui: &mut Ui, value: &mut f32, range: Range, format: fn(f32) -> String) {
    let slider = Slider::new(value, range.min..=range.max)
        .custom_formatter(move |value, _| format(value as f32))
        .show_value(true);
    let slider = if range.taper == Taper::Log {
        slider.logarithmic(true).smallest_positive(0.01)
    } else {
        slider
    };
    ui.spacing_mut().slider_width = SLIDER_WIDTH;
    if ui.add(slider).double_clicked() {
        *value = range.default;
    }
}

/// Draws the lines of a menu.
fn draw_entries(ui: &mut Ui, id: Id, entries: Vec<Entry<'_>>) {
    Grid::new(id)
        .num_columns(3)
        .spacing(vec2(10.0, 6.0))
        .show(ui, |ui| {
            for entry in entries {
                match entry {
                    Entry::Heading(text) => {
                        ui.label(RichText::new(text).size(10.0).color(TEXT_FAINT));
                        ui.end_row();
                    }
                    Entry::Paint(label, paint) => {
                        ui.label(RichText::new(label).color(TEXT));
                        swatch(ui, &mut paint.color);
                        amount(ui, &mut paint.opacity, OPACITY, percent);
                        ui.end_row();
                    }
                    Entry::Color(label, color) => {
                        ui.label(RichText::new(label).color(TEXT));
                        swatch(ui, color);
                        ui.end_row();
                    }
                    Entry::Amount(label, value, range, format) => {
                        ui.label(RichText::new(label).color(TEXT));
                        ui.label("");
                        amount(ui, value, range, format);
                        ui.end_row();
                    }
                    Entry::Heat(heat) => {
                        ui.label(RichText::new("Heat map").color(TEXT));
                        let mut map = heat.map;
                        ComboBox::from_id_salt((id, "heat"))
                            .selected_text(ColorMap::LABELS[map.index()])
                            .show_ui(ui, |ui| {
                                for (choice, label) in ColorMap::ALL.iter().zip(ColorMap::LABELS) {
                                    ui.selectable_value(&mut map, *choice, *label);
                                }
                            });
                        heat.map = map;
                        ui.horizontal(|ui| {
                            let before = heat.stops;
                            for stop in &mut heat.stops {
                                swatch(ui, stop);
                            }
                            ui.label(RichText::new("custom").size(10.0).color(TEXT_DIM));
                            if heat.stops != before {
                                heat.map = ColorMap::Custom;
                            }
                        });
                        ui.end_row();
                    }
                }
            }
        });
}

/// The frame of the menus: the panel color with the accent as border.
fn frame(accent: Color32) -> Frame {
    Frame::NONE
        .fill(PANEL)
        .stroke(stroke(1.0, accent))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(12))
}

/// Whether a menu at `rect` should close: Esc, or a press elsewhere while no color
/// picker or list is open.
fn should_close(context: &Context, rect: Rect, keep: Option<Rect>) -> bool {
    let picking = context.memory(|memory| memory.any_popup_open());
    context.input(|input| {
        input.key_pressed(Key::Escape)
            || (!picking
                && input.pointer.any_pressed()
                && input.pointer.interact_pos().is_some_and(|pointer| {
                    !rect.contains(pointer) && !keep.is_some_and(|keep| keep.contains(pointer))
                }))
    })
}

/// The right-click menu of a view. Returns `false` when it closed.
pub fn view_menu(
    context: &Context,
    menu: ViewMenu,
    settings: &mut AnalyzerSettings,
    colors: &Colors,
) -> bool {
    let id = Id::new(("view-menu", menu.module));
    let mut reset = false;
    let response = Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(menu.at)
        .constrain(true)
        .show(context, |ui| {
            frame(colors.accent).show(ui, |ui| {
                ui.set_max_width(420.0);
                ui.label(
                    RichText::new(format!("{} COLORS", menu.module.label()))
                        .size(12.0)
                        .color(colors.accent),
                );
                ui.add_space(6.0);
                draw_entries(ui, id, entries(menu.module, settings));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    reset = ui.button("Reset this view's colors").clicked();
                    ui.label(
                        RichText::new("double-click a slider to reset it")
                            .size(10.0)
                            .color(TEXT_FAINT),
                    );
                });
            });
        })
        .response;
    if reset {
        settings.styles.reset(menu.module);
    }
    !should_close(context, response.rect, None)
}

/// The THEME panel below its button. Returns `false` when it closed.
pub fn theme_panel(
    context: &Context,
    button: Rect,
    settings: &mut AnalyzerSettings,
    colors: &Colors,
) -> bool {
    let id = Id::new("theme-panel");
    let mut reset_views = false;
    let response = Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(button.left_bottom() + vec2(-300.0, 8.0))
        .constrain(true)
        .show(context, |ui| {
            frame(colors.accent).show(ui, |ui| {
                ui.set_max_width(460.0);
                ui.label(RichText::new("THEME").size(12.0).color(colors.accent));
                ui.label(
                    RichText::new("Buttons, tabs, sliders and panel marks; never the diagrams")
                        .size(10.0)
                        .color(TEXT_DIM),
                );
                ui.add_space(6.0);
                Grid::new((id, "theme"))
                    .num_columns(2)
                    .spacing(vec2(10.0, 6.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Preset").color(TEXT));
                        let mut scheme = settings.scheme;
                        ComboBox::from_id_salt((id, "scheme"))
                            .selected_text(ColorScheme::LABELS[scheme.index()])
                            .show_ui(ui, |ui| {
                                for (choice, label) in
                                    ColorScheme::ALL.iter().zip(ColorScheme::LABELS)
                                {
                                    ui.selectable_value(&mut scheme, *choice, *label);
                                }
                            });
                        if scheme != settings.scheme {
                            settings.scheme = scheme;
                        }
                        ui.end_row();
                        ui.label(RichText::new("Accent").color(TEXT));
                        let mut accent = Colors::accent_of(settings.scheme, settings.accent);
                        let before = accent;
                        swatch(ui, &mut accent);
                        if accent != before {
                            settings.accent = accent;
                            settings.scheme = ColorScheme::Custom;
                        }
                        ui.end_row();
                    });
                ui.add_space(10.0);
                draw_entries(
                    ui,
                    Id::new((id, "plot")),
                    plot_entries(&mut settings.plot).into(),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Right-click any diagram to set its own colors.")
                        .size(10.0)
                        .color(TEXT_DIM),
                );
                ui.add_space(4.0);
                reset_views = ui.button("Reset every view's colors").clicked();
            });
        })
        .response;
    if reset_views {
        settings.styles = ViewStyles::default();
    }
    !should_close(context, response.rect, Some(button))
}

#[cfg(test)]
mod tests {
    use super::{Entry, entries};
    use crate::layout::Module;
    use crate::settings::AnalyzerSettings;

    #[test]
    fn every_view_menu_ends_with_the_shared_background_and_grid() {
        for module in Module::ALL {
            let mut settings = AnalyzerSettings::default();
            let list = entries(module, &mut settings);
            assert!(list.len() > 4, "{module:?} offers its own elements");
            assert!(matches!(list[0], Entry::Heading(_)));
            assert!(matches!(
                list[list.len() - 2],
                Entry::Paint("Background", _)
            ));
            assert!(matches!(list[list.len() - 1], Entry::Paint("Grid", _)));
        }
    }

    #[test]
    fn menu_entries_change_the_settings() {
        let mut settings = AnalyzerSettings::default();
        for entry in entries(Module::Loudness, &mut settings) {
            if let Entry::Color("Too loud", color) = entry {
                *color = [1, 2, 3];
            }
        }
        assert_eq!(settings.styles.loudness.too_loud, [1, 2, 3]);
        assert_eq!(
            settings.styles.spectrum,
            AnalyzerSettings::default().styles.spectrum
        );
    }
}
