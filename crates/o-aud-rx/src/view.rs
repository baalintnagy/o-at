//! The O-AUD-RX editor body, shared by the plugin and the standalone application.

use aud_ui::control::{Control, Switch};
use aud_ui::name_window::{NameWindow, name_editor};
use aud_ui::theme::{ALERT, BUS, LEVEL, RECEIVE, SYNC, TEXT_DIM, TEXT_FAINT, font, mono};
use aud_ui::widgets::{
    GuideLine, History, MeterState, bar_meter, button, channel_meters, history_graph, knob,
    list_row, panel, pill, readout, title,
};
use egui::{Align2, Id, Rect, Ui, pos2, vec2};

use crate::link::{LinkState, LinkStatus, RxLink};
use crate::settings::LATENCY_MAX_MS;
use crate::telemetry::{METER_CHANNELS, PlayState, RxTelemetry};

/// Logical size of the editor body in pixels.
pub const VIEW_WIDTH: f32 = 760.0;
pub const VIEW_HEIGHT: f32 = 440.0;

const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_HEIGHT: f32 = 52.0;
const BUS_WIDTH: f32 = 236.0;
const OUTPUT_WIDTH: f32 = 168.0;
const KNOB_SIZE: (f32, f32) = (66.0, 94.0);
const ROW_HEIGHT: f32 = 26.0;
/// History length in editor frames (about six seconds at 60 frames per second).
const HISTORY_LEN: usize = 360;

/// The receiver's controls, bound to plugin parameters or application settings.
pub struct RxControls<'a> {
    pub latency: &'a dyn Control,
    pub gain: &'a dyn Control,
    pub mute: &'a dyn Switch,
}

/// Editor state kept between frames.
pub struct RxViewState {
    name_draft: String,
    name_window: NameWindow,
    meters: [MeterState; METER_CHANNELS],
    fill: History,
}

impl Default for RxViewState {
    fn default() -> Self {
        Self {
            name_draft: String::new(),
            name_window: NameWindow::default(),
            meters: [MeterState::default(); METER_CHANNELS],
            fill: History::new(HISTORY_LEN, 0.0),
        }
    }
}

/// Draws the editor body into `rect`.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    state: &mut RxViewState,
    controls: &RxControls<'_>,
    link: &RxLink,
    telemetry: &RxTelemetry,
) {
    let elapsed_s = ui.input(|input| input.stable_dt).min(0.1);
    for (meter, peak) in state.meters.iter_mut().zip(&telemetry.peaks) {
        meter.update(peak.take(), elapsed_s);
    }
    let playing = telemetry.state() == PlayState::Playing;
    state.fill.push(if playing {
        telemetry.fill_ms.load()
    } else {
        0.0
    });
    let status = link.status();

    let inner = rect.shrink2(vec2(MARGIN, 0.0));
    draw_header(
        ui,
        Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT)),
        &status,
        telemetry,
    );
    let body = Rect::from_min_max(
        pos2(inner.left(), inner.top() + HEADER_HEIGHT),
        pos2(inner.right(), rect.bottom() - MARGIN),
    );
    let bus = Rect::from_min_size(body.min, vec2(BUS_WIDTH, body.height()));
    let output = Rect::from_min_max(pos2(body.right() - OUTPUT_WIDTH, body.top()), body.max);
    let sync = Rect::from_min_max(
        pos2(bus.right() + GAP, body.top()),
        pos2(output.left() - GAP, body.bottom()),
    );
    draw_bus(ui, bus, state, link, &status);
    draw_sync(ui, sync, state, controls, telemetry);
    draw_output(ui, output, state, controls);
}

fn draw_header(ui: &mut Ui, rect: Rect, status: &LinkStatus, telemetry: &RxTelemetry) {
    title(ui, rect, "O-AUD-RX", "Audio Bus Receiver", RECEIVE);
    let centre_y = rect.center().y;
    let play_state = telemetry.state();
    let (text, color, lit) = match (&status.state, play_state) {
        (LinkState::Connected, PlayState::Playing) => ("LOCKED", RECEIVE, true),
        (LinkState::Connected, _) => ("BUFFERING", SYNC, true),
        (LinkState::Waiting, _) => ("WAITING", BUS, true),
        (LinkState::InvalidName(_) | LinkState::Error(_), _) => ("ERROR", ALERT, true),
        (LinkState::NoBus, _) => ("NO BUS", BUS, false),
    };
    let state = Rect::from_min_size(pos2(rect.right() - 92.0, centre_y - 9.0), vec2(92.0, 18.0));
    pill(ui, state, text, color, lit);

    let source_rate = telemetry.source_rate() as f32;
    let device_rate = telemetry.device_rate() as f32;
    let info = if status.state == LinkState::Connected && source_rate > 0.0 {
        format!(
            "{:.1} → {:.1} kHz · {} ch",
            source_rate / 1_000.0,
            device_rate / 1_000.0,
            telemetry.source_channels()
        )
    } else if device_rate > 0.0 {
        format!("{:.1} kHz output", device_rate / 1_000.0)
    } else {
        String::from("not processing")
    };
    ui.painter().text(
        pos2(state.left() - 12.0, centre_y),
        Align2::RIGHT_CENTER,
        info,
        mono(11.0),
        TEXT_FAINT,
    );
}

fn draw_bus(ui: &mut Ui, rect: Rect, state: &mut RxViewState, link: &RxLink, status: &LinkStatus) {
    panel(ui, rect, "BUS", BUS);
    let field = Rect::from_min_size(rect.min + vec2(12.0, 34.0), vec2(rect.width() - 24.0, 28.0));
    let current = link.name();
    if let Some(name) = name_editor(
        ui,
        field,
        (&mut state.name_draft, &current),
        &mut state.name_window,
        ("rx-bus-name", "O-AUD-RX: bus name"),
        BUS,
    ) {
        link.set_name(&name);
    }

    let message = match &status.state {
        LinkState::NoBus => String::from("Pick a bus or type its name"),
        LinkState::InvalidName(error) => error.to_string(),
        LinkState::Waiting => String::from("Waiting for its transmitter"),
        LinkState::Connected => String::from("Receiving"),
        LinkState::Error(error) => error.to_string(),
    };
    let message_color = match status.state {
        LinkState::InvalidName(_) | LinkState::Error(_) => ALERT,
        LinkState::Connected => RECEIVE,
        _ => TEXT_DIM,
    };
    ui.painter().text(
        pos2(field.left() + 2.0, field.bottom() + 12.0),
        Align2::LEFT_CENTER,
        message,
        font(10.0),
        message_color,
    );

    let list_top = field.bottom() + 28.0;
    ui.painter().text(
        pos2(field.left() + 2.0, list_top),
        Align2::LEFT_CENTER,
        "AVAILABLE BUSES",
        font(10.0),
        TEXT_FAINT,
    );
    let selected = link.name();
    let rows = ((rect.bottom() - list_top - 14.0) / ROW_HEIGHT).max(0.0) as usize;
    if status.buses.is_empty() {
        ui.painter().text(
            pos2(field.left() + 2.0, list_top + 22.0),
            Align2::LEFT_CENTER,
            "No transmitters running",
            font(11.0),
            TEXT_DIM,
        );
    }
    for (index, bus) in status.buses.iter().take(rows).enumerate() {
        let row = Rect::from_min_size(
            pos2(field.left(), list_top + 10.0 + index as f32 * ROW_HEIGHT),
            vec2(field.width(), ROW_HEIGHT - 2.0),
        );
        let detail = if bus.sample_rate > 0 {
            format!(
                "{:.1}k · {}ch",
                bus.sample_rate as f32 / 1_000.0,
                bus.channels
            )
        } else {
            String::from("idle")
        };
        let is_selected = bus.name.to_lowercase() == selected.trim().to_lowercase();
        let streaming = bus.sample_rate > 0;
        let response = list_row(
            ui,
            row,
            &bus.name,
            is_selected,
            (&bus.name, &detail),
            Some(if streaming { RECEIVE } else { TEXT_FAINT }),
            BUS,
        );
        if response.clicked() {
            link.set_name(&bus.name);
            state.name_draft.clone_from(&bus.name);
        }
    }
}

fn draw_sync(
    ui: &mut Ui,
    rect: Rect,
    state: &RxViewState,
    controls: &RxControls<'_>,
    telemetry: &RxTelemetry,
) {
    panel(ui, rect, "SYNC", SYNC);
    let playing = telemetry.state() == PlayState::Playing;
    let fill = telemetry.fill_ms.load();
    let target = telemetry.target_ms.load();

    let readouts_left = rect.left() + 14.0;
    let column = (rect.width() - 28.0) / 3.0;
    let dash = String::from("–");
    let values = [
        (
            "Buffered",
            if playing {
                format!("{fill:.1} ms")
            } else {
                dash.clone()
            },
            RECEIVE,
        ),
        (
            "Target",
            if target > 0.0 {
                format!("{target:.1} ms")
            } else {
                dash.clone()
            },
            SYNC,
        ),
        (
            "Jitter",
            if playing {
                format!("{:.1} ms", telemetry.jitter_ms.load())
            } else {
                dash.clone()
            },
            LEVEL,
        ),
        (
            "Clock drift",
            if playing {
                format!("{:+.0} ppm", telemetry.drift_ppm.load())
            } else {
                dash.clone()
            },
            SYNC,
        ),
        (
            "Dropouts",
            telemetry.underruns().to_string(),
            if telemetry.underruns() > 0 {
                ALERT
            } else {
                LEVEL
            },
        ),
        (
            "Skips",
            telemetry.overruns().to_string(),
            if telemetry.overruns() > 0 {
                ALERT
            } else {
                LEVEL
            },
        ),
    ];
    for (index, (caption, value, color)) in values.iter().enumerate() {
        let x = readouts_left + column * (index % 3) as f32;
        let y = rect.top() + 34.0 + 38.0 * (index / 3) as f32;
        readout(ui, pos2(x, y), caption, value, *color);
    }

    let knob_rect = Rect::from_min_size(
        pos2(readouts_left - 4.0, rect.top() + 112.0),
        vec2(KNOB_SIZE.0, KNOB_SIZE.1),
    );
    knob(ui, controls.latency, knob_rect, "Latency", SYNC, false)
        .on_hover_text("Requested buffer latency");

    let gauge_left = knob_rect.right() + 12.0;
    let scale_ms = (2.0 * target).clamp(10.0, 2.0 * LATENCY_MAX_MS);
    bar_meter(
        ui,
        Rect::from_min_size(
            pos2(gauge_left, knob_rect.top() + 14.0),
            vec2(rect.right() - 14.0 - gauge_left, 20.0),
        ),
        if playing { fill / scale_ms } else { 0.0 },
        (target > 0.0).then_some(target / scale_ms),
        RECEIVE,
        "Queue",
        &format!("0 – {scale_ms:.0} ms"),
    );
    let requested = telemetry.requested_ms.load();
    let (hint, hint_color) = if target > requested + 0.05 {
        ("Target raised for irregular blocks", SYNC)
    } else if telemetry.underruns() > 0 {
        ("Raise the latency if dropouts recur", TEXT_DIM)
    } else {
        ("Target follows the latency setting", TEXT_FAINT)
    };
    ui.painter().text(
        pos2(gauge_left, knob_rect.top() + 52.0),
        Align2::LEFT_CENTER,
        hint,
        font(10.0),
        hint_color,
    );

    let graph = Rect::from_min_max(
        pos2(rect.left() + 14.0, knob_rect.bottom() + 8.0),
        pos2(rect.right() - 14.0, rect.bottom() - 14.0),
    );
    let guide_label = format!("target {target:.0} ms");
    let guides = [GuideLine {
        value: target,
        label: &guide_label,
        color: SYNC,
    }];
    history_graph(
        ui,
        graph,
        &state.fill,
        (0.0, scale_ms),
        RECEIVE,
        if target > 0.0 { &guides } else { &[] },
    );
}

fn draw_output(ui: &mut Ui, rect: Rect, state: &RxViewState, controls: &RxControls<'_>) {
    panel(ui, rect, "OUTPUT", RECEIVE);
    let knob_rect = Rect::from_min_size(
        pos2(rect.left() + 10.0, rect.top() + 32.0),
        vec2(KNOB_SIZE.0, KNOB_SIZE.1),
    );
    knob(ui, controls.gain, knob_rect, "Gain", RECEIVE, false);
    let mute = Rect::from_center_size(
        pos2(knob_rect.center().x, knob_rect.bottom() + 14.0),
        vec2(56.0, 20.0),
    );
    let muted = controls.mute.is_on();
    if button(ui, Id::new("rx-mute"), mute, "MUTE", ALERT, muted).clicked() {
        controls.mute.set_on(!muted);
    }
    let meters = Rect::from_min_max(
        pos2(knob_rect.right() + 4.0, rect.top() + 34.0),
        pos2(rect.right() - 12.0, rect.bottom() - 12.0),
    );
    channel_meters(ui, meters, &state.meters, &["L", "R"], RECEIVE);
}
