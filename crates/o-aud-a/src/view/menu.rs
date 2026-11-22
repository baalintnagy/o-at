//! Popups: the THEME panel for the controls and the shared diagram background and grid,
//! the overview's VIEWS panel, and each view's popup, opened by its settings button or a right click, with a
//! SETTINGS tab for what the view shows and a COLORS tab for how it is colored.

use aud_ui::theme::{PANEL, TEXT, TEXT_DIM, TEXT_FAINT, font, stroke};
use aud_ui::widgets::{button, segmented};
use egui::color_picker::{Alpha, color_edit_button_srgba};
use egui::{
    Align2, Area, Color32, ComboBox, Context, CornerRadius, Frame, Grid, Id, Key, Margin, Order,
    Pos2, Rect, RichText, Sense, Slider, Ui, vec2,
};

use crate::layout::{Module, OverviewLayout};
use crate::settings::{
    AnalyzerSettings, Choice, ColorMap, ColorScheme, LEVEL_RANGE_DB, Lanes, LoudnessSettings,
    LoudnessTarget, Range, SonogramSettings, SpectralSettings, SpectrumSettings, SurfaceSettings,
    Taper,
};
use crate::sources::SourceList;
use crate::style::{HeatStyle, Paint, PlotStyle, Rgb, ViewStyles};
use crate::view::Actions;
use crate::view::colormap::{Colors, rgb};
use crate::view::plot::text_width;
use crate::view::sources::source_color;

/// Width of the menus' sliders in logical pixels.
const SLIDER_WIDTH: f32 = 130.0;
/// Height of the settings tab's selectors and buttons in logical pixels.
const CONTROL_HEIGHT: f32 = 20.0;

/// Opacity of a paint: logarithmic, so faint values are easy to set.
const OPACITY: Range = Range {
    min: 0.0,
    max: 1.0,
    default: 1.0,
    taper: Taper::Log,
};

/// Oscilloscope trigger level as a fraction of the half height.
const TRIGGER_LEVEL: Range = Range {
    min: -1.0,
    max: 1.0,
    default: 0.0,
    taper: Taper::Linear,
};

/// Camera rotation around the vertical axis in degrees.
const YAW_DEG: Range = Range {
    min: -180.0,
    max: 180.0,
    default: SurfaceSettings::DEFAULT_YAW_DEG,
    taper: Taper::Linear,
};

/// Camera elevation in degrees.
const PITCH_DEG: Range = Range {
    min: SurfaceSettings::PITCH_RANGE_DEG.0,
    max: SurfaceSettings::PITCH_RANGE_DEG.1,
    default: SurfaceSettings::DEFAULT_PITCH_DEG,
    taper: Taper::Linear,
};

/// The tab a view's popup shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuTab {
    #[default]
    Settings,
    Colors,
}

/// An open view popup: the view, where it opened and the area that opened it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewMenu {
    pub module: Module,
    /// The popup's corner at `pivot`.
    pub at: Pos2,
    pub pivot: Align2,
    /// The button that opened the popup; a press on it does not close the popup, so the
    /// button can close it itself.
    pub opener: Option<Rect>,
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
            list.extend(tone_entries(spectral.sonogram.get_or_insert(*sonogram)));
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
pub(super) fn frame(accent: Color32) -> Frame {
    Frame::NONE
        .fill(PANEL)
        .stroke(stroke(1.0, accent))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(12))
}

/// Whether a menu at `rect` should close: Esc, or a press elsewhere while no color
/// picker or list is open.
pub(super) fn should_close(context: &Context, rect: Rect, keep: Option<Rect>) -> bool {
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

/// The controls of a view's SETTINGS tab, laid out as a two-column grid of captions and
/// controls: segmented selectors for choices of kind, sliders for ranges and steps.
struct SettingsGrid<'u> {
    ui: &'u mut Ui,
    accent: Color32,
}

impl SettingsGrid<'_> {
    fn caption(&mut self, text: &str) {
        self.ui.label(RichText::new(text).color(TEXT));
    }

    /// A heading above the next rows.
    fn heading(&mut self, text: &str) {
        self.ui
            .label(RichText::new(text).size(10.0).color(TEXT_FAINT));
        self.ui.end_row();
    }

    /// A segmented selector; `segment` is the width per choice.
    fn choice<C: Choice>(&mut self, caption: &str, value: &mut C, segment: f32) {
        self.caption(caption);
        let size = vec2(segment * C::LABELS.len() as f32, CONTROL_HEIGHT);
        let (rect, _) = self.ui.allocate_exact_size(size, Sense::hover());
        let key = format!("menu-{caption}");
        if let Some(index) = segmented(self.ui, &key, rect, C::LABELS, value.index(), self.accent) {
            *value = C::from_index(index);
        }
        self.ui.end_row();
    }

    /// A slider that steps through ordered choices; double-click resets it.
    fn steps<C: Choice + Default>(&mut self, caption: &str, value: &mut C) {
        self.caption(caption);
        let last = C::ALL.len() - 1;
        let mut index = value.index();
        let label = move |position: f64| C::LABELS[(position.round().max(0.0) as usize).min(last)];
        let slider = Slider::new(&mut index, 0..=last)
            .custom_formatter(move |position, _| label(position).to_owned())
            .custom_parser(|text| {
                C::LABELS
                    .iter()
                    .position(|candidate| candidate.eq_ignore_ascii_case(text.trim()))
                    .map(|position| position as f64)
            });
        self.ui.spacing_mut().slider_width = SLIDER_WIDTH;
        if self.ui.add(slider).double_clicked() {
            index = C::default().index();
        }
        *value = C::from_index(index);
        self.ui.end_row();
    }

    /// A slider over a continuous `range`; double-click resets it.
    fn number(&mut self, caption: &str, value: &mut f32, range: Range, format: fn(f32) -> String) {
        self.caption(caption);
        amount(self.ui, value, range, format);
        self.ui.end_row();
    }

    /// An on/off button.
    fn toggle(&mut self, caption: &str, value: &mut bool) {
        self.caption(caption);
        let (rect, _) = self
            .ui
            .allocate_exact_size(vec2(64.0, CONTROL_HEIGHT), Sense::hover());
        let label = if *value { "ON" } else { "OFF" };
        let id = Id::new(("menu-toggle", caption));
        if button(self.ui, id, rect, label, self.accent, *value).clicked() {
            *value = !*value;
        }
        self.ui.end_row();
    }

    /// The signals a view shows, as toggles: the input and each listed bus in its color.
    fn signals(&mut self, module: Module, list: &mut SourceList) {
        self.caption("Signals");
        let shown = list.shown(module);
        let toggles: Vec<(String, bool, Option<usize>)> =
            [(String::from("INPUT"), shown.input, None)]
                .into_iter()
                .chain(
                    list.buses
                        .iter()
                        .enumerate()
                        .map(|(index, bus)| (bus.name.clone(), shown.has_bus(index), Some(index))),
                )
                .collect();
        let accent = self.accent;
        let mut flipped = Vec::new();
        self.ui.horizontal_wrapped(|ui| {
            ui.set_max_width(300.0);
            for (label, on, bus) in &toggles {
                let width = (16.0 + text_width(ui.painter(), label, &font(11.0))).max(48.0);
                let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover());
                let color = bus.map_or(accent, source_color);
                let id = Id::new(("menu-signal", module, *bus));
                if button(ui, id, rect, label, color, *on)
                    .on_hover_text("Show or hide this signal here; several are overlaid")
                    .clicked()
                {
                    flipped.push((*bus, !*on));
                }
            }
            if list.buses.is_empty() {
                ui.label(
                    RichText::new("add buses under SOURCES")
                        .size(10.0)
                        .color(TEXT_DIM),
                );
            }
        });
        for (bus, on) in flipped {
            match bus {
                None => list.show_input(module, on),
                Some(index) => list.show_bus(module, index, on),
            }
        }
        self.ui.end_row();
    }

    /// The loudness target: a list of presets with what each is for, and the value of a
    /// custom target.
    fn target(&mut self, loudness: &mut LoudnessSettings) {
        self.caption("Target");
        let label = |target: LoudnessTarget| match target.lufs() {
            Some(lufs) => format!("{lufs:.0} LUFS · {}", target.description()),
            None => String::from("Custom"),
        };
        ComboBox::from_id_salt("menu-loudness-target")
            .selected_text(label(loudness.target))
            .width(SLIDER_WIDTH + 90.0)
            .show_ui(self.ui, |ui| {
                for target in LoudnessTarget::ALL {
                    ui.selectable_value(&mut loudness.target, *target, label(*target));
                }
            });
        self.ui.end_row();
        if loudness.target == LoudnessTarget::Custom {
            self.number(
                "Custom target",
                &mut loudness.custom_lufs,
                LoudnessSettings::CUSTOM_LUFS,
                |lufs| format!("{lufs:.0} LUFS"),
            );
        }
    }

    /// A push button after an empty caption; `true` when clicked.
    fn action(&mut self, label: &str) -> bool {
        self.caption("");
        let (rect, _) = self
            .ui
            .allocate_exact_size(vec2(150.0, CONTROL_HEIGHT), Sense::hover());
        let id = Id::new(("menu-action", label));
        let clicked = button(self.ui, id, rect, label, self.accent, false).clicked();
        self.ui.end_row();
        clicked
    }
}

fn whole_db(value: f32) -> String {
    format!("{value:.0} dB")
}

fn slope(value: f32) -> String {
    format!("{value:.1} dB/oct")
}

fn degrees(value: f32) -> String {
    format!("{value:.0}°")
}

fn signed_percent(value: f32) -> String {
    format!("{:+.0} %", value * 100.0)
}

/// The sonogram's settings, which the spectral view shares.
fn sonogram_rows(grid: &mut SettingsGrid<'_>, sonogram: &mut SonogramSettings) {
    grid.steps("FFT length", &mut sonogram.fft);
    grid.choice("Channels", &mut sonogram.channels, 50.0);
    if sonogram.channels == Lanes::Single {
        grid.choice("Signal", &mut sonogram.signal, 46.0);
    }
    grid.choice("Frequency scale", &mut sonogram.scale, 38.0);
    grid.choice("Highest frequency", &mut sonogram.limit, 38.0);
    grid.steps("Speed", &mut sonogram.speed);
    grid.number("Level range", &mut sonogram.range, LEVEL_RANGE_DB, whole_db);
}

/// The SETTINGS tab of `module`'s popup; returns the resets asked for.
fn settings_tab(
    grid: &mut SettingsGrid<'_>,
    module: Module,
    (settings, sources): (&mut AnalyzerSettings, &mut SourceList),
) -> Actions {
    let mut actions = Actions::default();
    grid.signals(module, sources);
    match module {
        Module::Spectrum => {
            let spectrum = &mut settings.spectrum;
            grid.steps("FFT length", &mut spectrum.fft);
            grid.choice("Window", &mut spectrum.window, 64.0);
            grid.choice("Channels", &mut spectrum.channels, 50.0);
            grid.choice("Averaging", &mut spectrum.averaging, 42.0);
            grid.steps("Smoothing", &mut spectrum.smoothing);
            grid.number(
                "Slope",
                &mut spectrum.slope,
                SpectrumSettings::SLOPE_DB,
                slope,
            );
            grid.number("Level range", &mut spectrum.range, LEVEL_RANGE_DB, whole_db);
            grid.choice("Frequency scale", &mut spectrum.scale, 38.0);
            grid.choice("Highest frequency", &mut spectrum.limit, 38.0);
            grid.toggle("Peak hold", &mut spectrum.peak_hold);
            actions.reset_peaks = grid.action("RESET PEAKS");
        }
        Module::Sonogram => sonogram_rows(grid, &mut settings.sonogram),
        Module::Spectral => {
            let spectral = &mut settings.spectral;
            grid.heading("WAVEFORM LAYER");
            grid.number(
                "Gain",
                &mut spectral.waveform_gain_db,
                SpectralSettings::WAVEFORM_GAIN_DB,
                |db| format!("+{db:.0} dB"),
            );
            grid.choice("Amplitude scale", &mut spectral.scale, 36.0);
            grid.heading("SONOGRAM LAYER");
            sonogram_rows(grid, spectral.sonogram.get_or_insert(settings.sonogram));
        }
        Module::Surface => {
            let surface = &mut settings.surface;
            grid.choice("Mode", &mut surface.mode, 76.0);
            grid.steps("Bands", &mut surface.bands);
            grid.steps("History rows", &mut surface.depth);
            grid.number("Level range", &mut surface.range, LEVEL_RANGE_DB, whole_db);
            grid.toggle("3D grid", &mut surface.grid);
            grid.heading("ANALYSIS · shared with the sonogram");
            let sonogram = &mut settings.sonogram;
            grid.steps("FFT length", &mut sonogram.fft);
            grid.choice("Channels", &mut sonogram.channels, 50.0);
            if sonogram.channels == Lanes::Single {
                grid.choice("Signal", &mut sonogram.signal, 46.0);
            }
            grid.heading("CAMERA");
            grid.number("Rotation", &mut surface.yaw_deg, YAW_DEG, degrees);
            grid.number("Elevation", &mut surface.pitch_deg, PITCH_DEG, degrees);
            grid.number("Zoom", &mut surface.zoom, SurfaceSettings::ZOOM, factor);
            if grid.action("RESET VIEW") {
                surface.reset_camera();
            }
        }
        Module::Stereo => {
            let stereo = &mut settings.stereo;
            grid.choice("Diagram", &mut stereo.mode, 52.0);
            grid.toggle("Lines", &mut stereo.lines);
            grid.steps("Samples", &mut stereo.samples);
            grid.choice("Gain", &mut stereo.gain, 56.0);
            grid.steps("Band FFT length", &mut settings.spectrum.fft);
        }
        Module::Oscilloscope => {
            let scope = &mut settings.oscilloscope;
            grid.steps("Time span", &mut scope.span);
            grid.choice("Channels", &mut scope.channels, 50.0);
            grid.choice("Layout", &mut scope.layout, 62.0);
            grid.steps("Zoom", &mut scope.zoom);
            grid.choice("Trigger", &mut scope.trigger, 42.0);
            grid.choice("Trigger source", &mut scope.source, 46.0);
            grid.number(
                "Trigger level",
                &mut scope.level,
                TRIGGER_LEVEL,
                signed_percent,
            );
        }
        Module::Loudness => {
            let loudness = &mut settings.loudness;
            grid.target(loudness);
            grid.choice("Readouts", &mut loudness.scale, 44.0);
            grid.steps("History", &mut loudness.span);
            grid.toggle("Platforms", &mut loudness.platforms);
            actions.reset_loudness = grid.action("RESET MEASUREMENT");
        }
    }
    actions
}

/// A view's popup with its SETTINGS and COLORS tabs. Returns whether it is still open
/// and the resets asked for.
pub fn view_menu(
    context: &Context,
    menu: ViewMenu,
    tab: &mut MenuTab,
    (settings, sources): (&mut AnalyzerSettings, &mut SourceList),
    colors: &Colors,
) -> (bool, Actions) {
    let id = Id::new(("view-menu", menu.module));
    let mut reset_colors = false;
    let mut actions = Actions::default();
    let response = Area::new(id)
        .order(Order::Foreground)
        .pivot(menu.pivot)
        .fixed_pos(menu.at)
        .constrain(true)
        .show(context, |ui| {
            frame(colors.accent).show(ui, |ui| {
                ui.set_max_width(460.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(menu.module.label())
                            .size(12.0)
                            .color(colors.accent),
                    );
                    ui.add_space(16.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(180.0, 22.0), Sense::hover());
                    if let Some(index) = segmented(
                        ui,
                        "view-menu-tab",
                        rect,
                        &["SETTINGS", "COLORS"],
                        usize::from(*tab == MenuTab::Colors),
                        colors.accent,
                    ) {
                        *tab = if index == 0 {
                            MenuTab::Settings
                        } else {
                            MenuTab::Colors
                        };
                    }
                });
                ui.add_space(8.0);
                match tab {
                    MenuTab::Settings => {
                        Grid::new((id, "settings"))
                            .num_columns(2)
                            .spacing(vec2(12.0, 6.0))
                            .show(ui, |ui| {
                                let mut grid = SettingsGrid {
                                    ui,
                                    accent: colors.accent,
                                };
                                actions = settings_tab(
                                    &mut grid,
                                    menu.module,
                                    (&mut *settings, &mut *sources),
                                );
                            });
                    }
                    MenuTab::Colors => {
                        draw_entries(ui, id, entries(menu.module, settings));
                        ui.add_space(8.0);
                        reset_colors = ui.button("Reset this view's colors").clicked();
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("double-click a slider to reset it · Esc closes")
                        .size(10.0)
                        .color(TEXT_FAINT),
                );
            });
        })
        .response;
    if reset_colors {
        settings.styles.reset(menu.module);
    }
    (!should_close(context, response.rect, menu.opener), actions)
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

/// The overview's VIEWS panel below its button: shows or hides each view and resets the
/// layout. Returns `false` when it closed.
pub fn views_panel(
    context: &Context,
    button_rect: Rect,
    layout: &mut OverviewLayout,
    colors: &Colors,
) -> bool {
    const BUTTON_WIDTH: f32 = 104.0;
    const COLUMNS: usize = 2;
    let id = Id::new("views-panel");
    let response = Area::new(id)
        .order(Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(button_rect.right_bottom() + vec2(0.0, 8.0))
        .constrain(true)
        .show(context, |ui| {
            frame(colors.accent).show(ui, |ui| {
                ui.label(RichText::new("VIEWS").size(12.0).color(colors.accent));
                ui.label(
                    RichText::new("The views the overview shows")
                        .size(10.0)
                        .color(TEXT_DIM),
                );
                ui.add_space(6.0);
                Grid::new((id, "views"))
                    .num_columns(COLUMNS)
                    .spacing(vec2(8.0, 6.0))
                    .show(ui, |ui| {
                        for (index, module) in Module::ALL.into_iter().enumerate() {
                            let visible = layout.placement(module).visible;
                            let (rect, _) = ui.allocate_exact_size(
                                vec2(BUTTON_WIDTH, CONTROL_HEIGHT),
                                Sense::hover(),
                            );
                            let id = Id::new(("views-toggle", module));
                            if button(ui, id, rect, module.label(), colors.accent, visible)
                                .clicked()
                            {
                                layout.set_visible(module, !visible);
                            }
                            if index % COLUMNS == COLUMNS - 1 {
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(10.0);
                let (rect, _) = ui.allocate_exact_size(
                    vec2(BUTTON_WIDTH * COLUMNS as f32 + 8.0, CONTROL_HEIGHT),
                    Sense::hover(),
                );
                if button(
                    ui,
                    Id::new("views-reset"),
                    rect,
                    "RESET LAYOUT",
                    colors.accent,
                    false,
                )
                .clicked()
                {
                    *layout = OverviewLayout::default();
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("drag a title to move · drag a gap to resize · Esc closes")
                        .size(10.0)
                        .color(TEXT_FAINT),
                );
            });
        })
        .response;
    !should_close(context, response.rect, Some(button_rect))
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
