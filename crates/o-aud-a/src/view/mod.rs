//! The O-AUD-A editor: a header with the pages and the analyzer views.

mod colormap;
mod loudness;
mod menu;
mod oscilloscope;
mod overview;
mod plot;
mod sonogram;
mod spectrum;
mod stereo;
mod surface;

use aud_ui::theme::{ALERT, BORDER, PANEL, TEXT, TEXT_DIM, TEXT_FAINT, alpha, mono, stroke};
use aud_ui::widgets::{button, pill, segmented, title};
use egui::{Align2, CornerRadius, CursorIcon, Id, Key, Rect, Sense, StrokeKind, Ui, pos2, vec2};

use crate::analysis::spectrum::SonogramFeed;
use crate::analysis::{AnalysisShared, Published};
use crate::layout::Module;
use crate::settings::{AnalyzerSettings, Choice, Signal, SonogramSettings, Tab};
use crate::view::colormap::Colors;
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
/// Width of the settings button in a panel's header, in logical pixels.
pub const SETTINGS_BUTTON_WIDTH: f32 = 26.0;

/// Editor state kept between frames.
#[derive(Default)]
pub struct ViewState {
    /// The results being drawn; kept while frozen.
    data: Published,
    frozen: bool,
    theme_open: bool,
    /// Whether the overview's VIEWS panel is open.
    views_open: bool,
    /// The open view popup.
    menu: Option<menu::ViewMenu>,
    /// The tab view popups open with: the one used last.
    menu_tab: menu::MenuTab,
    last_dropped: u64,
    overload_s: f32,
    spectrum: spectrum::SpectrumView,
    sonogram: sonogram::SonogramView,
    /// The spectral view's own sonogram.
    spectral: sonogram::SonogramView,
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
        state.sonogram.take_columns(&mut state.data.sonogram);
        state.spectral.take_columns(&mut state.data.spectral);
        state.oscilloscope.take_envelope(&mut state.data);
    }
    if state.data.dropped_frames != state.last_dropped {
        state.last_dropped = state.data.dropped_frames;
        state.overload_s = 2.0;
    }
    state.overload_s = (state.overload_s - elapsed_s).max(0.0);

    let before = shared.settings();
    let mut settings = before;
    settings.upgrade();
    let colors = Colors::new(&settings);
    set_colors(ui.ctx(), colors);
    let inner = rect.shrink2(vec2(MARGIN, 0.0));
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT));
    let (theme_button, views_button) =
        draw_header(ui, header, state, &mut settings, (&colors, window));
    let body = Rect::from_min_max(
        pos2(
            inner.left(),
            inner.top() + HEADER_HEIGHT + TAB_HEIGHT + 10.0,
        ),
        pos2(inner.right(), rect.bottom() - MARGIN),
    );
    let mut page = draw_page(ui, body, state, &mut settings, &colors);
    let popover_open = state.theme_open || state.views_open || state.menu.is_some();
    if state.theme_open {
        state.theme_open = menu::theme_panel(ui.ctx(), theme_button, &mut settings, &colors);
    }
    if state.views_open {
        state.views_open = settings.tab == Tab::Overview
            && menu::views_panel(ui.ctx(), views_button, &mut settings.overview, &colors);
    }
    if let Some(open) = state.menu {
        let (still_open, actions) =
            menu::view_menu(ui.ctx(), open, &mut state.menu_tab, &mut settings, &colors);
        if !still_open {
            state.menu = None;
        }
        page = page.or(actions);
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
    take_feed(&mut data.sonogram, &mut published.sonogram);
    take_feed(&mut data.spectral, &mut published.spectral);
    data.surface.clone_from(&published.surface);
    data.scope.clone_from(&published.scope);
    data.stereo = published.stereo;
    data.loudness = published.loudness;
    data.loudness_history
        .clone_from(&published.loudness_history);
    data.envelope_generation = published.envelope_generation;
    data.envelope.extend(published.envelope.drain(..));
    data.dropped_frames = published.dropped_frames;
    data.sequence = published.sequence;
}

/// Copies a sonogram feed, handing over its columns.
fn take_feed(data: &mut SonogramFeed, published: &mut SonogramFeed) {
    data.generation = published.generation;
    data.range_hz = published.range_hz;
    data.scale = published.scale;
    data.lanes = published.lanes;
    data.columns.extend(published.columns.drain(..));
}

/// Resets requested by the user.
#[derive(Clone, Copy, Debug, Default)]
struct Actions {
    reset_loudness: bool,
    reset_peaks: bool,
}

impl Actions {
    /// The resets either asks for.
    fn or(self, other: Self) -> Self {
        Self {
            reset_loudness: self.reset_loudness || other.reset_loudness,
            reset_peaks: self.reset_peaks || other.reset_peaks,
        }
    }
}

/// Draws the header and the page tabs; returns the THEME and VIEWS buttons' rectangles.
fn draw_header(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    (colors, window): (&Colors, &WindowState),
) -> (Rect, Rect) {
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
    let theme_button = Rect::from_min_size(
        pos2(freeze.left() - 84.0, centre_y - 11.0),
        vec2(76.0, 22.0),
    );
    if button(
        ui,
        Id::new("theme"),
        theme_button,
        "THEME",
        colors.accent,
        state.theme_open,
    )
    .on_hover_text("Colors of the controls and of every diagram's background and grid")
    .clicked()
    {
        state.theme_open = !state.theme_open;
        state.views_open = false;
        state.menu = None;
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
        pos2(theme_button.left() - 104.0, centre_y - 9.0),
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
    // The overview's VIEWS button at the right end of the tab row.
    let views_button = Rect::from_min_size(
        pos2(rect.right() - 96.0, tabs.center().y - 11.0),
        vec2(96.0, 22.0),
    );
    let line_end = if settings.tab == Tab::Overview {
        if button(
            ui,
            Id::new("views"),
            views_button,
            "VIEWS",
            colors.accent,
            state.views_open,
        )
        .on_hover_text("Show or hide views and reset the layout")
        .clicked()
        {
            state.views_open = !state.views_open;
            state.theme_open = false;
            state.menu = None;
        }
        views_button.left() - GAP
    } else {
        rect.right()
    };
    ui.painter().hline(
        (tabs.right() + GAP)..=line_end,
        tabs.center().y,
        stroke(1.0, BORDER),
    );
    (theme_button, views_button)
}

/// The view a page shows; `None` for the overview.
fn page_module(tab: Tab) -> Option<Module> {
    match tab {
        Tab::Overview => None,
        Tab::Spectral => Some(Module::Spectral),
        Tab::Spectrum => Some(Module::Spectrum),
        Tab::Sonogram => Some(Module::Sonogram),
        Tab::Oscilloscope => Some(Module::Oscilloscope),
        Tab::Stereo => Some(Module::Stereo),
        Tab::Loudness => Some(Module::Loudness),
        Tab::Surface => Some(Module::Surface),
    }
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
    match page_module(settings.tab) {
        None => overview::draw(ui, body, state, settings, colors),
        Some(module) => draw_module(ui, body, module, state, settings, (colors, false)),
    }
}

/// Draws one view into `rect`; `compact` leaves out details for small panels.
fn draw_module(
    ui: &mut Ui,
    rect: Rect,
    module: Module,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    (colors, compact): (&Colors, bool),
) -> Actions {
    // A right click on a view opens its popup there.
    let right_click = ui.input(|input| {
        input
            .pointer
            .secondary_clicked()
            .then(|| input.pointer.interact_pos())
            .flatten()
    });
    if let Some(pointer) = right_click.filter(|pointer| rect.contains(*pointer)) {
        state.menu = Some(menu::ViewMenu {
            module,
            at: pointer,
            pivot: Align2::LEFT_TOP,
            opener: None,
        });
        state.theme_open = false;
        state.views_open = false;
    }
    let mut actions = Actions::default();
    let styles = &settings.styles;
    let data = &state.data;
    match module {
        Module::Spectrum => spectrum::draw(
            ui,
            rect,
            &mut state.spectrum,
            data,
            &mut settings.spectrum,
            (colors, &styles.spectrum),
            compact,
        ),
        Module::Sonogram => sonogram::draw(
            ui,
            rect,
            &mut state.sonogram,
            data,
            &settings.sonogram,
            (colors, &styles.sonogram),
            compact,
        ),
        Module::Spectral => sonogram::draw_spectral(
            ui,
            rect,
            &mut state.spectral,
            data,
            (&settings.spectral_sonogram(), &settings.spectral),
            (colors, &styles.spectral),
        ),
        Module::Stereo => stereo::draw(
            ui,
            rect,
            &mut state.stereo,
            data,
            &mut settings.stereo,
            (colors, &styles.stereo),
            compact,
            state.frozen,
        ),
        Module::Oscilloscope => oscilloscope::draw(
            ui,
            rect,
            &mut state.oscilloscope,
            data,
            &mut settings.oscilloscope,
            (colors, &styles.oscilloscope),
            state.frozen,
        ),
        Module::Loudness => {
            actions.reset_loudness = loudness::draw(
                ui,
                rect,
                data,
                &mut settings.loudness,
                (colors, &styles.loudness),
                compact,
            );
        }
        Module::Surface => surface::draw(
            ui,
            rect,
            &mut state.surface,
            data,
            &mut settings.surface,
            (colors, &styles.surface, lane_names(&settings.sonogram)),
        ),
    }
    settings_button(ui, rect, module, state, colors);
    actions
}

/// The button at the right end of a panel's header that opens and closes its popup.
fn settings_button(
    ui: &mut Ui,
    panel: Rect,
    module: Module,
    state: &mut ViewState,
    colors: &Colors,
) {
    let rect = settings_button_rect(panel);
    let open = state.menu.is_some_and(|menu| menu.module == module);
    let response = ui
        .interact(rect, Id::new(("settings-button", module)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Settings and colors of this view (or right-click it)");
    if response.clicked() {
        state.menu = (!open).then_some(menu::ViewMenu {
            module,
            at: rect.right_bottom() + vec2(0.0, 6.0),
            pivot: Align2::RIGHT_TOP,
            opener: Some(rect),
        });
        state.theme_open = false;
        state.views_open = false;
    }
    let lit = open || response.hovered();
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(4),
        if open {
            alpha(colors.accent, 0.22)
        } else {
            PANEL
        },
        stroke(1.0, if lit { colors.accent } else { BORDER }),
        StrokeKind::Inside,
    );
    // Three slider tracks with their knobs.
    let color = if lit { colors.accent } else { TEXT_DIM };
    for (row, knob) in [(-4.0, 0.3), (0.0, 0.7), (4.0, 0.45)] {
        let y = rect.center().y + row;
        let x_range = (rect.left() + 6.0)..=(rect.right() - 6.0);
        painter.hline(x_range.clone(), y, stroke(1.0, alpha(color, 0.7)));
        let x = x_range.start() + (x_range.end() - x_range.start()) * knob;
        painter.circle_filled(pos2(x, y), 1.8, color);
    }
}

/// Where a panel's settings button is.
fn settings_button_rect(panel: Rect) -> Rect {
    Rect::from_min_size(
        pos2(
            panel.right() - 12.0 - SETTINGS_BUTTON_WIDTH,
            panel.top() + 8.0,
        ),
        vec2(SETTINGS_BUTTON_WIDTH, 18.0),
    )
}

/// Names of the sonogram lanes' signals; the second is empty with one lane.
fn lane_names(sonogram: &SonogramSettings) -> [&'static str; 2] {
    let (first, second) = sonogram.signals();
    [
        Signal::LABELS[first.index()],
        second.map_or("", |signal| Signal::LABELS[signal.index()]),
    ]
}
