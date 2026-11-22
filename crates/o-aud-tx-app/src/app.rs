//! The application window: the shared O-AUD-TX editor body above an audio source panel.

use std::iter::once;
use std::sync::Arc;

use aud_ui::theme::{self, ALERT, BACKGROUND, SEND, TEXT, TEXT_DIM, TEXT_FAINT, font, mono};
use aud_ui::widgets::{button, list_row, panel, readout};
use egui::{
    Align2, CentralPanel, Context, Frame, Id, Rect, ScrollArea, Sense, Ui, UiBuilder, pos2, vec2,
};
use o_aud_tx::link::TxLink;
use o_aud_tx::telemetry::TxTelemetry;
use o_aud_tx::view::{self, TxControls, TxViewState, VIEW_HEIGHT, VIEW_WIDTH};

use crate::capture::{Capture, CaptureStatus};
use crate::settings::{AppSettings, Source};

/// Height of the source panel in logical pixels.
const SOURCE_HEIGHT: f32 = 236.0;
/// Logical window size in pixels.
pub const WINDOW_WIDTH: f32 = VIEW_WIDTH;
pub const WINDOW_HEIGHT: f32 = VIEW_HEIGHT + SOURCE_HEIGHT;

const MARGIN: f32 = 16.0;
const ROW_HEIGHT: f32 = 26.0;
const SECTION_HEIGHT: f32 = 20.0;
const INFO_WIDTH: f32 = 220.0;

pub struct App {
    settings: Arc<AppSettings>,
    link: Arc<TxLink>,
    telemetry: Arc<TxTelemetry>,
    capture: Capture,
    view: TxViewState,
}

impl App {
    pub fn new(
        settings: Arc<AppSettings>,
        link: Arc<TxLink>,
        telemetry: Arc<TxTelemetry>,
        capture: Capture,
    ) -> Self {
        Self {
            settings,
            link,
            telemetry,
            capture,
            view: TxViewState::default(),
        }
    }

    pub fn setup(context: &Context) {
        theme::apply(context);
    }

    pub fn update(&mut self, context: &Context) {
        CentralPanel::default()
            .frame(Frame::NONE.fill(BACKGROUND))
            .show(context, |ui| {
                let full = ui.max_rect();
                let transmitter = Rect::from_min_size(full.min, vec2(full.width(), VIEW_HEIGHT));
                let controls = TxControls {
                    gain: &self.settings.gain_db,
                    send: &self.settings.send,
                    thru: None,
                };
                view::draw(
                    ui,
                    transmitter,
                    &mut self.view,
                    &controls,
                    &self.link,
                    &self.telemetry,
                );
                let sources = Rect::from_min_max(
                    pos2(full.left() + MARGIN, transmitter.bottom() - 4.0),
                    pos2(full.right() - MARGIN, full.bottom() - MARGIN),
                );
                self.draw_sources(ui, sources);
            });
        context.request_repaint();
    }

    fn draw_sources(&self, ui: &mut Ui, rect: Rect) {
        panel(ui, rect, "AUDIO SOURCE", SEND);
        let status = self.capture.status();
        let list = Rect::from_min_max(
            rect.min + vec2(12.0, 32.0),
            pos2(rect.right() - INFO_WIDTH - 12.0, rect.bottom() - 10.0),
        );
        let selected = self.settings.source();
        let mut choice = None;
        ui.allocate_new_ui(UiBuilder::new().max_rect(list), |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let sections = [
                    (false, "INPUTS", "System default input", &status.inputs),
                    (
                        true,
                        "OUTPUTS · CAPTURES WHAT THEY PLAY",
                        "System default output",
                        &status.outputs,
                    ),
                ];
                for (loopback, heading, default, devices) in sections {
                    section_heading(ui, heading);
                    for device in once(None).chain(devices.iter().map(Some)) {
                        let source = Source {
                            loopback,
                            device: device.cloned(),
                        };
                        let title = device.map_or(default, String::as_str);
                        if source_row(ui, &source, title, &selected, &status) {
                            choice = Some(source);
                        }
                    }
                }
            });
        });
        if let Some(source) = choice {
            self.settings.set_source(source);
            self.capture.reopen();
        }
        self.draw_source_info(ui, rect, &status);
    }

    fn draw_source_info(&self, ui: &mut Ui, rect: Rect, status: &CaptureStatus) {
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
        let block = self.capture.block_frames();
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
        let loopback = status.active.as_ref().is_some_and(|source| source.loopback);
        let notes: [(&str, _); 2] = match &status.error {
            Some(error) => [(error.as_str(), ALERT), ("", TEXT_DIM)],
            None if loopback && status.kept_alive => [
                ("Sends everything this device plays", TEXT_DIM),
                ("Play the bus elsewhere: feedback", TEXT_DIM),
            ],
            None if loopback => [
                ("Sends everything this device plays", TEXT_DIM),
                ("Pauses while the device is silent", ALERT),
            ],
            None => [("Sends the device's input", TEXT_DIM), ("", TEXT_DIM)],
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
            self.capture.refresh_devices();
        }
        ui.painter().text(
            pos2(refresh.right() + 10.0, refresh.center().y),
            Align2::LEFT_CENTER,
            format!("{} devices", status.inputs.len() + status.outputs.len()),
            mono(10.0),
            TEXT_FAINT,
        );
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

/// Draws one selectable source and returns whether it was newly chosen.
fn source_row(
    ui: &mut Ui,
    source: &Source,
    title: &str,
    selected: &Source,
    status: &CaptureStatus,
) -> bool {
    let size = vec2(ui.available_width(), ROW_HEIGHT - 2.0);
    let (row, _) = ui.allocate_exact_size(size, Sense::hover());
    let is_selected = source == selected;
    // A default entry is in use when it is selected and its device opened.
    let active = status.active.as_ref().is_some_and(|active| {
        active.loopback == source.loopback
            && match &source.device {
                Some(name) => active.device.as_ref() == Some(name),
                None => is_selected,
            }
    });
    let detail = if active { "in use" } else { "" };
    let kind = if source.loopback { "output" } else { "input" };
    let key = format!("source-{kind}-{title}");
    let response = list_row(
        ui,
        row,
        &key,
        is_selected,
        (title, detail),
        Some(if active { SEND } else { TEXT_FAINT }),
        SEND,
    );
    response.clicked() && !is_selected
}
