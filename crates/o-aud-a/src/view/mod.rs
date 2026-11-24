//! The O-AUD-A editor: a header with the pages and the analyzer views.

mod colormap;
mod loudness;
mod menu;
mod oscilloscope;
mod overview;
mod plot;
mod sonogram;
mod sources;
mod spectrum;
mod stereo;
mod surface;

use std::mem;

use aud_ui::theme::{ALERT, BORDER, PANEL, TEXT, TEXT_DIM, TEXT_FAINT, alpha, mono, stroke};
use aud_ui::widgets::{button, pill, segmented, title};
use egui::{Align2, CornerRadius, CursorIcon, Id, Key, Rect, Sense, StrokeKind, Ui, pos2, vec2};

use crate::analysis::bus::SourceReading;
use crate::analysis::{AnalysisShared, Published, SignalReading};
use crate::layout::Module;
use crate::settings::{AnalyzerSettings, Choice, Tab};
use crate::sources::SourceList;
use crate::view::colormap::{Colors, paint};
use crate::view::plot::set_colors;
use crate::view::sources::{Trace, traces};
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
    sources_open: bool,
    /// The bus sources, as last read from the analyzer.
    sources: SourceList,
    /// A bus name being typed in the SOURCES panel.
    bus_draft: String,
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
    /// The views' state for each bus, in the order of `data.sources`.
    buses: Vec<BusViews>,
}

/// The editor state of one bus's views; the image and envelope rings only once the bus
/// sends them.
#[derive(Default)]
struct BusViews {
    name: String,
    sonogram: Option<sonogram::SonogramView>,
    spectral: Option<sonogram::SonogramView>,
    oscilloscope: Option<oscilloscope::OscilloscopeView>,
    stereo: stereo::StereoView,
}

impl BusViews {
    /// Takes the new sonogram columns and envelope buckets out of `signal`.
    fn take(&mut self, signal: &mut SignalReading) {
        for (view, feed) in [
            (&mut self.sonogram, &mut signal.sonogram),
            (&mut self.spectral, &mut signal.spectral),
        ] {
            if view.is_some() || feed.generation != 0 {
                view.get_or_insert_with(Default::default).take_columns(feed);
            }
        }
        if self.oscilloscope.is_some() || !signal.envelope.is_empty() {
            self.oscilloscope
                .get_or_insert_with(Default::default)
                .take_envelope(signal);
        }
    }

    fn sonogram(&mut self) -> &mut sonogram::SonogramView {
        self.sonogram.get_or_insert_with(Default::default)
    }

    fn spectral(&mut self) -> &mut sonogram::SonogramView {
        self.spectral.get_or_insert_with(Default::default)
    }

    fn oscilloscope(&mut self) -> &mut oscilloscope::OscilloscopeView {
        self.oscilloscope.get_or_insert_with(Default::default)
    }

    fn stereo(&mut self) -> &mut stereo::StereoView {
        &mut self.stereo
    }
}

/// Keeps one [`BusViews`] per bus, by name, and hands each its new data.
fn take_bus_views(buses: &mut Vec<BusViews>, sources: &mut [SourceReading]) {
    let same = buses
        .iter()
        .map(|views| views.name.as_str())
        .eq(sources.iter().map(|reading| reading.name.as_str()));
    if !same {
        let mut previous = mem::take(buses);
        *buses = sources
            .iter()
            .map(|reading| {
                previous
                    .iter()
                    .position(|views| views.name == reading.name)
                    .map_or_else(
                        || BusViews {
                            name: reading.name.clone(),
                            ..BusViews::default()
                        },
                        |index| previous.remove(index),
                    )
            })
            .collect();
    }
    for (views, reading) in buses.iter_mut().zip(sources) {
        views.take(&mut reading.signal);
    }
}

/// One view state per trace: `input`'s for the input, `pick` of the bus's for a bus.
/// Traces are in list order, as `buses` is.
fn trace_views<'v, T>(
    traces: &[Trace<'_>],
    input: &'v mut T,
    buses: &'v mut [BusViews],
    pick: fn(&mut BusViews) -> &mut T,
) -> Vec<&'v mut T> {
    let mut input = Some(input);
    let mut buses = buses.iter_mut().enumerate();
    traces
        .iter()
        .filter_map(|trace| match trace.bus {
            None => input.take(),
            Some((index, _)) => buses
                .find(|(position, _)| *position == index)
                .map(|(_, views)| pick(views)),
        })
        .collect()
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
        shared.with_published(|published| state.data.take_from(published));
        state.sonogram.take_columns(&mut state.data.input.sonogram);
        state.spectral.take_columns(&mut state.data.input.spectral);
        state.oscilloscope.take_envelope(&mut state.data.input);
        take_bus_views(&mut state.buses, &mut state.data.sources);
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
    let sources_before = shared.sources();
    state.sources.clone_from(&sources_before);
    let inner = rect.shrink2(vec2(MARGIN, 0.0));
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT));
    let [sources_button, theme_button, views_button] =
        draw_header(ui, header, state, &mut settings, (&colors, window));
    let body = Rect::from_min_max(
        pos2(
            inner.left(),
            inner.top() + HEADER_HEIGHT + TAB_HEIGHT + 10.0,
        ),
        pos2(inner.right(), rect.bottom() - MARGIN),
    );
    let mut page = draw_page(ui, body, state, &mut settings, &colors);
    let popover_open =
        state.theme_open || state.views_open || state.sources_open || state.menu.is_some();
    if state.sources_open {
        let input_color = paint(settings.styles.spectrum.first);
        state.sources_open = sources::sources_panel(
            ui.ctx(),
            sources_button,
            (&mut state.sources, &mut state.bus_draft),
            &state.data,
            (&colors, input_color),
        );
    }
    if state.theme_open {
        state.theme_open = menu::theme_panel(ui.ctx(), theme_button, &mut settings, &colors);
    }
    if state.views_open {
        state.views_open = settings.tab == Tab::Overview
            && menu::views_panel(ui.ctx(), views_button, &mut settings.overview, &colors);
    }
    if let Some(open) = state.menu {
        let (still_open, actions) = menu::view_menu(
            ui.ctx(),
            open,
            &mut state.menu_tab,
            (&mut settings, &mut state.sources),
            &colors,
        );
        if !still_open {
            state.menu = None;
        }
        page = page.or(actions);
    }
    if state.sources != sources_before {
        shared.set_sources(state.sources.clone());
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

/// Draws the header and the page tabs; returns the SOURCES, THEME and VIEWS buttons'
/// rectangles.
fn draw_header(
    ui: &mut Ui,
    rect: Rect,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    (colors, window): (&Colors, &WindowState),
) -> [Rect; 3] {
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
        state.sources_open = false;
        state.menu = None;
    }
    let sources_button = Rect::from_min_size(
        pos2(theme_button.left() - 92.0, centre_y - 11.0),
        vec2(84.0, 22.0),
    );
    let label = match state.sources.buses.len() {
        0 => String::from("SOURCES"),
        count => format!("SOURCES {count}"),
    };
    if button(
        ui,
        Id::new("sources"),
        sources_button,
        &label,
        colors.accent,
        state.sources_open,
    )
    .on_hover_text("Analyze buses from O-AUD-TX next to the input")
    .clicked()
    {
        state.sources_open = !state.sources_open;
        state.theme_open = false;
        state.views_open = false;
        state.menu = None;
    }
    let data = &state.data.input;
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
        pos2(sources_button.left() - 104.0, centre_y - 9.0),
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
            state.sources_open = false;
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
    [sources_button, theme_button, views_button]
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
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let size = pointer - rect.min + vec2(4.0, 4.0);
        window.request((size.x.round() as u32, size.y.round() as u32));
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
        state.sources_open = false;
    }
    let mut actions = Actions::default();
    let styles = &settings.styles;
    let input_color = paint(styles.spectrum.first);
    let ViewState {
        data,
        sources: list,
        buses,
        frozen,
        spectrum: spectrum_view,
        sonogram: sonogram_view,
        spectral: spectral_view,
        stereo: stereo_view,
        oscilloscope: scope_view,
        surface: surface_view,
        ..
    } = state;
    let (data, list, frozen) = (&*data, &*list, *frozen);
    let traces = traces(list, data, module, input_color);
    match module {
        Module::Spectrum => spectrum::draw(
            ui,
            rect,
            spectrum_view,
            &traces,
            (&mut settings.spectrum, list),
            (colors, &styles.spectrum),
            compact,
        ),
        Module::Sonogram => {
            let mut views = trace_views(&traces, sonogram_view, buses, BusViews::sonogram);
            sonogram::draw(
                ui,
                rect,
                (&mut views, &traces),
                &settings.sonogram,
                (colors, &styles.sonogram),
                compact,
            );
        }
        Module::Spectral => {
            let mut views = trace_views(&traces, spectral_view, buses, BusViews::spectral);
            sonogram::draw_spectral(
                ui,
                rect,
                (&mut views, &traces),
                (&settings.spectral_sonogram(), &settings.spectral),
                (colors, &styles.spectral),
            );
        }
        Module::Stereo => {
            let mut views = trace_views(&traces, stereo_view, buses, BusViews::stereo);
            stereo::draw(
                ui,
                rect,
                (&mut views, &traces, data.sequence),
                &mut settings.stereo,
                (colors, &styles.stereo),
                compact,
                frozen,
            );
        }
        Module::Oscilloscope => {
            let mut views = trace_views(&traces, scope_view, buses, BusViews::oscilloscope);
            oscilloscope::draw(
                ui,
                rect,
                (&mut views, &traces, data.sequence),
                &mut settings.oscilloscope,
                (colors, &styles.oscilloscope),
                frozen,
            );
        }
        Module::Loudness => {
            actions.reset_loudness = loudness::draw(
                ui,
                rect,
                &traces,
                &mut settings.loudness,
                (colors, &styles.loudness),
                compact,
            );
        }
        Module::Surface => surface::draw(
            ui,
            rect,
            surface_view,
            &traces,
            &mut settings.surface,
            (colors, &styles.surface, settings.sonogram.lane_names()),
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
        state.sources_open = false;
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
