//! The plugin editor: spectrum and activity displays above five module panels.

mod colors;
mod spectrum;
mod timeline;

use std::sync::{Arc, Mutex};

use aud_ui::param::{ParamControl, ParamSwitch, enum_selector};
use aud_ui::theme::{self, BACKGROUND, TEXT, TEXT_DIM, TEXT_FAINT, font, mono};
use aud_ui::widgets::{
    bar_meter, bipolar_meter, knob, logo, panel, pill, power_switch, toggle_button, vertical_meter,
};
use nih_plug::prelude::{Editor, Param, ParamSetter};
use nih_plug_egui::create_egui_editor;
use nih_plug_egui::egui::{Align2, CentralPanel, Frame, Rect, Ui, pos2, vec2};
use triple_buffer::Output;

use crate::dsp::denoise::ModelStatus;
use crate::dsp::level::FLOOR_DB;
use crate::editor::colors::{ECHO, GATE, INPUT, LEVELER, REMOVED, VOICE};
use crate::params::{EDITOR_WIDTH, GateMode, NoiseModelChoice, ONecParams};
use crate::telemetry::Telemetry;

const MARGIN: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADER_HEIGHT: f32 = 48.0;
const SPECTRUM_HEIGHT: f32 = 262.0;
const TIMELINE_HEIGHT: f32 = 92.0;
const KNOB_SIZE: (f32, f32) = (66.0, 94.0);

pub fn create(
    params: Arc<ONecParams>,
    telemetry: Arc<Mutex<Output<Telemetry>>>,
) -> Option<Box<dyn Editor>> {
    let state = params.editor_state.clone();
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
                .show(context, |ui| draw(ui, &params, setter, &data));
            context.request_repaint();
        },
    )
}

fn draw(ui: &mut Ui, params: &ONecParams, setter: &ParamSetter, data: &Telemetry) {
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

    let spectrum_rect = Rect::from_min_size(pos2(left, top), vec2(width, SPECTRUM_HEIGHT));
    spectrum::draw(ui, spectrum_rect, params, setter, data);
    top += SPECTRUM_HEIGHT + 8.0;

    let timeline_rect = Rect::from_min_size(pos2(left, top), vec2(width, TIMELINE_HEIGHT));
    timeline::draw(ui, timeline_rect, params, setter, data);
    top += TIMELINE_HEIGHT + GAP;

    let height = ui.max_rect().bottom().max(top + 200.0) - top - MARGIN;
    let widths = [226.0, 204.0, 284.0, 284.0];
    let mut x = left;
    let mut panel = |panel_width: f32| {
        let rect = Rect::from_min_size(pos2(x, top), vec2(panel_width, height));
        x += panel_width + GAP;
        rect
    };
    let noise = panel(widths[0]);
    let echo = panel(widths[1]);
    let gate = panel(widths[2]);
    let leveler = panel(widths[3]);
    let output_width = width - widths.iter().sum::<f32>() - 4.0 * GAP;
    let output = panel(output_width);

    draw_noise(ui, noise, params, setter, data);
    draw_echo(ui, echo, params, setter, data);
    draw_gate(ui, gate, params, setter, data);
    draw_leveler(ui, leveler, params, setter, data);
    draw_output(ui, output, params, setter, data);
}

fn draw_header(
    ui: &mut Ui,
    rect: Rect,
    params: &ONecParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    let centre_y = rect.center().y;
    let left = rect.left() + logo(ui, rect.left(), centre_y);
    let painter = ui.painter();
    let title = painter.layout_no_wrap(String::from("O-NEC"), font(24.0), TEXT);
    let title_width = title.size().x;
    painter.galley(pos2(left, centre_y - title.size().y * 0.5), title, TEXT);
    painter.text(
        pos2(left + title_width + 12.0, centre_y + 1.0),
        Align2::LEFT_CENTER,
        "Open Noise & Echo Cancelation",
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
    .on_hover_text("Solo what O-NEC removes, to tune without losing voice");

    let reference =
        Rect::from_min_size(pos2(listen.left() - 92.0, centre_y - 9.0), vec2(82.0, 18.0));
    let reference_text = if data.reference_active {
        "REF ACTIVE"
    } else {
        "NO REF"
    };
    pill(ui, reference, reference_text, ECHO, data.reference_active);

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
        pos2(reference.left() - 12.0, centre_y),
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

fn draw_noise(
    ui: &mut Ui,
    rect: Rect,
    params: &ONecParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    panel(ui, rect, "NOISE", VOICE);
    power_switch(
        ui,
        &ParamSwitch::new(&params.denoise_enabled, setter),
        rect.right_top() + vec2(-20.0, 17.0),
        VOICE,
    )
    .on_hover_text(params.denoise_enabled.name());
    let selector = Rect::from_min_max(
        rect.min + vec2(64.0, 7.0),
        rect.right_top() + vec2(-38.0, 27.0),
    );
    enum_selector(ui, setter, &params.noise_model, selector, VOICE);

    let top = rect.top() + 36.0;
    let mut centres = columns(rect, 2);
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.reduction_db, setter),
            knob_rect(x, top),
            "Reduction",
            VOICE,
            false,
        );
    }
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.high_pass_hz, setter),
            knob_rect(x, top),
            "High-pass",
            VOICE,
            false,
        );
        let button =
            Rect::from_center_size(pos2(x, top - 8.0 + KNOB_SIZE.1 + 14.0), vec2(44.0, 16.0));
        toggle_button(
            ui,
            &ParamSwitch::new(&params.high_pass_enabled, setter),
            button,
            "HPF",
            VOICE,
        );
    }

    let meter_top = top + KNOB_SIZE.1 + 22.0;
    let meters = rect.shrink2(vec2(14.0, 0.0));
    let removed = -data.reduction_db;
    bar_meter(
        ui,
        Rect::from_min_size(pos2(meters.left(), meter_top), vec2(meters.width(), 20.0)),
        removed / 40.0,
        None,
        REMOVED,
        "Removed",
        &format!("{removed:.1} dB"),
    );
    bar_meter(
        ui,
        Rect::from_min_size(
            pos2(meters.left(), meter_top + 26.0),
            vec2(meters.width(), 20.0),
        ),
        data.voice_probability,
        None,
        GATE,
        "Voice",
        &format!("{:.0} %", data.voice_probability * 100.0),
    );

    let (hint, warning) = match (params.noise_model.value(), data.noise_status) {
        (NoiseModelChoice::RnNoise, _) => ("RNNoise: light, 10 ms delay", false),
        (NoiseModelChoice::DeepFilterNet, ModelStatus::Running) => {
            ("DeepFilterNet3: strongest, more delay", false)
        }
        (NoiseModelChoice::DeepFilterNet, ModelStatus::Loading) => {
            ("Loading DeepFilterNet, RNNoise fills in", true)
        }
        (NoiseModelChoice::DeepFilterNet, ModelStatus::Late) => {
            ("CPU overload: RNNoise fills in", true)
        }
        (NoiseModelChoice::DeepFilterNet, ModelStatus::Failed) => {
            ("DeepFilterNet failed, using RNNoise", true)
        }
    };
    ui.painter().text(
        pos2(meters.left(), meter_top + 60.0),
        Align2::LEFT_CENTER,
        hint,
        font(10.0),
        if warning { REMOVED } else { TEXT_FAINT },
    );
}

fn draw_echo(ui: &mut Ui, rect: Rect, params: &ONecParams, setter: &ParamSetter, data: &Telemetry) {
    panel(ui, rect, "ECHO", ECHO);
    power_switch(
        ui,
        &ParamSwitch::new(&params.echo_enabled, setter),
        rect.right_top() + vec2(-20.0, 17.0),
        ECHO,
    )
    .on_hover_text(params.echo_enabled.name());
    let top = rect.top() + 36.0;
    let mut centres = columns(rect, 2);
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.echo_suppression, setter),
            knob_rect(x, top),
            "Suppression",
            ECHO,
            false,
        );
    }
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.echo_tail_ms, setter),
            knob_rect(x, top),
            "Tail",
            ECHO,
            false,
        );
    }

    let meter_top = top + KNOB_SIZE.1 + 8.0;
    let inner = rect.shrink2(vec2(14.0, 0.0));
    let enabled = params.echo_enabled.value();
    let erle = if enabled { data.erle_db } else { 0.0 };
    bar_meter(
        ui,
        Rect::from_min_size(pos2(inner.left(), meter_top), vec2(inner.width(), 20.0)),
        erle / 40.0,
        None,
        ECHO,
        "Echo cancelled",
        &format!("{erle:.1} dB"),
    );

    let pills_top = meter_top + 30.0;
    let pill_width = (inner.width() - 8.0) / 2.0;
    let state = if !enabled {
        ("BYPASSED", false)
    } else if !data.reference_active {
        ("WAITING FOR REF", false)
    } else if data.echo_adapted {
        ("LOCKED", true)
    } else {
        ("LEARNING", true)
    };
    pill(
        ui,
        Rect::from_min_size(pos2(inner.left(), pills_top), vec2(pill_width, 18.0)),
        state.0,
        ECHO,
        state.1,
    );
    pill(
        ui,
        Rect::from_min_size(
            pos2(inner.left() + pill_width + 8.0, pills_top),
            vec2(pill_width, 18.0),
        ),
        "DOUBLE TALK",
        VOICE,
        enabled && data.double_talk,
    );
}

fn draw_gate(ui: &mut Ui, rect: Rect, params: &ONecParams, setter: &ParamSetter, data: &Telemetry) {
    panel(ui, rect, "GATE", GATE);
    let mode = params.gate_mode.value();
    let lit = mode != GateMode::Off;
    let status = Rect::from_min_size(rect.right_top() + vec2(-78.0, 8.0), vec2(66.0, 18.0));
    pill(
        ui,
        status,
        if data.gate_open { "OPEN" } else { "CLOSED" },
        GATE,
        lit && data.gate_open,
    );

    let selector = Rect::from_min_max(
        rect.min + vec2(70.0, 7.0),
        status.left_top() + vec2(-8.0, 20.0),
    );
    enum_selector(ui, setter, &params.gate_mode, selector, GATE);

    let top = rect.top() + 40.0;
    let mut centres = columns(rect, 4);
    if let Some(x) = centres.next() {
        if mode == GateMode::Level {
            knob(
                ui,
                &ParamControl::new(&params.gate_level_db, setter),
                knob_rect(x, top),
                "Threshold",
                GATE,
                false,
            );
        } else {
            knob(
                ui,
                &ParamControl::new(&params.gate_voice_threshold, setter),
                knob_rect(x, top),
                "Threshold",
                GATE,
                false,
            );
        }
    }
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.gate_range_db, setter),
            knob_rect(x, top),
            "Range",
            GATE,
            false,
        );
    }
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.gate_hold_ms, setter),
            knob_rect(x, top),
            "Hold",
            GATE,
            false,
        );
    }
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.gate_release_ms, setter),
            knob_rect(x, top),
            "Release",
            GATE,
            false,
        );
    }

    let inner = rect.shrink2(vec2(14.0, 0.0));
    let meter_top = top + KNOB_SIZE.1 + 4.0;
    let attenuation = -data.gate_gain_db;
    let range = params.gate_range_db.value().max(1.0);
    bar_meter(
        ui,
        Rect::from_min_size(pos2(inner.left(), meter_top), vec2(inner.width(), 20.0)),
        if lit { attenuation / range } else { 0.0 },
        None,
        GATE,
        "Gate attenuation",
        &format!("{:.1} dB", if lit { attenuation } else { 0.0 }),
    );
    let hint = match mode {
        GateMode::Off => "Gate off: everything noise suppression keeps passes",
        GateMode::Voice => "Opens on detected speech (automatic sensitivity)",
        GateMode::Level => "Opens above a fixed level (manual sensitivity)",
    };
    ui.painter().text(
        pos2(inner.left(), meter_top + 32.0),
        Align2::LEFT_CENTER,
        hint,
        font(10.0),
        TEXT_FAINT,
    );
}

fn draw_leveler(
    ui: &mut Ui,
    rect: Rect,
    params: &ONecParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    panel(ui, rect, "LEVELER", LEVELER);
    power_switch(
        ui,
        &ParamSwitch::new(&params.leveler_enabled, setter),
        rect.right_top() + vec2(-20.0, 17.0),
        LEVELER,
    )
    .on_hover_text(params.leveler_enabled.name());
    let selector = Rect::from_min_max(
        rect.min + vec2(92.0, 7.0),
        rect.right_top() + vec2(-38.0, 27.0),
    );
    enum_selector(ui, setter, &params.leveler_speed, selector, LEVELER);

    let top = rect.top() + 40.0;
    let knobs = [
        (&params.leveler_target_db, "Target"),
        (&params.leveler_amount, "Amount"),
        (&params.leveler_max_boost_db, "Max boost"),
        (&params.leveler_max_cut_db, "Max cut"),
    ];
    for (x, (param, label)) in columns(rect, knobs.len()).zip(knobs) {
        knob(
            ui,
            &ParamControl::new(param, setter),
            knob_rect(x, top),
            label,
            LEVELER,
            false,
        );
    }

    let enabled = params.leveler_enabled.value();
    let inner = rect.shrink2(vec2(14.0, 0.0));
    let meter_top = top + KNOB_SIZE.1 + 4.0;
    let row = |index: f32| {
        Rect::from_min_size(
            pos2(inner.left(), meter_top + 26.0 * index),
            vec2(inner.width(), 20.0),
        )
    };
    let target = params.leveler_target_db.value();
    let heard = data.speech_level_db > FLOOR_DB;
    let speech_text = if heard {
        format!("{:.1} dB", data.speech_level_db)
    } else {
        String::from("no speech yet")
    };
    bar_meter(
        ui,
        row(0.0),
        if heard {
            (data.speech_level_db + 60.0) / 60.0
        } else {
            0.0
        },
        Some((target + 60.0) / 60.0),
        LEVELER,
        "Speech level",
        &speech_text,
    );
    let gain = if enabled { data.leveler_gain_db } else { 0.0 };
    let range = params
        .leveler_max_boost_db
        .value()
        .max(params.leveler_max_cut_db.value())
        .max(1.0);
    bipolar_meter(
        ui,
        row(1.0),
        gain,
        range,
        LEVELER,
        "Gain",
        &format!("{gain:+.1} dB"),
    );
    let limiting = if enabled { -data.limiter_db } else { 0.0 };
    bar_meter(
        ui,
        row(2.0),
        limiting / 12.0,
        None,
        REMOVED,
        "Peak limiting",
        &format!("{limiting:.1} dB"),
    );
}

fn draw_output(
    ui: &mut Ui,
    rect: Rect,
    params: &ONecParams,
    setter: &ParamSetter,
    data: &Telemetry,
) {
    panel(ui, rect, "LEVEL", INPUT);
    let top = rect.top() + 36.0;
    let knobs = Rect::from_min_max(rect.min, pos2(rect.right() - 44.0, rect.bottom()));
    let mut centres = columns(knobs, 1);
    if let Some(x) = centres.next() {
        knob(
            ui,
            &ParamControl::new(&params.input_gain_db, setter),
            knob_rect(x, top),
            "Input",
            INPUT,
            true,
        );
        knob(
            ui,
            &ParamControl::new(&params.output_gain_db, setter),
            knob_rect(x, top + KNOB_SIZE.1 + 4.0),
            "Output",
            VOICE,
            true,
        );
    }
    let meter_bottom = rect.bottom() - 12.0;
    let meter_top = rect.top() + 40.0;
    let input = Rect::from_min_max(
        pos2(rect.right() - 44.0, meter_top),
        pos2(rect.right() - 32.0, meter_bottom),
    );
    let output = Rect::from_min_max(
        pos2(rect.right() - 26.0, meter_top),
        pos2(rect.right() - 14.0, meter_bottom),
    );
    vertical_meter(ui, input, data.input_peak_db, INPUT, "IN");
    vertical_meter(ui, output, data.output_peak_db, VOICE, "OUT");
}
