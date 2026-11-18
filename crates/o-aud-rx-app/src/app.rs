//! The application window: the shared O-AUD-RX editor body above an output device panel.

use std::iter::once;
use std::sync::Arc;

use aud_ui::theme::{self, ALERT, BACKGROUND, RECEIVE, TEXT, TEXT_DIM, TEXT_FAINT, font, mono};
use aud_ui::widgets::{button, list_row, panel, readout};
use egui::{
    Align2, CentralPanel, Context, Frame, Id, Rect, ScrollArea, Sense, Ui, UiBuilder, pos2, vec2,
};
use o_aud_rx::link::RxLink;
use o_aud_rx::telemetry::RxTelemetry;
use o_aud_rx::view::{self, RxControls, RxViewState, VIEW_HEIGHT, VIEW_WIDTH};

use crate::output::{Output, OutputStatus};
use crate::settings::AppSettings;

/// Height of the device panel in logical pixels.
const DEVICE_HEIGHT: f32 = 196.0;
/// Logical window size in pixels.
pub const WINDOW_WIDTH: f32 = VIEW_WIDTH;
pub const WINDOW_HEIGHT: f32 = VIEW_HEIGHT + DEVICE_HEIGHT;

const MARGIN: f32 = 16.0;
const ROW_HEIGHT: f32 = 26.0;
const INFO_WIDTH: f32 = 220.0;

pub struct App {
    settings: Arc<AppSettings>,
    link: Arc<RxLink>,
    telemetry: Arc<RxTelemetry>,
    output: Output,
    view: RxViewState,
}

impl App {
    pub fn new(
        settings: Arc<AppSettings>,
        link: Arc<RxLink>,
        telemetry: Arc<RxTelemetry>,
        output: Output,
    ) -> Self {
        Self {
            settings,
            link,
            telemetry,
            output,
            view: RxViewState::default(),
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
                let receiver = Rect::from_min_size(full.min, vec2(full.width(), VIEW_HEIGHT));
                let controls = RxControls {
                    latency: &self.settings.latency_ms,
                    gain: &self.settings.gain_db,
                    mute: &self.settings.mute,
                };
                view::draw(
                    ui,
                    receiver,
                    &mut self.view,
                    &controls,
                    &self.link,
                    &self.telemetry,
                );
                let devices = Rect::from_min_max(
                    pos2(full.left() + MARGIN, receiver.bottom() - 4.0),
                    pos2(full.right() - MARGIN, full.bottom() - MARGIN),
                );
                self.draw_devices(ui, devices);
            });
        context.request_repaint();
    }

    fn draw_devices(&self, ui: &mut Ui, rect: Rect) {
        panel(ui, rect, "OUTPUT DEVICE", RECEIVE);
        let status = self.output.status();
        let list = Rect::from_min_max(
            rect.min + vec2(12.0, 32.0),
            pos2(rect.right() - INFO_WIDTH - 12.0, rect.bottom() - 10.0),
        );
        let selected = self.settings.device();
        let mut choice = None;
        ui.allocate_new_ui(UiBuilder::new().max_rect(list), |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for device in once(None).chain(status.devices.iter().map(Some)) {
                    let size = vec2(ui.available_width(), ROW_HEIGHT - 2.0);
                    let (row, _) = ui.allocate_exact_size(size, Sense::hover());
                    let title = device.map_or("System default", String::as_str);
                    let is_selected = device == selected.as_ref();
                    let active = match device {
                        Some(name) => status.device.as_ref() == Some(name),
                        None => selected.is_none() && status.device.is_some(),
                    };
                    let detail = if active { "in use" } else { "" };
                    let key = format!("device-{title}");
                    let response = list_row(
                        ui,
                        row,
                        &key,
                        is_selected,
                        (title, detail),
                        Some(if active { RECEIVE } else { TEXT_FAINT }),
                        RECEIVE,
                    );
                    if response.clicked() && !is_selected {
                        choice = Some(device.cloned());
                    }
                }
            });
        });
        if let Some(device) = choice {
            self.settings.set_device(device);
            self.output.reopen();
        }
        self.draw_device_info(ui, rect, &status);
    }

    fn draw_device_info(&self, ui: &mut Ui, rect: Rect, status: &OutputStatus) {
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
        let block = self.telemetry.block_frames();
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
        let (note, color) = match &status.error {
            Some(error) => (error.as_str(), ALERT),
            None => ("Channels beyond the bus stay silent", TEXT_DIM),
        };
        ui.painter().text(
            pos2(left, top + 88.0),
            Align2::LEFT_CENTER,
            note,
            font(10.0),
            color,
        );
        let refresh = Rect::from_min_size(pos2(left, rect.bottom() - 34.0), vec2(96.0, 22.0));
        if button(
            ui,
            Id::new("refresh-devices"),
            refresh,
            "REFRESH",
            RECEIVE,
            false,
        )
        .clicked()
        {
            self.output.refresh_devices();
        }
        ui.painter().text(
            pos2(refresh.right() + 10.0, refresh.center().y),
            Align2::LEFT_CENTER,
            format!("{} devices", status.devices.len()),
            mono(10.0),
            TEXT_FAINT,
        );
    }
}
