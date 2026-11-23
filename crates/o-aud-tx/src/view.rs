//! The O-AUD-TX editor body, shared by the plugin and the standalone application.

use aud_ui::control::{Control, Switch};
use aud_ui::name_window::{NameWindow, name_editor};
use aud_ui::theme::{ALERT, BUS, RECEIVE, SEND, TEXT, TEXT_DIM, TEXT_FAINT, font, mono};
use aud_ui::widgets::{
    GuideLine, History, METER_FLOOR_DB, MeterState, button, channel_meters, gain_to_db,
    history_graph, knob, list_row, panel, pill, power_switch, title, toggle_button,
};
use egui::{Align2, Id, Rect, Ui, pos2, vec2};

use crate::link::{TxLink, TxState, TxStatus};
use crate::telemetry::{METER_CHANNELS, TxTelemetry};

/// Logical editor size in pixels.
pub const VIEW_WIDTH: f32 = 680.0;
pub const VIEW_HEIGHT: f32 = 400.0;

const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_HEIGHT: f32 = 52.0;
const BUS_WIDTH: f32 = 252.0;
const METER_WIDTH: f32 = 104.0;
const KNOB_SIZE: (f32, f32) = (66.0, 94.0);
const ROW_HEIGHT: f32 = 26.0;
/// History length in editor frames (about six seconds at 60 frames per second).
const HISTORY_LEN: usize = 360;

/// The transmitter's controls, bound to plugin parameters or application settings.
pub struct TxControls<'a> {
    pub gain: &'a dyn Control,
    pub send: &'a dyn Switch,
    /// Keeps the track's own output; `None` where there is no track (the application).
    pub thru: Option<&'a dyn Switch>,
}

/// Editor state kept between frames.
pub struct TxViewState {
    name_draft: String,
    name_window: NameWindow,
    meters: [MeterState; METER_CHANNELS],
    level: History,
}

impl Default for TxViewState {
    fn default() -> Self {
        Self {
            name_draft: String::new(),
            name_window: NameWindow::default(),
            meters: [MeterState::default(); METER_CHANNELS],
            level: History::new(HISTORY_LEN, METER_FLOOR_DB),
        }
    }
}

impl TxViewState {
    /// Feeds the peaks since the previous frame, `elapsed_s` seconds later, to the meters
    /// and the level history. [`draw`] does this; call it for a state that is not drawn,
    /// so its meters keep up.
    pub fn update_meters(&mut self, telemetry: &TxTelemetry, elapsed_s: f32) {
        let mut loudest = 0.0_f32;
        for (meter, peak) in self.meters.iter_mut().zip(&telemetry.peaks) {
            let peak = peak.take();
            loudest = loudest.max(peak);
            meter.update(peak, elapsed_s);
        }
        self.level.push(gain_to_db(loudest));
    }

    /// The loudest channel's meter level in dBFS.
    pub fn level_db(&self) -> f32 {
        self.meters
            .iter()
            .map(MeterState::level_db)
            .fold(METER_FLOOR_DB, f32::max)
    }
}

/// Draws the editor into `rect`.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    state: &mut TxViewState,
    controls: &TxControls<'_>,
    link: &TxLink,
    telemetry: &TxTelemetry,
) {
    let elapsed_s = ui.input(|input| input.stable_dt).min(0.1);
    state.update_meters(telemetry, elapsed_s);
    let status = link.status();

    let inner = rect.shrink2(vec2(MARGIN, 0.0));
    draw_header(
        ui,
        Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_HEIGHT)),
        &status,
        controls,
    );
    let body = Rect::from_min_max(
        pos2(inner.left(), inner.top() + HEADER_HEIGHT),
        pos2(inner.right(), rect.bottom() - MARGIN),
    );
    let bus = Rect::from_min_size(body.min, vec2(BUS_WIDTH, body.height()));
    let meters = Rect::from_min_max(pos2(body.right() - METER_WIDTH, body.top()), body.max);
    let send = Rect::from_min_max(
        pos2(bus.right() + GAP, body.top()),
        pos2(meters.left() - GAP, body.bottom()),
    );
    draw_bus(ui, bus, state, link, &status);
    draw_send(ui, send, state, controls);
    panel(ui, meters, "LEVEL", SEND);
    channel_meters(
        ui,
        Rect::from_min_max(meters.min + vec2(8.0, 34.0), meters.max - vec2(12.0, 12.0)),
        &state.meters,
        &["L", "R"],
        SEND,
    );
}

fn draw_header(ui: &mut Ui, rect: Rect, status: &TxStatus, controls: &TxControls<'_>) {
    title(ui, rect, "O-AUD-TX", "Audio Bus Transmitter", SEND);
    let centre_y = rect.center().y;
    let (text, color) = match status.state {
        TxState::Live if controls.send.is_on() => ("ON AIR", SEND),
        TxState::Live => ("SEND OFF", TEXT_DIM),
        TxState::Inactive => ("INACTIVE", TEXT_FAINT),
        TxState::Taken { .. } => ("NAME TAKEN", ALERT),
        TxState::InvalidName(_) | TxState::Error(_) => ("ERROR", ALERT),
    };
    let lit = !matches!(status.state, TxState::Inactive);
    let state = Rect::from_min_size(pos2(rect.right() - 96.0, centre_y - 9.0), vec2(96.0, 18.0));
    pill(ui, state, text, color, lit);
    let listeners = match status.listeners {
        0 => String::from("no receivers"),
        1 => String::from("1 receiver"),
        count => format!("{count} receivers"),
    };
    ui.painter().text(
        pos2(state.left() - 12.0, centre_y),
        Align2::RIGHT_CENTER,
        listeners,
        mono(11.0),
        if status.listeners > 0 {
            RECEIVE
        } else {
            TEXT_FAINT
        },
    );
}

fn draw_bus(ui: &mut Ui, rect: Rect, state: &mut TxViewState, link: &TxLink, status: &TxStatus) {
    panel(ui, rect, "BUS NAME", BUS);
    let field = Rect::from_min_size(rect.min + vec2(12.0, 34.0), vec2(rect.width() - 24.0, 28.0));
    let current = link.name();
    if let Some(name) = name_editor(
        ui,
        field,
        (&mut state.name_draft, &current),
        &mut state.name_window,
        ("tx-bus-name", "O-AUD-TX: bus name"),
        BUS,
    ) {
        link.set_name(&name);
    }

    let (message, color) = match &status.state {
        TxState::Inactive => (String::from("Starts when the audio starts"), TEXT_DIM),
        TxState::InvalidName(error) => (error.to_string(), ALERT),
        TxState::Taken { .. } => (String::from("Another O-AUD-TX uses this name"), ALERT),
        TxState::Live => (String::from("Receivers can select this bus"), SEND),
        TxState::Error(error) => (error.to_string(), ALERT),
    };
    ui.painter().text(
        pos2(field.left() + 2.0, field.bottom() + 12.0),
        Align2::LEFT_CENTER,
        message,
        font(10.0),
        color,
    );
    let mut list_top = field.bottom() + 28.0;
    if let TxState::Taken { suggestion } = &status.state {
        let suggest = Rect::from_min_size(
            pos2(field.left(), field.bottom() + 24.0),
            vec2(field.width(), 22.0),
        );
        let label = format!("Use “{suggestion}”");
        if button(ui, Id::new("tx-suggest"), suggest, &label, BUS, false).clicked() {
            link.set_name(suggestion);
            state.name_draft.clone_from(suggestion);
        }
        list_top = suggest.bottom() + 16.0;
    }

    ui.painter().text(
        pos2(field.left() + 2.0, list_top),
        Align2::LEFT_CENTER,
        "BUSES IN USE",
        font(10.0),
        TEXT_FAINT,
    );
    let own = link.name();
    let rows = ((rect.bottom() - list_top - 14.0) / ROW_HEIGHT).max(0.0) as usize;
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
            String::from("starting")
        };
        let mine =
            bus.name.to_lowercase() == own.trim().to_lowercase() && status.state == TxState::Live;
        list_row(
            ui,
            row,
            &bus.name,
            mine,
            (&bus.name, &detail),
            Some(if mine { SEND } else { TEXT_FAINT }),
            SEND,
        )
        .on_hover_text(if mine {
            "This transmitter"
        } else {
            "Used by another transmitter"
        });
    }
}

fn draw_send(ui: &mut Ui, rect: Rect, state: &TxViewState, controls: &TxControls<'_>) {
    panel(ui, rect, "SEND", SEND);
    power_switch(
        ui,
        controls.send,
        rect.right_top() + vec2(-20.0, 17.0),
        SEND,
    )
    .on_hover_text("Send to the bus");
    let knob_rect = Rect::from_min_size(
        pos2(rect.left() + 14.0, rect.top() + 32.0),
        vec2(KNOB_SIZE.0, KNOB_SIZE.1),
    );
    knob(ui, controls.gain, knob_rect, "Send Gain", SEND, false);

    let thru = Rect::from_min_size(
        pos2(knob_rect.right() + 18.0, rect.top() + 44.0),
        vec2(rect.right() - knob_rect.right() - 32.0, 22.0),
    );
    if let Some(thru_switch) = controls.thru {
        toggle_button(ui, thru_switch, thru, "THRU TO TRACK", SEND)
            .on_hover_text("Keep the track's own output; switch off to only send");
        let hint = if thru_switch.is_on() {
            "Track output unchanged"
        } else {
            "Track output muted, send only"
        };
        ui.painter().text(
            pos2(thru.left(), thru.bottom() + 14.0),
            Align2::LEFT_CENTER,
            hint,
            font(10.0),
            TEXT_DIM,
        );
    }
    let send_hint = if controls.send.is_on() {
        "Sending to the bus"
    } else {
        "Bus carries silence"
    };
    ui.painter().text(
        pos2(thru.left(), thru.bottom() + 30.0),
        Align2::LEFT_CENTER,
        send_hint,
        font(10.0),
        if controls.send.is_on() {
            TEXT
        } else {
            TEXT_DIM
        },
    );

    let graph = Rect::from_min_max(
        pos2(rect.left() + 14.0, knob_rect.bottom() + 12.0),
        pos2(rect.right() - 14.0, rect.bottom() - 14.0),
    );
    let guides = [
        GuideLine {
            value: 0.0,
            label: "0 dB",
            color: TEXT_FAINT,
        },
        GuideLine {
            value: -18.0,
            label: "−18 dB",
            color: TEXT_FAINT,
        },
    ];
    history_graph(
        ui,
        graph,
        &state.level,
        (METER_FLOOR_DB, 6.0),
        SEND,
        &guides,
    );
}
