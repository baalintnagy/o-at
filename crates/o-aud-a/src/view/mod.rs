//! The O-AUD-A editor: a header with the pages, a toolbar per page and the analyzer views.

mod colormap;
mod loudness;
mod oscilloscope;
mod overview;
mod plot;
mod sonogram;
mod spectrum;
mod stereo;
mod surface;

use aud_ui::theme::{ALERT, BORDER, PANEL, TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::{button, pill, segmented, title};
use egui::color_picker::{Alpha, color_edit_button_srgba};
use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Id, Key, Pos2, Rect, Sense, StrokeKind, Ui,
    UiBuilder, pos2, vec2,
};

use crate::analysis::{AnalysisShared, Published};
use crate::layout::Module;
use crate::settings::{
    AnalyzerSettings, Choice, ColorMap, ColorScheme, Range, Rgb, SonogramSettings,
    SpectralSettings, Tab,
};
use crate::view::colormap::{Colors, Palette, rgb};
use crate::view::plot::set_colors;
use crate::window::WindowState;

/// Default logical editor size in pixels.
pub const VIEW_WIDTH: f32 = 1_180.0;
pub const VIEW_HEIGHT: f32 = 760.0;
/// Smallest logical editor size in pixels.
pub const MIN_WIDTH: f32 = 960.0;
pub const MIN_HEIGHT: f32 = 600.0;

const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_HEIGHT: f32 = 52.0;
const TAB_HEIGHT: f32 = 26.0;
const TAB_WIDTH: f32 = 92.0;
const TOOL_HEIGHT: f32 = 22.0;
/// Width of a toolbar slider's track.
const SLIDER_WIDTH: f32 = 110.0;

/// A fraction as a percentage.
fn percent(fraction: f32) -> String {
    format!("{:.0} %", fraction * 100.0)
}

/// Editor state kept between frames.
#[derive(Default)]
pub struct ViewState {
    /// The results being drawn; kept while frozen.
    data: Published,
    frozen: bool,
    colors_open: bool,
    last_dropped: u64,
    overload_s: f32,
    spectrum: spectrum::SpectrumView,
    sonogram: sonogram::SonogramView,
    stereo: stereo::StereoView,
    oscilloscope: oscilloscope::OscilloscopeView,
    surface: surface::SurfaceView,
    overview: overview::OverviewState,
}

/// Draws the editor into `rect`.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ViewState,
    shared: &AnalysisShared,
    window: &WindowState,
) {
    shared.mark_viewed();
    let elapsed_s = ui.input(|input| input.stable_dt).min(0.1);
    if !state.frozen {
        shared.with_published(|published| take(&mut state.data, published));
        state.sonogram.take_columns(&mut state.data);
    }
    if state.data.dropped_frames != state.last_dropped {
        state.last_dropped = state.data.dropped_frames;
        state.overload_s = 2.0;
    }
    state.overload_s = (state.overload_s - elapsed_s).max(0.0);

    let before = shared.settings();
    let mut settings = before;
    let colors = Colors::new(&settings);
    set_colors(ui.ctx(), colors);
    let inner = rect.shrink2(vec2(MARGIN, 0.0));
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT));
    let colors_button = draw_header(ui, header, state, &mut settings, (&colors, window));
    let body = Rect::from_min_max(
        pos2(
            inner.left(),
            inner.top() + HEADER_HEIGHT + TAB_HEIGHT + 10.0,
        ),
        pos2(inner.right(), rect.bottom() - MARGIN),
    );
    let page = draw_page(ui, body, state, &mut settings, &colors);
    let popover_open = state.colors_open;
    if popover_open {
        draw_colors_popover(ui, colors_button, state, &mut settings);
    }
    resize_grip(ui, rect, window, &colors);
    let (f11, escape) =
        ui.input(|input| (input.key_pressed(Key::F11), input.key_pressed(Key::Escape)));
    if f11 || (escape && !popover_open && window.is_full_screen()) {
        window.toggle_full_screen();
    }
    if page.reset_loudness {
        shared.reset_loudness();
    }
    if page.reset_peaks {
        shared.reset_peaks();
    }
    if settings != before {
        shared.edit_settings(|stored| *stored = settings);
    }
}

/// Copies new results, handing over the sonogram columns.
fn take(data: &mut Published, published: &mut Published) {
    if data.sequence == published.sequence {
        return;
    }
    data.sample_rate = published.sample_rate;
    data.channels = published.channels;
    data.spectrum.clone_from(&published.spectrum);
    data.sonogram.generation = published.sonogram.generation;
    data.sonogram.range_hz = published.sonogram.range_hz;
    data.sonogram.scale = published.sonogram.scale;
    data.sonogram
        .columns
        .extend(published.sonogram.columns.drain(..));
    data.surface.clone_from(&published.surface);
    data.scope.clone_from(&published.scope);
    data.stereo = published.stereo;
    data.loudness = published.loudness;
    data.loudness_history
        .clone_from(&published.loudness_history);
    data.dropped_frames = published.dropped_frames;
    data.sequence = published.sequence;
}

/// Resets requested by the user.
#[derive(Clone, Copy, Debug, Default)]
struct Actions {
    reset_loudness: bool,
    reset_peaks: bool,
}

/// Draws the header and the page tabs; returns the COLORS button's rectangle.
fn draw_header(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    (colors, window): (&Colors, &WindowState),
) -> Rect {
    title(ui, rect, "O-AUD-A", "Audio Analyzer", colors.accent);
    let centre_y = rect.center().y;
    let full = Rect::from_min_size(pos2(rect.right() - 96.0, centre_y - 11.0), vec2(96.0, 22.0));
    let full_screen = window.is_full_screen();
    if button(
        ui,
        Id::new("full-screen"),
        full,
        if full_screen {
            "RESTORE"
        } else {
            "FULL SCREEN"
        },
        colors.accent,
        full_screen,
    )
    .on_hover_text(if window.can_full_screen() {
        "Fill the screen with the analyzer (F11); Esc or RESTORE returns"
    } else {
        "Full screen is not available on this platform"
    })
    .clicked()
    {
        window.toggle_full_screen();
    }
    let freeze = Rect::from_min_size(pos2(full.left() - 84.0, centre_y - 11.0), vec2(76.0, 22.0));
    if button(
        ui,
        Id::new("freeze"),
        freeze,
        "FREEZE",
        colors.accent,
        state.frozen,
    )
    .on_hover_text("Hold the display; analysis continues")
    .clicked()
    {
        state.frozen = !state.frozen;
    }
    let colors_button = Rect::from_min_size(
        pos2(freeze.left() - 84.0, centre_y - 11.0),
        vec2(76.0, 22.0),
    );
    if button(
        ui,
        Id::new("colors"),
        colors_button,
        "COLORS",
        colors.accent,
        state.colors_open,
    )
    .on_hover_text("Trace colors and heat map of every view")
    .clicked()
    {
        state.colors_open = !state.colors_open;
    }
    let data = &state.data;
    let (status, color, lit) = if data.sample_rate == 0 {
        ("INACTIVE", TEXT_FAINT, false)
    } else if state.overload_s > 0.0 {
        ("OVERLOAD", ALERT, true)
    } else if state.frozen {
        ("FROZEN", TEXT, true)
    } else if data.stereo.mid_db.max(data.stereo.side_db) > -90.0 {
        ("ANALYZING", colors.accent, true)
    } else {
        ("NO SIGNAL", TEXT_DIM, false)
    };
    let status_rect = Rect::from_min_size(
        pos2(colors_button.left() - 104.0, centre_y - 9.0),
        vec2(92.0, 18.0),
    );
    pill(ui, status_rect, status, color, lit);
    if data.sample_rate > 0 {
        let channels = if data.channels == 1 { "mono" } else { "stereo" };
        ui.painter().text(
            pos2(status_rect.left() - 12.0, centre_y),
            Align2::RIGHT_CENTER,
            format!("{:.1} kHz · {channels}", data.sample_rate as f32 / 1_000.0),
            mono(11.0),
            TEXT_DIM,
        );
    }

    let tabs = Rect::from_min_size(
        pos2(rect.left(), rect.bottom()),
        vec2(TAB_WIDTH * Tab::ALL.len() as f32, TAB_HEIGHT),
    );
    if let Some(index) = segmented(
        ui,
        "o-aud-a-tab",
        tabs,
        Tab::LABELS,
        settings.tab.index(),
        colors.accent,
    ) {
        settings.tab = Tab::from_index(index);
    }
    ui.painter().hline(
        (tabs.right() + GAP)..=rect.right(),
        tabs.center().y,
        stroke(1.0, BORDER),
    );
    colors_button
}

/// The color settings, below the COLORS button and above everything else.
fn draw_colors_popover(
    ui: &mut Ui,
    anchor: Rect,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
) {
    let size = vec2(620.0, 196.0);
    let rect = Rect::from_min_size(
        pos2(anchor.right() + 184.0 - size.x, anchor.bottom() + 8.0),
        size,
    );
    // Covers the page below, so clicks inside do not reach it.
    ui.interact(rect, Id::new("colors-popover"), Sense::click());
    let picking = ui.memory(|memory| memory.any_popup_open());
    let outside = ui.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pointer| !rect.contains(pointer) && !anchor.contains(pointer))
    });
    if (outside && !picking) || ui.input(|input| input.key_pressed(Key::Escape)) {
        state.colors_open = false;
    }
    let colors = Colors::new(settings);
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        PANEL,
        stroke(1.0, colors.accent),
        StrokeKind::Inside,
    );
    let left = rect.left() + 14.0;
    let label_width = 74.0;
    let caption = |ui: &Ui, y: f32, text: &str| {
        ui.painter().text(
            pos2(left, y + 11.0),
            Align2::LEFT_CENTER,
            text,
            font(10.0),
            TEXT_DIM,
        );
    };
    let label = |ui: &Ui, x: f32, y: f32, text: &str| {
        ui.painter().text(
            pos2(x + 30.0, y + 11.0),
            Align2::LEFT_CENTER,
            text,
            font(10.0),
            TEXT_DIM,
        );
    };

    let top = rect.top() + 14.0;
    caption(ui, top, "PALETTE");
    let schemes = Rect::from_min_max(
        pos2(left + label_width, top),
        pos2(rect.right() - 14.0, top + 22.0),
    );
    if let Some(index) = segmented(
        ui,
        "color-scheme",
        schemes,
        ColorScheme::LABELS,
        settings.scheme.index(),
        colors.accent,
    ) {
        settings.scheme = ColorScheme::from_index(index);
    }

    // Each role's color; editing one switches to the CUSTOM scheme.
    let roles_top = top + 34.0;
    caption(ui, roles_top, "COLORS");
    let mut custom = Colors::custom_from(settings.scheme, &settings.custom);
    let mut edited = false;
    let roles: [(&str, &mut Rgb); 6] = [
        ("accent", &mut custom.accent),
        ("left · mid", &mut custom.primary),
        ("right", &mut custom.secondary),
        ("side", &mut custom.side),
        ("plot", &mut custom.plot),
        ("grid", &mut custom.grid),
    ];
    for (index, (text, value)) in roles.into_iter().enumerate() {
        let x = left + label_width + index as f32 * 88.0;
        edited |= color_swatch(ui, pos2(x, roles_top), ("role", index), value);
        label(ui, x, roles_top, text);
    }
    if edited {
        settings.custom = custom;
        settings.scheme = ColorScheme::Custom;
    }

    let heat_top = roles_top + 44.0;
    caption(ui, heat_top, "HEAT MAP");
    let maps = Rect::from_min_max(
        pos2(left + label_width, heat_top),
        pos2(left + label_width + 340.0, heat_top + 22.0),
    );
    if let Some(index) = segmented(
        ui,
        "color-map",
        maps,
        ColorMap::LABELS,
        settings.colormap.index(),
        colors.accent,
    ) {
        settings.colormap = ColorMap::from_index(index);
    }
    let palette = Palette::new(Colors::new(settings).heat);
    let ramp = Rect::from_min_max(
        pos2(maps.right() + 14.0, maps.top() + 4.0),
        pos2(rect.right() - 14.0, maps.bottom() - 4.0),
    );
    let steps = 48;
    for step in 0..steps {
        let fraction = step as f32 / steps as f32;
        let x = ramp.left() + ramp.width() * fraction;
        ui.painter().rect_filled(
            Rect::from_min_max(
                pos2(x, ramp.top()),
                pos2(x + ramp.width() / steps as f32 + 0.5, ramp.bottom()),
            ),
            0.0,
            palette.color(fraction),
        );
    }

    // The custom heat map's stops; editing one switches to the CUSTOM heat map.
    let stops_top = heat_top + 32.0;
    let mut heat = settings.custom.heat;
    let mut heat_edited = false;
    for (index, (text, value)) in ["quiet", "middle", "loud"]
        .into_iter()
        .zip(heat.iter_mut())
        .enumerate()
    {
        let x = left + label_width + index as f32 * 88.0;
        heat_edited |= color_swatch(ui, pos2(x, stops_top), ("heat", index), value);
        label(ui, x, stops_top, text);
    }
    if heat_edited {
        settings.custom.heat = heat;
        settings.colormap = ColorMap::Custom;
    }
    ui.painter().text(
        pos2(left, rect.bottom() - 12.0),
        Align2::LEFT_CENTER,
        "Click a color to edit it; editing switches to CUSTOM. Applies to every view and is \
         saved with the session.",
        font(10.0),
        TEXT_FAINT,
    );
}

/// A color button that opens a picker; returns `true` when the color changed.
fn color_swatch(ui: &mut Ui, position: Pos2, key: (&str, usize), value: &mut Rgb) -> bool {
    let mut color = rgb(*value);
    let rect = Rect::from_min_size(position, vec2(24.0, 22.0));
    let changed = ui
        .allocate_new_ui(UiBuilder::new().max_rect(rect).id_salt(key), |ui| {
            ui.spacing_mut().interact_size = vec2(24.0, 22.0);
            color_edit_button_srgba(ui, &mut color, Alpha::Opaque).changed()
        })
        .inner;
    if changed {
        let [r, g, b, _] = color.to_array();
        *value = [r, g, b];
    }
    changed
}

/// A grip in the lower right corner that resizes the window.
fn resize_grip(ui: &mut Ui, rect: Rect, window: &WindowState, colors: &Colors) {
    if window.is_full_screen() {
        return;
    }
    let corner = Rect::from_min_max(rect.max - vec2(18.0, 18.0), rect.max);
    let response = ui
        .interact(corner, Id::new("resize-grip"), Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeNwSe)
        .on_hover_text("Drag to resize");
    if response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let size = pointer - rect.min + vec2(4.0, 4.0);
            window.request((size.x.round() as u32, size.y.round() as u32));
        }
    }
    let color = if response.hovered() || response.dragged() {
        colors.accent
    } else {
        TEXT_FAINT
    };
    for offset in [4.0, 8.0, 12.0] {
        ui.painter().line_segment(
            [
                pos2(rect.right() - offset, rect.bottom() - 2.0),
                pos2(rect.right() - 2.0, rect.bottom() - offset),
            ],
            stroke(1.2, color),
        );
    }
}

fn draw_page(
    ui: &mut Ui,
    body: Rect,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    colors: &Colors,
) -> Actions {
    if settings.tab == Tab::Overview {
        return overview::draw(ui, body, state, settings, colors);
    }
    let mut toolbar = Toolbar::new(body, colors.accent);
    let mut actions = Actions::default();
    match settings.tab {
        Tab::Overview => {}
        Tab::Spectrum => {
            let spectrum = &mut settings.spectrum;
            toolbar.choice(ui, "FFT", &mut spectrum.fft, 40.0);
            toolbar.choice(ui, "WINDOW", &mut spectrum.window, 64.0);
            toolbar.choice(ui, "CHANNELS", &mut spectrum.channels, 50.0);
            toolbar.choice(ui, "AVERAGE", &mut spectrum.averaging, 42.0);
            toolbar.choice(ui, "SMOOTH", &mut spectrum.smoothing, 40.0);
            toolbar.choice(ui, "SLOPE dB/oct", &mut spectrum.slope, 34.0);
            toolbar.choice(ui, "RANGE", &mut spectrum.range, 52.0);
            toolbar.choice(ui, "TOP", &mut spectrum.limit, 38.0);
            toolbar.choice(ui, "SCALE", &mut spectrum.scale, 38.0);
            toolbar.toggle(ui, "PEAK HOLD", &mut spectrum.peak_hold);
            actions.reset_peaks = toolbar.action(ui, "RESET PEAKS");
        }
        Tab::Sonogram | Tab::Spectral | Tab::Surface => {
            if settings.tab == Tab::Spectral {
                let spectral = &mut settings.spectral;
                toolbar.group(ui, "SONOGRAM LAYER");
                toolbar.slider(
                    ui,
                    "OPACITY",
                    &mut spectral.sonogram_opacity,
                    SpectralSettings::OPACITY,
                    percent,
                );
                toolbar.group(ui, "WAVEFORM LAYER");
                toolbar.slider(
                    ui,
                    "OPACITY",
                    &mut spectral.waveform_opacity,
                    SpectralSettings::OPACITY,
                    percent,
                );
                toolbar.slider(
                    ui,
                    "GAIN",
                    &mut spectral.waveform_gain_db,
                    SpectralSettings::WAVEFORM_GAIN_DB,
                    |db| format!("+{db:.0} dB"),
                );
                toolbar.choice(ui, "SCALE", &mut spectral.scale, 36.0);
                toolbar.break_row();
            }
            if settings.tab == Tab::Surface {
                let surface = &mut settings.surface;
                toolbar.choice(ui, "MODE", &mut surface.mode, 76.0);
                toolbar.choice(ui, "BANDS", &mut surface.bands, 36.0);
                toolbar.choice(ui, "DEPTH", &mut surface.depth, 36.0);
                toolbar.choice(ui, "RANGE", &mut surface.range, 52.0);
                toolbar.choice(ui, "FFT", &mut settings.sonogram.fft, 40.0);
            } else {
                let sonogram = &mut settings.sonogram;
                toolbar.choice(ui, "FFT", &mut sonogram.fft, 40.0);
                toolbar.choice(ui, "SIGNAL", &mut sonogram.signal, 46.0);
                toolbar.choice(ui, "SCALE", &mut sonogram.scale, 38.0);
                toolbar.choice(ui, "SPEED", &mut sonogram.speed, 44.0);
                toolbar.choice(ui, "RANGE", &mut sonogram.range, 52.0);
                toolbar.choice(ui, "TOP", &mut sonogram.limit, 38.0);
                toolbar.slider(
                    ui,
                    "BRIGHTNESS",
                    &mut sonogram.brightness_db,
                    SonogramSettings::BRIGHTNESS_DB,
                    |db| format!("{db:+.0} dB"),
                );
                toolbar.slider(
                    ui,
                    "CONTRAST",
                    &mut sonogram.contrast,
                    SonogramSettings::CONTRAST,
                    |contrast| format!("{contrast:.2}×"),
                );
            }
            toolbar.choice(ui, "HEAT", &mut settings.colormap, 56.0);
            if settings.tab == Tab::Surface && toolbar.action(ui, "RESET VIEW") {
                settings.surface.reset_camera();
            }
        }
        Tab::Stereo => {
            let stereo = &mut settings.stereo;
            toolbar.choice(ui, "SCOPE", &mut stereo.mode, 52.0);
            toolbar.choice(ui, "SAMPLES", &mut stereo.samples, 38.0);
            toolbar.choice(ui, "GAIN", &mut stereo.gain, 56.0);
            toolbar.choice(ui, "SPECTRUM FFT", &mut settings.spectrum.fft, 40.0);
        }
        Tab::Oscilloscope => {
            let scope = &mut settings.oscilloscope;
            toolbar.choice(ui, "TIME ms", &mut scope.span, 34.0);
            toolbar.choice(ui, "CHANNELS", &mut scope.channels, 50.0);
            toolbar.choice(ui, "TRIGGER", &mut scope.trigger, 42.0);
            toolbar.choice(ui, "SOURCE", &mut scope.source, 46.0);
            toolbar.choice(ui, "ZOOM", &mut scope.zoom, 38.0);
            toolbar.choice(ui, "LAYOUT", &mut scope.layout, 62.0);
        }
        Tab::Loudness => {
            let loudness = &mut settings.loudness;
            toolbar.choice(ui, "TARGET LUFS", &mut loudness.target, 44.0);
            toolbar.choice(ui, "SCALE", &mut loudness.scale, 44.0);
            toolbar.choice(ui, "HISTORY", &mut loudness.span, 52.0);
            actions.reset_loudness = toolbar.action(ui, "RESET");
        }
    }
    let area = toolbar.rest(body);
    let module = match settings.tab {
        Tab::Overview | Tab::Spectrum => Module::Spectrum,
        Tab::Sonogram => Module::Sonogram,
        Tab::Spectral => Module::Spectral,
        Tab::Stereo => Module::Stereo,
        Tab::Oscilloscope => Module::Oscilloscope,
        Tab::Loudness => Module::Loudness,
        Tab::Surface => Module::Surface,
    };
    draw_module(ui, area, module, state, settings, colors, false);
    actions
}

/// Draws one view into `rect`; `compact` leaves out details for small panels.
fn draw_module(
    ui: &mut Ui,
    rect: Rect,
    module: Module,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    colors: &Colors,
    compact: bool,
) {
    let data = &state.data;
    match module {
        Module::Spectrum => spectrum::draw(
            ui,
            rect,
            &mut state.spectrum,
            data,
            &mut settings.spectrum,
            colors,
            compact,
        ),
        Module::Sonogram => sonogram::draw(
            ui,
            rect,
            &mut state.sonogram,
            data,
            &settings.sonogram,
            colors,
            compact,
        ),
        Module::Spectral => sonogram::draw_spectral(
            ui,
            rect,
            &mut state.sonogram,
            data,
            (&settings.sonogram, &settings.spectral),
            colors,
        ),
        Module::Stereo => stereo::draw(
            ui,
            rect,
            &mut state.stereo,
            data,
            &mut settings.stereo,
            colors,
            compact,
            state.frozen,
        ),
        Module::Oscilloscope => oscilloscope::draw(
            ui,
            rect,
            &mut state.oscilloscope,
            data,
            &mut settings.oscilloscope,
            colors,
            state.frozen,
        ),
        Module::Loudness => loudness::draw(ui, rect, data, &mut settings.loudness, colors, compact),
        Module::Surface => surface::draw(
            ui,
            rect,
            &mut state.surface,
            data,
            &mut settings.surface,
            colors,
        ),
    }
}

/// Lays out labelled controls from left to right, wrapping into new rows.
struct Toolbar {
    rect: Rect,
    x: f32,
    y: f32,
    accent: Color32,
}

impl Toolbar {
    const GAP: f32 = 16.0;
    const ROW: f32 = TOOL_HEIGHT + 8.0;

    fn new(rect: Rect, accent: Color32) -> Self {
        Self {
            rect,
            x: rect.left(),
            y: rect.top(),
            accent,
        }
    }

    /// Places an item `width` wide, wrapping if needed; returns its rectangle.
    fn place(&mut self, width: f32) -> Rect {
        if self.x > self.rect.left() && self.x + width > self.rect.right() {
            self.x = self.rect.left();
            self.y += Self::ROW;
        }
        let rect = Rect::from_min_size(pos2(self.x, self.y), vec2(width, TOOL_HEIGHT));
        self.x += width + Self::GAP;
        rect
    }

    /// Caption and segmented selector for a choice; `segment` is the width per choice.
    fn choice<C: Choice>(&mut self, ui: &mut Ui, caption: &str, value: &mut C, segment: f32) {
        let caption_width = ui
            .painter()
            .layout_no_wrap(caption.to_owned(), font(10.0), TEXT_DIM)
            .size()
            .x;
        let width = caption_width + 6.0 + segment * C::LABELS.len() as f32;
        let rect = self.place(width);
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            caption,
            font(10.0),
            TEXT_DIM,
        );
        let selector = Rect::from_min_max(
            pos2(rect.left() + caption_width + 6.0, rect.top()),
            rect.max,
        );
        if let Some(index) = segmented(ui, caption, selector, C::LABELS, value.index(), self.accent)
        {
            *value = C::from_index(index);
        }
    }

    /// An on/off button `width` wide.
    fn toggle_sized(&mut self, ui: &mut Ui, label: &str, value: &mut bool, width: f32) {
        let rect = self.place(width);
        if button(
            ui,
            Id::new(("tool-toggle", label)),
            rect,
            label,
            self.accent,
            *value,
        )
        .clicked()
        {
            *value = !*value;
        }
    }

    /// An on/off button.
    fn toggle(&mut self, ui: &mut Ui, label: &str, value: &mut bool) {
        self.toggle_sized(ui, label, value, 84.0);
    }

    /// A push button; `true` when clicked.
    fn action(&mut self, ui: &mut Ui, label: &str) -> bool {
        let rect = self.place(92.0);
        button(
            ui,
            Id::new(("tool-action", label)),
            rect,
            label,
            self.accent,
            false,
        )
        .clicked()
    }

    /// Starts the next control on a new row.
    fn break_row(&mut self) {
        if self.x > self.rect.left() {
            self.x = self.rect.left();
            self.y += Self::ROW;
        }
    }

    /// A group heading for the controls after it.
    fn group(&mut self, ui: &Ui, text: &str) {
        let width = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font(10.0), self.accent)
            .size()
            .x;
        let rect = self.place(width);
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            text,
            font(10.0),
            self.accent,
        );
    }

    /// Caption and horizontal slider: drag or scroll to change, double-click to reset.
    fn slider(
        &mut self,
        ui: &mut Ui,
        caption: &str,
        value: &mut f32,
        range: Range,
        format: impl Fn(f32) -> String,
    ) {
        let caption_width = ui
            .painter()
            .layout_no_wrap(caption.to_owned(), font(10.0), TEXT_DIM)
            .size()
            .x;
        let rect = self.place(caption_width + 6.0 + SLIDER_WIDTH);
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            caption,
            font(10.0),
            TEXT_DIM,
        );
        let track = Rect::from_min_max(
            pos2(rect.left() + caption_width + 6.0, rect.top()),
            rect.max,
        );
        let id = Id::new((
            "tool-slider",
            caption,
            rect.left() as i32,
            rect.top() as i32,
        ));
        let response = ui
            .interact(track, id, Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag or scroll; double-click to reset");
        if response.double_clicked() {
            *value = range.default;
        } else if response.dragged() || response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                *value = range.value((pointer.x - track.left()) / track.width());
            }
        } else if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                *value = range.value(range.position(*value) + scroll.signum() * 0.02);
            }
        }
        *value = range.clamp(*value);
        let fraction = range.position(*value);
        let painter = ui.painter();
        painter.rect(
            track,
            CornerRadius::same(4),
            PANEL,
            stroke(
                1.0,
                if response.hovered() {
                    self.accent
                } else {
                    BORDER
                },
            ),
            StrokeKind::Inside,
        );
        let filled = Rect::from_min_max(
            track.min,
            pos2(track.left() + track.width() * fraction, track.bottom()),
        );
        painter.rect_filled(
            filled.shrink(2.0),
            CornerRadius::same(3),
            alpha(self.accent, 0.3),
        );
        painter.text(
            track.center(),
            Align2::CENTER_CENTER,
            format(*value),
            mono(10.0),
            TEXT,
        );
    }

    /// A dim note after the controls.
    fn note(&mut self, ui: &Ui, text: &str) {
        let width = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font(10.0), TEXT_FAINT)
            .size()
            .x;
        let rect = self.place(width);
        ui.painter().text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            text,
            font(10.0),
            TEXT_FAINT,
        );
    }

    /// The part of `area` below the toolbar.
    fn rest(&self, area: Rect) -> Rect {
        Rect::from_min_max(pos2(area.left(), self.y + Self::ROW + 2.0), area.max)
    }
}
