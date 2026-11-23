//! The plugin editor: a spectrum with the noise estimate above the profile, reduction,
//! analysis and level panels.

mod spectrum;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aud_ui::param::{ParamControl, ParamSwitch, enum_selector};
use aud_ui::theme::{
    self, ALERT, BACKGROUND, BUS, LEVEL, RECEIVE, TEXT, TEXT_DIM, TEXT_FAINT, font, mono,
};
use aud_ui::widgets::{bar_meter, button, knob, logo, panel, toggle_button, vertical_meter};
use nih_plug::prelude::{Editor, ParamSetter};
use nih_plug_egui::create_egui_editor;
use nih_plug_egui::egui::{Align2, CentralPanel, Color32, Frame, Id, Rect, Ui, pos2, vec2};
use triple_buffer::Output;

use crate::params::{EDITOR_WIDTH, ModeChoice, ONrParams};
use crate::telemetry::{NoiseSource, Telemetry};

/// Kept signal and output.
const KEPT: Color32 = RECEIVE;
/// Removed noise.
const REMOVED: Color32 = ALERT;
/// The noise profile and estimate.
const NOISE: Color32 = BUS;
/// Unprocessed input.
const INPUT: Color32 = LEVEL;

const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_HEIGHT: f32 = 48.0;
const SPECTRUM_HEIGHT: f32 = 300.0;
const KNOB_SIZE: (f32, f32) = (66.0, 94.0);

/// What the editor shares with the plugin besides its parameters.
struct Shared {
    params: Arc<ONrParams>,
    learn: Arc<AtomicBool>,
}

pub fn create(
    params: Arc<ONrParams>,
    learn: Arc<AtomicBool>,
    telemetry: Arc<Mutex<Output<Telemetry>>>,
) -> Option<Box<dyn Editor>> {
    let state = params.editor_state.clone();
    let shared = Shared { params, learn };
    create_egui_editor(
        state,
        (),
        |context, _| theme::apply(context),
        move |context, setter, _| {
            let data = telemetry
                .lock()
                .map(|mut output| output.read().clone())
                .unwrap_or_default();
            CentralPanel::default()
                .frame(Frame::NONE.fill(BACKGROUND))
                .show(context, |ui| draw(ui, &shared, setter, &data));
            context.request_repaint();
        },
    )
}

fn draw(ui: &mut Ui, shared: &Shared, setter: &ParamSetter, data: &Telemetry) {
    let params = &*shared.params;
    let width = EDITOR_WIDTH as f32 - 2.0 * MARGIN;
    let left = MARGIN;
    let mut top = 0.0;

    draw_header(
        ui,
        Rect::from_min_size(pos2(left, top), vec2(width, HEADER_HEIGHT)),
        params,
        setter,
        data,
    );
    top += HEADER_HEIGHT;

    spectrum::draw(
        ui,
        Rect::from_min_size(pos2(left, top), vec2(width, SPECTRUM_HEIGHT)),
        data,
    );
    top += SPECTRUM_HEIGHT + GAP;

    let height = ui.max_rect().bottom().max(top + 200.0) - top - MARGIN;
    let widths = [262.0, 340.0, 206.0];
    let mut x = left;
    let mut next = |panel_width: f32| {
        let rect = Rect::from_min_size(pos2(x, top), vec2(panel_width, height));
        x += panel_width + GAP;
        rect
    };
    let profile = next(widths[0]);
    let reduction = next(widths[1]);
    let analysis = next(widths[2]);
    let level = next(width - widths.iter().sum::<f32>() - 3.0 * GAP);

    draw_profile(ui, profile, shared, setter, data);
    draw_reduction(ui, reduction, params, setter, data);
    draw_analysis(ui, analysis, params, setter, data);
    draw_level(ui, level, data);
}

fn draw_header(
    ui: &mut Ui,
    rect: Rect,
    params: &ONrParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    let centre_y = rect.center().y;
    let left = rect.left() + logo(ui, rect.left(), centre_y);
    let painter = ui.painter();
    let title = painter.layout_no_wrap(String::from("O-NR"), font(24.0), TEXT);
    let title_width = title.size().x;
    painter.galley(pos2(left, centre_y - title.size().y * 0.5), title, TEXT);
    painter.text(
        pos2(left + title_width + 12.0, centre_y + 1.0),
        Align2::LEFT_CENTER,
        "Open Noise Reduction",
        font(12.0),
        TEXT_DIM,
    );

    let listen = Rect::from_min_size(
        pos2(rect.right() - 124.0, centre_y - 11.0),
        vec2(124.0, 22.0),
    );
    toggle_button(
        ui,
        &ParamSwitch::new(&params.listen_removed, setter),
        listen,
        "LISTEN REMOVED",
        REMOVED,
    )
    .on_hover_text("Solo what O-NR removes, to tune without damaging the signal");

    let info = if data.host_rate > 0.0 {
        format!(
            "{:.1} kHz · {:.1} ms",
            data.host_rate / 1_000.0,
            1_000.0 * data.latency_samples as f32 / data.host_rate
        )
    } else {
        String::from("not processing")
    };
    ui.painter().text(
        pos2(listen.left() - 12.0, centre_y),
        Align2::RIGHT_CENTER,
        info,
        mono(11.0),
        TEXT_FAINT,
    );
}

fn knob_rect(centre_x: f32, top: f32) -> Rect {
    Rect::from_min_size(
        pos2(centre_x - KNOB_SIZE.0 * 0.5, top),
        vec2(KNOB_SIZE.0, KNOB_SIZE.1),
    )
}

/// Centres of `count` evenly spaced columns inside `rect`.
fn columns(rect: Rect, count: usize) -> impl Iterator<Item = f32> {
    let inner = rect.shrink2(vec2(10.0, 0.0));
    let width = inner.width() / count as f32;
    (0..count).map(move |index| inner.left() + width * (index as f32 + 0.5))
}

fn draw_profile(ui: &mut Ui, rect: Rect, shared: &Shared, setter: &ParamSetter, data: &Telemetry) {
    let params = &*shared.params;
    panel(ui, rect, "NOISE", NOISE);
    let selector = Rect::from_min_max(
        rect.min + vec2(64.0, 7.0),
        rect.right_top() + vec2(-12.0, 27.0),
    );
    enum_selector(ui, setter, &params.mode, selector, NOISE);

    let inner = rect.shrink2(vec2(14.0, 0.0));
    let row = rect.top() + 44.0;
    let learning = shared.learn.load(Ordering::Relaxed);
    let learn_rect = Rect::from_min_size(pos2(inner.left(), row), vec2(150.0, 30.0));
    let label = if learning { "STOP LEARNING" } else { "LEARN" };
    let response = button(ui, Id::new("learn"), learn_rect, label, NOISE, learning).on_hover_text(
        "Play a passage of noise alone, then stop: its spectrum becomes the profile",
    );
    if response.clicked() {
        shared.learn.store(!learning, Ordering::Relaxed);
        // A learned profile is meant to be used.
        if learning && params.mode.value() != ModeChoice::Profile {
            setter.begin_set_parameter(&params.mode);
            setter.set_parameter(&params.mode, ModeChoice::Profile);
            setter.end_set_parameter(&params.mode);
        }
    }
    let clear_rect = Rect::from_min_max(
        pos2(learn_rect.right() + 8.0, row),
        pos2(inner.right(), row + 30.0),
    );
    let has_profile = params.profile.has_profile();
    if button(ui, Id::new("clear"), clear_rect, "CLEAR", NOISE, false)
        .on_hover_text("Remove the learned profile")
        .clicked()
        && has_profile
        && !learning
    {
        params.profile.clear();
    }

    let (status, detail, warning) = match data.source {
        NoiseSource::Learning => (
            format!("Learning: {:.1} s", data.learned_s),
            "Play noise only; the signal passes unchanged",
            false,
        ),
        NoiseSource::Profile => (
            String::from("Using the learned profile"),
            "LEARN again to replace it",
            false,
        ),
        NoiseSource::NoProfile => (
            String::from("No profile: adapting"),
            "Press LEARN during a noise-only passage",
            true,
        ),
        NoiseSource::Adaptive => (
            String::from("Tracking the noise floor"),
            "Follows steady noise under the signal",
            false,
        ),
    };
    let painter = ui.painter();
    painter.text(
        pos2(inner.left(), row + 52.0),
        Align2::LEFT_CENTER,
        status,
        font(13.0),
        if warning { REMOVED } else { NOISE },
    );
    painter.text(
        pos2(inner.left(), row + 72.0),
        Align2::LEFT_CENTER,
        detail,
        font(10.0),
        TEXT_FAINT,
    );
    painter.text(
        pos2(inner.left(), rect.bottom() - 18.0),
        Align2::LEFT_CENTER,
        if has_profile {
            "Profile stored with the session"
        } else {
            "No profile stored"
        },
        font(10.0),
        TEXT_FAINT,
    );
}

fn draw_reduction(
    ui: &mut Ui,
    rect: Rect,
    params: &ONrParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    panel(ui, rect, "REDUCTION", KEPT);
    let top = rect.top() + 36.0;
    let knobs = [
        (&params.reduction_db, "Reduction"),
        (&params.threshold_db, "Threshold"),
        (&params.release_ms, "Release"),
        (&params.smoothing, "Smoothing"),
    ];
    for ((param, label), x) in knobs.into_iter().zip(columns(rect, 4)) {
        knob(
            ui,
            &ParamControl::new(param, setter),
            knob_rect(x, top),
            label,
            KEPT,
            false,
        );
    }
    let inner = rect.shrink2(vec2(14.0, 0.0));
    let removed = (-data.reduction_db).max(0.0);
    bar_meter(
        ui,
        Rect::from_min_size(
            pos2(inner.left(), top + KNOB_SIZE.1 + 22.0),
            vec2(inner.width(), 20.0),
        ),
        removed / 40.0,
        None,
        REMOVED,
        "Removed",
        &format!("{removed:.1} dB"),
    );
}

fn draw_analysis(
    ui: &mut Ui,
    rect: Rect,
    params: &ONrParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    panel(ui, rect, "ANALYSIS", INPUT);
    let inner = rect.shrink2(vec2(14.0, 0.0));
    let painter = ui.painter();
    painter.text(
        pos2(inner.left(), rect.top() + 46.0),
        Align2::LEFT_CENTER,
        "Resolution",
        font(11.0),
        TEXT_DIM,
    );
    enum_selector(
        ui,
        setter,
        &params.resolution,
        Rect::from_min_size(
            pos2(inner.left(), rect.top() + 58.0),
            vec2(inner.width(), 22.0),
        ),
        INPUT,
    );
    let info = if data.fft_size > 0 && data.host_rate > 0.0 {
        format!(
            "FFT {} · {:.1} Hz bins",
            data.fft_size,
            data.host_rate / data.fft_size as f32
        )
    } else {
        String::new()
    };
    ui.painter().text(
        pos2(inner.left(), rect.top() + 94.0),
        Align2::LEFT_CENTER,
        info,
        mono(10.0),
        TEXT_FAINT,
    );
    toggle_button(
        ui,
        &ParamSwitch::new(&params.link, setter),
        Rect::from_min_size(pos2(inner.left(), rect.top() + 116.0), vec2(96.0, 22.0)),
        "STEREO LINK",
        INPUT,
    )
    .on_hover_text("One gain for both channels, which keeps the stereo image steady");
    ui.painter().text(
        pos2(inner.left(), rect.bottom() - 18.0),
        Align2::LEFT_CENTER,
        "Higher resolution: finer tones, more delay",
        font(10.0),
        TEXT_FAINT,
    );
}

fn draw_level(ui: &mut Ui, rect: Rect, data: &Telemetry) {
    panel(ui, rect, "LEVEL", INPUT);
    let meter_top = rect.top() + 40.0;
    let meter_bottom = rect.bottom() - 12.0;
    let centre = rect.center().x;
    let input = Rect::from_min_max(
        pos2(centre - 20.0, meter_top),
        pos2(centre - 6.0, meter_bottom),
    );
    let output = Rect::from_min_max(
        pos2(centre + 6.0, meter_top),
        pos2(centre + 20.0, meter_bottom),
    );
    vertical_meter(ui, input, data.input_peak_db, INPUT, "IN");
    vertical_meter(ui, output, data.output_peak_db, KEPT, "OUT");
}
