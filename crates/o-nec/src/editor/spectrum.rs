//! Spectrum display: input, output, removed noise, echo reference and the draggable
//! high-pass filter.

use aud_ui::theme::{
    BORDER, GRID, GRID_STRONG, PANEL, TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke,
};
use nih_plug::prelude::{Param, ParamSetter};
use nih_plug_egui::egui::{
    Align2, Color32, CornerRadius, CursorIcon, Id, Mesh, Pos2, Rect, Sense, Shape, StrokeKind, Ui,
    pos2, vec2,
};

use crate::dsp::analyzer::{SPECTRUM_BANDS, SPECTRUM_MAX_HZ, SPECTRUM_MIN_HZ, band_frequency};
use crate::editor::colors::{ECHO, INPUT, REMOVED, VOICE};
use crate::params::ONecParams;
use crate::telemetry::Telemetry;

const TOP_DB: f32 = 0.0;
const BOTTOM_DB: f32 = -100.0;

/// Vertical position of the 0 dB line of the filter curve below the plot top.
const CURVE_OFFSET: f32 = 26.0;
/// Pixels per dB of the filter curve.
const CURVE_SCALE: f32 = 1.6;

pub fn draw(ui: &mut Ui, rect: Rect, params: &ONecParams, setter: &ParamSetter, data: &Telemetry) {
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same(6),
        PANEL,
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
    let plot = rect.shrink2(vec2(34.0, 14.0));
    let plot = Rect::from_min_max(pos2(plot.left() - 8.0, plot.top()), plot.max);
    draw_grid(ui, plot);

    let x_of = |band: usize| frequency_to_x(plot, band_frequency(band));
    let y_of = |db: f32| db_to_y(plot, db);

    let input: Vec<Pos2> = (0..SPECTRUM_BANDS)
        .map(|band| pos2(x_of(band), y_of(data.spectrum_input[band])))
        .collect();
    let output: Vec<Pos2> = (0..SPECTRUM_BANDS)
        .map(|band| pos2(x_of(band), y_of(data.spectrum_output[band])))
        .collect();

    let painter = ui.painter_at(plot);
    painter.add(area(
        &input,
        plot.bottom(),
        alpha(INPUT, 0.16),
        alpha(INPUT, 0.04),
    ));
    painter.add(removed_area(&input, &output));
    painter.add(area(
        &output,
        plot.bottom(),
        alpha(VOICE, 0.35),
        alpha(VOICE, 0.03),
    ));
    painter.add(Shape::line(input.clone(), stroke(1.0, alpha(INPUT, 0.7))));
    painter.add(Shape::line(output, stroke(1.6, VOICE)));

    if data.reference_active {
        let reference = (0..SPECTRUM_BANDS)
            .map(|band| pos2(x_of(band), y_of(data.spectrum_reference[band])))
            .collect();
        painter.add(Shape::line(reference, stroke(1.2, alpha(ECHO, 0.85))));
    }

    draw_high_pass(ui, plot, params, setter);
    draw_hover_readout(ui, plot, data);
    draw_legend(ui, plot, data.reference_active);
}

fn draw_grid(ui: &Ui, plot: Rect) {
    let painter = ui.painter();
    for frequency in [
        30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 200.0, 300.0, 400.0, 600.0, 700.0, 800.0, 900.0,
        3_000.0, 4_000.0, 6_000.0, 7_000.0, 8_000.0, 9_000.0,
    ] {
        let x = frequency_to_x(plot, frequency);
        painter.vline(x, plot.y_range(), stroke(1.0, alpha(GRID, 0.6)));
    }
    for (frequency, label) in [
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1_000.0, "1k"),
        (2_000.0, "2k"),
        (5_000.0, "5k"),
        (10_000.0, "10k"),
    ] {
        let x = frequency_to_x(plot, frequency);
        let strong = matches!(label, "100" | "1k" | "10k");
        let stroke = stroke(1.0, if strong { GRID_STRONG } else { GRID });
        painter.vline(x, plot.y_range(), stroke);
        painter.text(
            pos2(x, plot.bottom() + 2.0),
            Align2::CENTER_TOP,
            label,
            font(10.0),
            TEXT_FAINT,
        );
    }
    let mut db = TOP_DB;
    while db >= BOTTOM_DB {
        let y = db_to_y(plot, db);
        painter.hline(plot.x_range(), y, stroke(1.0, GRID));
        painter.text(
            pos2(plot.right() + 6.0, y),
            Align2::LEFT_CENTER,
            format!("{db:.0}"),
            font(10.0),
            TEXT_FAINT,
        );
        db -= 20.0;
    }
}

fn draw_high_pass(ui: &mut Ui, plot: Rect, params: &ONecParams, setter: &ParamSetter) {
    let param = &params.high_pass_hz;
    let enabled = params.high_pass_enabled.value();
    let cutoff = param.value();
    let color = if enabled { VOICE } else { TEXT_FAINT };

    // Analytic magnitude of the second-order Butterworth high-pass.
    let curve_y = |frequency: f32| {
        let ratio = (frequency / cutoff).powi(4);
        let gain_db = 10.0 * (ratio / (1.0 + ratio)).log10();
        plot.top() + CURVE_OFFSET - gain_db.max(-60.0) * CURVE_SCALE
    };
    let points: Vec<Pos2> = (0..=200)
        .map(|step| {
            let x = plot.left() + plot.width() * step as f32 / 200.0;
            pos2(x, curve_y(x_to_frequency(plot, x)))
        })
        .collect();
    let painter = ui.painter_at(plot);
    if enabled {
        let mut shade = points.clone();
        shade.push(pos2(plot.left(), plot.top()));
        let mut mesh = Mesh::default();
        let top = plot.top();
        for pair in points.windows(2) {
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(pair[0].x, top), alpha(Color32::BLACK, 0.0));
            mesh.colored_vertex(pair[0], alpha(VOICE, 0.10));
            mesh.colored_vertex(pos2(pair[1].x, top), alpha(Color32::BLACK, 0.0));
            mesh.colored_vertex(pair[1], alpha(VOICE, 0.10));
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 2, base + 3);
        }
        painter.add(Shape::mesh(mesh));
    }
    painter.add(Shape::line(points, stroke(1.4, alpha(color, 0.9))));

    let handle = pos2(frequency_to_x(plot, cutoff), curve_y(cutoff));
    let hit = Rect::from_center_size(handle, vec2(22.0, 22.0));
    let id = Id::new("hpf-handle");
    let response = ui
        .interact(hit, id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::ResizeHorizontal)
        .on_hover_text("High-pass: drag to tune, double-click to toggle");
    if response.double_clicked() {
        setter.begin_set_parameter(&params.high_pass_enabled);
        setter.set_parameter(&params.high_pass_enabled, !enabled);
        setter.end_set_parameter(&params.high_pass_enabled);
    }
    if response.drag_started() {
        setter.begin_set_parameter(param);
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let frequency = x_to_frequency(plot, pointer.x);
        setter.set_parameter(param, frequency);
    }
    if response.drag_stopped() {
        setter.end_set_parameter(param);
    }

    let active = response.hovered() || response.dragged();
    let painter = ui.painter();
    painter.circle_filled(handle, if active { 7.0 } else { 5.5 }, color);
    painter.circle_stroke(handle, 9.0, stroke(1.0, alpha(color, 0.4)));
    painter.text(
        handle + vec2(10.0, -12.0),
        Align2::LEFT_BOTTOM,
        format!(
            "HPF {}",
            param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
        ),
        mono(10.0),
        color,
    );
}

fn draw_hover_readout(ui: &Ui, plot: Rect, data: &Telemetry) {
    let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    if !plot.contains(pointer) {
        return;
    }
    let frequency = x_to_frequency(plot, pointer.x);
    let position = (frequency / SPECTRUM_MIN_HZ).ln() / (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).ln();
    let band = ((position * (SPECTRUM_BANDS - 1) as f32).round() as usize).min(SPECTRUM_BANDS - 1);
    let painter = ui.painter();
    painter.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.25)));
    let removed = (data.spectrum_input[band] - data.spectrum_output[band]).max(0.0);
    let label = if frequency >= 1_000.0 {
        format!("{:.2} kHz", frequency / 1_000.0)
    } else {
        format!("{frequency:.0} Hz")
    };
    let text = format!(
        "{label}\nin  {:>6.1} dB\nout {:>6.1} dB\n−   {removed:>6.1} dB",
        data.spectrum_input[band], data.spectrum_output[band]
    );
    let anchor = if pointer.x > plot.center().x {
        Align2::RIGHT_TOP
    } else {
        Align2::LEFT_TOP
    };
    let offset = if pointer.x > plot.center().x {
        -10.0
    } else {
        10.0
    };
    painter.text(
        pos2(pointer.x + offset, plot.top() + 40.0),
        anchor,
        text,
        mono(10.0),
        TEXT_DIM,
    );
}

fn draw_legend(ui: &Ui, plot: Rect, reference_active: bool) {
    let painter = ui.painter();
    let mut x = plot.right() - 8.0;
    let y = plot.top() + 10.0;
    let mut entries = vec![("Output", VOICE), ("Removed", REMOVED), ("Input", INPUT)];
    if reference_active {
        entries.insert(0, ("Echo ref", ECHO));
    }
    for (label, color) in entries {
        let galley = painter.layout_no_wrap(label.to_owned(), font(10.0), TEXT_DIM);
        x -= galley.size().x;
        painter.galley(pos2(x, y - galley.size().y * 0.5), galley, TEXT_DIM);
        x -= 12.0;
        painter.circle_filled(pos2(x + 4.0, y), 3.5, color);
        x -= 14.0;
    }
}

/// Filled area under a curve with a vertical gradient.
fn area(points: &[Pos2], bottom: f32, top_color: Color32, bottom_color: Color32) -> Shape {
    let mut mesh = Mesh::default();
    for pair in points.windows(2) {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(pair[0], top_color);
        mesh.colored_vertex(pos2(pair[0].x, bottom), bottom_color);
        mesh.colored_vertex(pair[1], top_color);
        mesh.colored_vertex(pos2(pair[1].x, bottom), bottom_color);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 2, base + 3);
    }
    Shape::mesh(mesh)
}

/// Region where the output lies below the input: what the processing removed.
fn removed_area(input: &[Pos2], output: &[Pos2]) -> Shape {
    let mut mesh = Mesh::default();
    let strong = alpha(REMOVED, 0.45);
    let faint = alpha(REMOVED, 0.12);
    for index in 0..input.len().saturating_sub(1) {
        // Screen y grows downwards: removed where the output is lower on screen.
        let left_top = input[index];
        let right_top = input[index + 1];
        let left_bottom = pos2(output[index].x, output[index].y.max(left_top.y));
        let right_bottom = pos2(output[index + 1].x, output[index + 1].y.max(right_top.y));
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(left_top, strong);
        mesh.colored_vertex(left_bottom, faint);
        mesh.colored_vertex(right_top, strong);
        mesh.colored_vertex(right_bottom, faint);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 2, base + 3);
    }
    Shape::mesh(mesh)
}

fn frequency_to_x(plot: Rect, frequency: f32) -> f32 {
    let position = (frequency.max(SPECTRUM_MIN_HZ) / SPECTRUM_MIN_HZ).ln()
        / (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).ln();
    plot.left() + plot.width() * position
}

fn x_to_frequency(plot: Rect, x: f32) -> f32 {
    let position = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
    SPECTRUM_MIN_HZ * (SPECTRUM_MAX_HZ / SPECTRUM_MIN_HZ).powf(position)
}

fn db_to_y(plot: Rect, db: f32) -> f32 {
    let position = ((db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
    plot.bottom() - plot.height() * position
}
