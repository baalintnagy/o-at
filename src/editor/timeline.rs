//! Scrolling activity history: levels, voice probability, gate state, echo activity,
//! speech leveler gain and the draggable gate threshold.

use aud_ui::theme::{BORDER, GRID, PANEL, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use nih_plug::prelude::{Param, ParamSetter};
use nih_plug_egui::egui::{
    Align2, CornerRadius, CursorIcon, Id, Mesh, Pos2, Rect, Response, Sense, Shape, StrokeKind, Ui,
    pos2, vec2,
};

use crate::editor::colors::{ECHO, GATE, INPUT, LEVELER, VOICE};
use crate::params::{GateMode, ONecParams};
use crate::telemetry::{HISTORY_LEN, Telemetry};

const TOP_DB: f32 = 0.0;
const BOTTOM_DB: f32 = -80.0;

/// Leveler gain lane at the bottom of the plot: its centre height above the bottom and
/// its height per dB, in pixels.
const LEVELER_LANE_CENTRE: f32 = 12.0;
const LEVELER_LANE_SCALE: f32 = 10.0 / 24.0;

pub fn draw(ui: &mut Ui, rect: Rect, params: &ONecParams, setter: &ParamSetter, data: &Telemetry) {
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same(6),
        PANEL,
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
    let plot = Rect::from_min_max(rect.min + vec2(10.0, 8.0), rect.max - vec2(34.0, 8.0));
    let painter = ui.painter_at(plot);
    for db in [-20.0, -40.0, -60.0] {
        let y = db_to_y(plot, db);
        painter.hline(plot.x_range(), y, stroke(1.0, GRID));
        ui.painter().text(
            pos2(plot.right() + 6.0, y),
            Align2::LEFT_CENTER,
            format!("{db:.0}"),
            font(10.0),
            TEXT_FAINT,
        );
    }

    let step = plot.width() / (HISTORY_LEN - 1) as f32;
    let x_of = |index: usize| plot.left() + step * index as f32;

    // Closed gate: darken the columns.
    let gate: Vec<f32> = data.chronological(&data.gate_history).collect();
    let mut shade = Mesh::default();
    for (index, gain) in gate.iter().enumerate() {
        let closed = (1.0 - gain).clamp(0.0, 1.0);
        if closed < 0.05 {
            continue;
        }
        let column = Rect::from_min_max(
            pos2(x_of(index), plot.top()),
            pos2(x_of(index) + step, plot.bottom()),
        );
        shade.add_colored_rect(column, alpha(GATE, 0.10 * closed));
    }
    painter.add(Shape::mesh(shade));

    // Echo reference activity strip.
    let mut echo = Mesh::default();
    for (index, active) in data.chronological(&data.echo_history).enumerate() {
        if active {
            let strip = Rect::from_min_max(
                pos2(x_of(index), plot.top()),
                pos2(x_of(index) + step, plot.top() + 3.0),
            );
            echo.add_colored_rect(strip, alpha(ECHO, 0.8));
        }
    }
    painter.add(Shape::mesh(echo));

    let input: Vec<Pos2> = data
        .chronological(&data.input_history_db)
        .enumerate()
        .map(|(index, db)| pos2(x_of(index), db_to_y(plot, db)))
        .collect();
    let output: Vec<Pos2> = data
        .chronological(&data.output_history_db)
        .enumerate()
        .map(|(index, db)| pos2(x_of(index), db_to_y(plot, db)))
        .collect();
    let mut fill = Mesh::default();
    for pair in output.windows(2) {
        let base = fill.vertices.len() as u32;
        fill.colored_vertex(pair[0], alpha(VOICE, 0.35));
        fill.colored_vertex(pos2(pair[0].x, plot.bottom()), alpha(VOICE, 0.04));
        fill.colored_vertex(pair[1], alpha(VOICE, 0.35));
        fill.colored_vertex(pos2(pair[1].x, plot.bottom()), alpha(VOICE, 0.04));
        fill.add_triangle(base, base + 1, base + 2);
        fill.add_triangle(base + 1, base + 2, base + 3);
    }
    painter.add(Shape::mesh(fill));
    painter.add(Shape::line(input, stroke(1.0, alpha(INPUT, 0.6))));
    painter.add(Shape::line(output, stroke(1.3, VOICE)));

    let voice: Vec<Pos2> = data
        .chronological(&data.voice_history)
        .enumerate()
        .map(|(index, probability)| pos2(x_of(index), probability_to_y(plot, probability)))
        .collect();
    let mut voice_fill = Mesh::default();
    for pair in voice.windows(2) {
        let base = voice_fill.vertices.len() as u32;
        voice_fill.colored_vertex(pair[0], alpha(GATE, 0.22));
        voice_fill.colored_vertex(pos2(pair[0].x, plot.bottom()), alpha(GATE, 0.02));
        voice_fill.colored_vertex(pair[1], alpha(GATE, 0.22));
        voice_fill.colored_vertex(pos2(pair[1].x, plot.bottom()), alpha(GATE, 0.02));
        voice_fill.add_triangle(base, base + 1, base + 2);
        voice_fill.add_triangle(base + 1, base + 2, base + 3);
    }
    painter.add(Shape::mesh(voice_fill));
    painter.add(Shape::line(voice, stroke(1.2, alpha(GATE, 0.9))));

    let leveling = params.leveler_enabled.value();
    if leveling {
        let centre = plot.bottom() - LEVELER_LANE_CENTRE;
        painter.hline(plot.x_range(), centre, stroke(1.0, alpha(LEVELER, 0.25)));
        let gain: Vec<Pos2> = data
            .chronological(&data.leveler_history_db)
            .enumerate()
            .map(|(index, db)| {
                let offset = (db * LEVELER_LANE_SCALE)
                    .clamp(-LEVELER_LANE_CENTRE + 1.0, LEVELER_LANE_CENTRE);
                pos2(x_of(index), centre - offset)
            })
            .collect();
        painter.add(Shape::line(gain, stroke(1.3, LEVELER)));
    }

    draw_threshold(ui, plot, params, setter);

    let painter = ui.painter();
    let mut legend = vec![
        ("Voice", GATE),
        ("Output", VOICE),
        ("Input", INPUT),
        ("Echo", ECHO),
    ];
    if leveling {
        legend.push(("Leveler gain", LEVELER));
    }
    let galleys: Vec<_> = legend
        .iter()
        .map(|(label, _)| painter.layout_no_wrap((*label).to_owned(), font(10.0), TEXT_DIM))
        .collect();
    let total: f32 = galleys.iter().map(|galley| galley.size().x + 24.0).sum();
    let backing = Rect::from_min_size(plot.min + vec2(2.0, 5.0), vec2(total, 15.0));
    painter.rect_filled(backing, CornerRadius::same(3), alpha(PANEL, 0.85));
    let mut x = plot.left() + 6.0;
    for ((_, color), galley) in legend.iter().zip(galleys) {
        painter.circle_filled(pos2(x + 3.0, plot.top() + 12.0), 3.0, *color);
        let width = galley.size().x;
        painter.galley(pos2(x + 10.0, plot.top() + 6.0), galley, TEXT_DIM);
        x += width + 24.0;
    }
}

fn draw_threshold(ui: &mut Ui, plot: Rect, params: &ONecParams, setter: &ParamSetter) {
    let mode = params.gate_mode.value();
    let (y, text) = match mode {
        GateMode::Off => return,
        GateMode::Voice => {
            let threshold = params.gate_voice_threshold.value() / 100.0;
            (
                probability_to_y(plot, threshold),
                format!("voice ≥ {:.0} %", threshold * 100.0),
            )
        }
        GateMode::Level => {
            let threshold = params.gate_level_db.value();
            (
                db_to_y(plot, threshold),
                format!("level ≥ {threshold:.1} dB"),
            )
        }
    };
    let band = Rect::from_min_max(pos2(plot.left(), y - 5.0), pos2(plot.right(), y + 5.0));
    let response = ui
        .interact(band, Id::new("gate-threshold"), Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeVertical)
        .on_hover_text("Gate threshold: drag to adjust");
    match mode {
        GateMode::Voice => drag_threshold(&response, setter, &params.gate_voice_threshold, |y| {
            y_to_probability(plot, y) * 100.0
        }),
        GateMode::Level => {
            drag_threshold(&response, setter, &params.gate_level_db, |y| {
                y_to_db(plot, y)
            });
        }
        GateMode::Off => {}
    }
    let active = response.hovered() || response.dragged();
    let painter = ui.painter_at(plot);
    let stroke = stroke(
        if active { 2.0 } else { 1.0 },
        alpha(GATE, if active { 1.0 } else { 0.7 }),
    );
    let mut x = plot.left();
    while x < plot.right() {
        painter.line_segment([pos2(x, y), pos2((x + 6.0).min(plot.right()), y)], stroke);
        x += 10.0;
    }
    painter.text(
        pos2(plot.right() - 4.0, y - 3.0),
        Align2::RIGHT_BOTTOM,
        text,
        mono(10.0),
        GATE,
    );
}

fn drag_threshold<P: Param<Plain = f32>>(
    response: &Response,
    setter: &ParamSetter,
    param: &P,
    value_at: impl Fn(f32) -> f32,
) {
    if response.drag_started() {
        setter.begin_set_parameter(param);
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        setter.set_parameter(param, value_at(pointer.y));
    }
    if response.drag_stopped() {
        setter.end_set_parameter(param);
    }
}

fn db_to_y(plot: Rect, db: f32) -> f32 {
    let position = ((db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
    plot.bottom() - plot.height() * position
}

fn y_to_db(plot: Rect, y: f32) -> f32 {
    let position = ((plot.bottom() - y) / plot.height()).clamp(0.0, 1.0);
    BOTTOM_DB + (TOP_DB - BOTTOM_DB) * position
}

fn probability_to_y(plot: Rect, probability: f32) -> f32 {
    plot.bottom() - plot.height() * probability.clamp(0.0, 1.0)
}

fn y_to_probability(plot: Rect, y: f32) -> f32 {
    ((plot.bottom() - y) / plot.height()).clamp(0.0, 1.0)
}
