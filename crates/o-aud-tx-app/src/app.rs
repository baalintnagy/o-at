//! The application window: a list of routes on the left; on the right, the shared
//! O-AUD-TX editor body above an audio source panel, for the selected route.

use std::sync::{Arc, Mutex, PoisonError};

use aud_ui::theme::{
    self, ALERT, BACKGROUND, PANEL_RAISED, SEND, TEXT, TEXT_DIM, TEXT_FAINT, TRACK, alpha, font,
    mono, stroke,
};
use aud_ui::widgets::{METER_FLOOR_DB, button, list_row, panel, readout};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, CentralPanel, Color32, Context, CornerRadius, CursorIcon, FontId, Frame, Id, Painter,
    Pos2, Rect, ScrollArea, Sense, StrokeKind, Ui, UiBuilder, pos2, vec2,
};
use o_aud_tx::link::TxState;
use o_aud_tx::view::{self, TxControls, VIEW_HEIGHT, VIEW_WIDTH};

use crate::application::matches;
use crate::capture::CaptureStatus;
use crate::route::Route;
use crate::settings::{RouteSettings, Source};

/// Height of the source panel in logical pixels.
const SOURCE_HEIGHT: f32 = 236.0;
/// Width of the route list column in logical pixels.
const ROUTES_WIDTH: f32 = 240.0;
/// Logical window size in pixels.
pub const WINDOW_WIDTH: f32 = ROUTES_WIDTH + VIEW_WIDTH;
pub const WINDOW_HEIGHT: f32 = VIEW_HEIGHT + SOURCE_HEIGHT;

const MARGIN: f32 = 16.0;
/// Top of the route list, level with the editor body below its header.
const ROUTES_TOP: f32 = 52.0;
const ROUTE_HEIGHT: f32 = 48.0;
/// Room for the state label at the right of a route row, in logical pixels.
const ROUTE_STATE_WIDTH: f32 = 84.0;
const ROW_HEIGHT: f32 = 26.0;
const SECTION_HEIGHT: f32 = 20.0;
const INFO_WIDTH: f32 = 220.0;
/// Longest editor frame fed to the meters, in seconds.
const MAX_FRAME_S: f32 = 0.1;

/// The routes' settings and the selected route, kept for saving when the window closes.
#[derive(Default)]
pub struct Snapshot {
    pub routes: Vec<Arc<RouteSettings>>,
    pub selected: usize,
}

pub struct App {
    /// At least one route.
    routes: Vec<Route>,
    /// Index into `routes` of the route shown in the editor.
    selected: usize,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl App {
    /// Starts the routes in `snapshot` and keeps it up to date. Without routes, starts
    /// a default one.
    pub fn new(snapshot: Arc<Mutex<Snapshot>>) -> Self {
        let (routes, selected) = {
            let snapshot = snapshot.lock().unwrap_or_else(PoisonError::into_inner);
            let routes: Vec<Route> = snapshot
                .routes
                .iter()
                .map(|settings| Route::start(Arc::clone(settings)))
                .collect();
            (routes, snapshot.selected)
        };
        let mut app = Self {
            routes,
            selected,
            snapshot,
        };
        if app.routes.is_empty() {
            app.add_route();
        }
        app.selected = app.selected.min(app.routes.len() - 1);
        app.publish();
        app
    }

    pub fn setup(context: &Context) {
        theme::apply(context);
    }

    pub fn update(&mut self, context: &Context) {
        CentralPanel::default()
            .frame(Frame::NONE.fill(BACKGROUND))
            .show(context, |ui| {
                let full = ui.max_rect();
                let elapsed_s = ui.input(|input| input.stable_dt).min(MAX_FRAME_S);
                // The editor updates the shown route's meters; the others keep up here.
                for (index, route) in self.routes.iter_mut().enumerate() {
                    if index != self.selected {
                        route.view.update_meters(&route.telemetry, elapsed_s);
                    }
                }
                let routes = Rect::from_min_max(
                    pos2(full.left() + MARGIN, full.top() + ROUTES_TOP),
                    pos2(full.left() + ROUTES_WIDTH, full.bottom() - MARGIN),
                );
                self.draw_routes(ui, routes);

                let right =
                    Rect::from_min_max(pos2(full.left() + ROUTES_WIDTH, full.top()), full.max);
                let transmitter = Rect::from_min_size(right.min, vec2(right.width(), VIEW_HEIGHT));
                let Route {
                    settings,
                    link,
                    telemetry,
                    view: state,
                    ..
                } = &mut self.routes[self.selected];
                let controls = TxControls {
                    gain: &settings.gain_db,
                    send: &settings.send,
                    thru: None,
                };
                view::draw(ui, transmitter, state, &controls, link, telemetry);
                let sources = Rect::from_min_max(
                    pos2(right.left() + MARGIN, transmitter.bottom() - 4.0),
                    pos2(right.right() - MARGIN, right.bottom() - MARGIN),
                );
                draw_sources(ui, sources, &self.routes[self.selected]);
            });
        context.request_repaint();
    }

    /// Updates the snapshot after routes were added, removed or selected.
    fn publish(&self) {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        snapshot.routes = self
            .routes
            .iter()
            .map(|route| Arc::clone(&route.settings))
            .collect();
        snapshot.selected = self.selected;
    }

    /// Adds a route from the default input to a new bus, and shows it.
    fn add_route(&mut self) {
        self.routes
            .push(Route::start(Arc::new(RouteSettings::default())));
        self.selected = self.routes.len() - 1;
        self.publish();
    }

    /// Stops and removes the shown route, keeping at least one.
    fn remove_selected(&mut self) {
        if self.routes.len() > 1 {
            // Dropping the route closes its capture, then its link frees the bus.
            drop(self.routes.remove(self.selected));
            self.selected = self.selected.min(self.routes.len() - 1);
            self.publish();
        }
    }

    fn draw_routes(&mut self, ui: &mut Ui, rect: Rect) {
        let live = self
            .routes
            .iter()
            .filter(|route| route.link.status().state == TxState::Live)
            .count();
        ui.painter().text(
            pos2(rect.left() + 2.0, rect.top() - 18.0),
            Align2::LEFT_CENTER,
            format!("{} routes · {live} live", self.routes.len()),
            mono(11.0),
            TEXT_FAINT,
        );
        panel(ui, rect, "ROUTES", SEND);
        let buttons_top = rect.bottom() - 34.0;
        let list = Rect::from_min_max(
            rect.min + vec2(10.0, 32.0),
            pos2(rect.right() - 10.0, buttons_top - 8.0),
        );
        let mut chosen = None;
        ui.allocate_new_ui(UiBuilder::new().max_rect(list), |ui| {
            ScrollArea::vertical()
                .id_salt("routes")
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (index, route) in self.routes.iter().enumerate() {
                        if route_row(ui, index, route, index == self.selected) {
                            chosen = Some(index);
                        }
                    }
                });
        });
        if let Some(index) = chosen {
            self.selected = index;
            self.publish();
        }

        let width = (rect.width() - 28.0) / 2.0;
        let add = Rect::from_min_size(pos2(rect.left() + 10.0, buttons_top), vec2(width, 24.0));
        let remove = Rect::from_min_size(pos2(add.right() + 8.0, buttons_top), vec2(width, 24.0));
        if button(ui, Id::new("route-add"), add, "+ ADD", SEND, false).clicked() {
            self.add_route();
        }
        let color = if self.routes.len() > 1 {
            ALERT
        } else {
            TEXT_FAINT
        };
        if button(ui, Id::new("route-remove"), remove, "REMOVE", color, false).clicked() {
            self.remove_selected();
        }
    }
}

/// Draws one route, with its bus, source, state and level, and returns whether it was
/// newly chosen.
fn route_row(ui: &mut Ui, index: usize, route: &Route, selected: bool) -> bool {
    let size = vec2(ui.available_width(), ROUTE_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui
        .interact(rect, Id::new(("route", index)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();
    let (fill, border) = if selected {
        (alpha(SEND, 0.18), SEND)
    } else if response.hovered() {
        (PANEL_RAISED, PANEL_RAISED)
    } else {
        (PANEL_RAISED, BACKGROUND)
    };
    painter.rect(
        rect,
        CornerRadius::same(4),
        fill,
        stroke(1.0, border),
        StrokeKind::Inside,
    );

    let status = route.link.status();
    let sending = route.settings.gain() > 0.0;
    let (state, color) = match status.state {
        TxState::Live if sending => ("LIVE", SEND),
        TxState::Live => ("SEND OFF", TEXT_DIM),
        TxState::Inactive => ("IDLE", TEXT_FAINT),
        TxState::Taken { .. } => ("NAME TAKEN", ALERT),
        TxState::InvalidName(_) | TxState::Error(_) => ("ERROR", ALERT),
    };
    let left = rect.left() + 22.0;
    painter.circle_filled(pos2(rect.left() + 11.0, rect.top() + 14.0), 3.5, color);
    let bus = route.link.name();
    let (bus, bus_color) = match (bus.is_empty(), selected) {
        (true, _) => (String::from("New bus"), TEXT_DIM),
        (false, true) => (bus, SEND),
        (false, false) => (bus, TEXT),
    };
    let right = rect.right() - 10.0;
    truncated_text(
        painter,
        pos2(left, rect.top() + 14.0),
        bus,
        font(12.0),
        bus_color,
        right - ROUTE_STATE_WIDTH - left,
    );
    painter.text(
        pos2(right, rect.top() + 14.0),
        Align2::RIGHT_CENTER,
        state,
        mono(10.0),
        color,
    );
    truncated_text(
        painter,
        pos2(left, rect.top() + 30.0),
        source_summary(&route.settings.source()),
        font(10.0),
        TEXT_DIM,
        right - left,
    );
    let meter = Rect::from_min_max(
        pos2(left, rect.bottom() - 8.0),
        pos2(rect.right() - 10.0, rect.bottom() - 5.0),
    );
    painter.rect_filled(meter, CornerRadius::same(1), TRACK);
    let fraction = (1.0 - route.view.level_db() / METER_FLOOR_DB).clamp(0.0, 1.0);
    if fraction > 0.0 {
        let level = Rect::from_min_size(meter.min, vec2(meter.width() * fraction, meter.height()));
        painter.rect_filled(level, CornerRadius::same(1), SEND);
    }
    response.clicked() && !selected
}

/// Draws one line of `text` from `left_centre`, ending in an ellipsis where it would
/// exceed `max_width` logical pixels.
fn truncated_text(
    painter: &Painter,
    left_centre: Pos2,
    text: String,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) {
    let mut job = LayoutJob::simple_singleline(text, font_id, color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = painter.layout_job(job);
    let top_left = pos2(left_centre.x, left_centre.y - galley.size().y / 2.0);
    painter.galley(top_left, galley, color);
}

/// A short description of a source, for the route list.
fn source_summary(source: &Source) -> String {
    match source {
        Source::Input(None) => String::from("Default input"),
        Source::Input(Some(device)) => format!("In · {device}"),
        Source::Loopback(None) => String::from("Default output"),
        Source::Loopback(Some(device)) => format!("Out · {device}"),
        Source::Application(executable) => format!("App · {executable}"),
    }
}

fn draw_sources(ui: &mut Ui, rect: Rect, route: &Route) {
    panel(ui, rect, "AUDIO SOURCE", SEND);
    let capture = route.capture();
    let status = capture.status();
    let list = Rect::from_min_max(
        rect.min + vec2(12.0, 32.0),
        pos2(rect.right() - INFO_WIDTH - 12.0, rect.bottom() - 10.0),
    );
    let selected = route.settings.source();
    let mut choice = None;
    ui.allocate_new_ui(UiBuilder::new().max_rect(list), |ui| {
        ScrollArea::vertical()
            .id_salt("sources")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (heading, rows) in sections(&status, &selected) {
                    section_heading(ui, heading);
                    if rows.is_empty() {
                        empty_row(ui, "No application has audio");
                    }
                    for row in rows {
                        if source_row(ui, &row, &selected, &status) {
                            choice = Some(row.source);
                        }
                    }
                }
            });
    });
    if let Some(source) = choice {
        route.settings.set_source(source);
        capture.reopen();
    }
    draw_source_info(ui, rect, route, &status);
}

fn draw_source_info(ui: &mut Ui, rect: Rect, route: &Route, status: &CaptureStatus) {
    let capture = route.capture();
    let left = rect.right() - INFO_WIDTH;
    let top = rect.top() + 36.0;
    let format = if status.sample_rate > 0 {
        format!(
            "{:.1} kHz · {} ch",
            status.sample_rate as f32 / 1_000.0,
            status.channels
        )
    } else {
        String::from("–")
    };
    readout(ui, pos2(left, top), "Format", &format, TEXT);
    let block = capture.block_frames();
    let block_text = if block > 0 && status.sample_rate > 0 {
        format!(
            "{block} frames · {:.1} ms",
            1_000.0 * block as f32 / status.sample_rate as f32
        )
    } else {
        String::from("–")
    };
    readout(
        ui,
        pos2(left, top + 40.0),
        "Device block",
        &block_text,
        TEXT,
    );
    let process = status
        .process_id
        .map(|process_id| format!("Process {process_id} and its children"))
        .unwrap_or_default();
    let notes: [(&str, _); 3] = match (&status.error, &status.active) {
        (Some(error), _) => [(error.as_str(), ALERT), ("", TEXT_DIM), ("", TEXT_DIM)],
        (None, Some(Source::Loopback(_))) if status.kept_alive => [
            ("Sends everything this device plays", TEXT_DIM),
            ("Play the bus elsewhere: feedback", TEXT_DIM),
            ("", TEXT_DIM),
        ],
        (None, Some(Source::Loopback(_))) => [
            ("Sends everything this device plays", TEXT_DIM),
            ("Pauses while the device is silent", ALERT),
            ("", TEXT_DIM),
        ],
        (None, Some(Source::Application(_))) => [
            ("Sends what this application plays", TEXT_DIM),
            (process.as_str(), TEXT_DIM),
            ("Sends silence while it is quiet", TEXT_DIM),
        ],
        (None, _) => [
            ("Sends the device's input", TEXT_DIM),
            ("", TEXT_DIM),
            ("", TEXT_DIM),
        ],
    };
    for (index, (note, color)) in notes.into_iter().enumerate() {
        ui.painter().text(
            pos2(left, top + 88.0 + 16.0 * index as f32),
            Align2::LEFT_CENTER,
            note,
            font(10.0),
            color,
        );
    }
    let refresh = Rect::from_min_size(pos2(left, rect.bottom() - 34.0), vec2(96.0, 22.0));
    if button(
        ui,
        Id::new("refresh-devices"),
        refresh,
        "REFRESH",
        SEND,
        false,
    )
    .clicked()
    {
        capture.refresh_devices();
    }
    ui.painter().text(
        pos2(refresh.right() + 10.0, refresh.center().y),
        Align2::LEFT_CENTER,
        format!("{} devices", status.inputs.len() + status.outputs.len()),
        mono(10.0),
        TEXT_FAINT,
    );
}

/// One selectable source in the list.
struct Row<'a> {
    source: Source,
    title: &'a str,
    /// Shown when the source is not in use.
    detail: &'static str,
}

/// The source list's sections: inputs and, on Windows, outputs and applications, which
/// need WASAPI loopback. The selected application stays listed while it is not running.
fn sections<'a>(
    status: &'a CaptureStatus,
    selected: &'a Source,
) -> Vec<(&'static str, Vec<Row<'a>>)> {
    let devices = |default: &'a str, names: &'a [String], loopback: bool| -> Vec<Row<'a>> {
        let device = |name: Option<&String>| {
            let device = name.cloned();
            if loopback {
                Source::Loopback(device)
            } else {
                Source::Input(device)
            }
        };
        let rows = names.iter().map(move |name| Row {
            source: device(Some(name)),
            title: name,
            detail: "",
        });
        let default = Row {
            source: device(None),
            title: default,
            detail: "",
        };
        Some(default).into_iter().chain(rows).collect()
    };
    let mut sections = vec![(
        "INPUTS",
        devices("System default input", &status.inputs, false),
    )];
    if cfg!(windows) {
        sections.push((
            "OUTPUTS · CAPTURES WHAT THEY PLAY",
            devices("System default output", &status.outputs, true),
        ));
        let mut applications: Vec<Row> = status
            .applications
            .iter()
            .map(|application| Row {
                source: Source::Application(application.executable.clone()),
                title: &application.executable,
                detail: if application.playing { "playing" } else { "" },
            })
            .collect();
        if let Source::Application(wanted) = selected {
            let listed = status
                .applications
                .iter()
                .any(|application| matches(&application.executable, wanted));
            if !listed {
                applications.push(Row {
                    source: selected.clone(),
                    title: wanted,
                    detail: "not running",
                });
            }
        }
        sections.push(("APPLICATIONS · CAPTURES WHAT THEY PLAY", applications));
    }
    sections
}

/// Whether `source` names the same source as `other`; application names may differ in
/// case or omit `.exe`.
fn same(source: &Source, other: &Source) -> bool {
    match (source, other) {
        (Source::Application(name), Source::Application(other)) => {
            matches(name, other) || matches(other, name)
        }
        _ => source == other,
    }
}

fn section_heading(ui: &mut Ui, heading: &str) {
    let size = vec2(ui.available_width(), SECTION_HEIGHT);
    let (row, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().text(
        pos2(row.left() + 2.0, row.center().y + 2.0),
        Align2::LEFT_CENTER,
        heading,
        font(10.0),
        TEXT_FAINT,
    );
}

/// Draws a note in place of a section's rows.
fn empty_row(ui: &mut Ui, text: &str) {
    let size = vec2(ui.available_width(), ROW_HEIGHT - 2.0);
    let (row, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().text(
        pos2(row.left() + 10.0, row.center().y),
        Align2::LEFT_CENTER,
        text,
        font(10.0),
        TEXT_FAINT,
    );
}

/// Draws one selectable source and returns whether it was newly chosen.
fn source_row(ui: &mut Ui, row: &Row, selected: &Source, status: &CaptureStatus) -> bool {
    let size = vec2(ui.available_width(), ROW_HEIGHT - 2.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let is_selected = same(&row.source, selected);
    // A default entry is in use when it is selected and its device opened.
    let active = status
        .active
        .as_ref()
        .is_some_and(|active| match (&row.source, active) {
            (Source::Input(None), Source::Input(_))
            | (Source::Loopback(None), Source::Loopback(_)) => is_selected,
            _ => same(&row.source, active),
        });
    let detail = if active { "in use" } else { row.detail };
    let kind = match row.source {
        Source::Input(_) => "input",
        Source::Loopback(_) => "output",
        Source::Application(_) => "application",
    };
    let key = format!("source-{kind}-{}", row.title);
    let response = list_row(
        ui,
        rect,
        &key,
        is_selected,
        (row.title, detail),
        Some(if active { SEND } else { TEXT_FAINT }),
        SEND,
    );
    response.clicked() && !is_selected
}
