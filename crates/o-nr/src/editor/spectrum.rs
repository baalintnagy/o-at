//! Spectrum display: input, output, what is removed and the noise estimate.

use aud_ui::theme::{
    BORDER, GRID, GRID_STRONG, PANEL, TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke,
};
use nih_plug_egui::egui::{
    Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Shape, StrokeKind, Ui, pos2, vec2,
};

use crate::editor::{INPUT, KEPT, NOISE, REMOVED};
use crate::telemetry::{SPECTRUM_BANDS, SPECTRUM_HIGH_HZ, SPECTRUM_LOW_HZ, Telemetry, band_hz};

const TOP_DB: f32 = 0.0;
const BOTTOM_DB: f32 = -120.0;

pub fn draw(ui: &mut Ui, rect: Rect, data: &Telemetry) {
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

    let bands = data.bands.min(SPECTRUM_BANDS);
    let curve = |values: &[f32]| -> Vec<Pos2> {
        (0..bands)
            .map(|band| {
                pos2(
                    frequency_to_x(plot, band_hz(band as f32 + 0.5)),
                    db_to_y(plot, values[band]),
                )
            })
            .collect()
    };
    let input = curve(&data.input_db);
    let output = curve(&data.output_db);
    let noise = curve(&data.noise_db);

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
        alpha(KEPT, 0.35),
        alpha(KEPT, 0.03),
    ));
    painter.add(Shape::line(input, stroke(1.0, alpha(INPUT, 0.7))));
    painter.add(Shape::line(output, stroke(1.6, KEPT)));
    painter.add(Shape::line(noise, stroke(1.4, alpha(NOISE, 0.9))));

    draw_hover_readout(ui, plot, data);
    draw_legend(ui, plot);
}

fn draw_grid(ui: &Ui, plot: Rect) {
    let painter = ui.painter();
    for frequency in [
        30.0, 40.0, 60.0, 70.0, 80.0, 90.0, 300.0, 400.0, 600.0, 700.0, 800.0, 900.0, 3_000.0,
        4_000.0, 6_000.0, 7_000.0, 8_000.0, 9_000.0,
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
        (20_000.0, "20k"),
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

fn draw_hover_readout(ui: &Ui, plot: Rect, data: &Telemetry) {
    let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
        return;
    };
    if !plot.contains(pointer) || data.bands == 0 {
        return;
    }
    let frequency = x_to_frequency(plot, pointer.x);
    let position = (frequency / SPECTRUM_LOW_HZ).ln() / (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).ln();
    let band = ((position * SPECTRUM_BANDS as f32) as usize).min(data.bands - 1);
    let painter = ui.painter();
    painter.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.25)));
    let removed = (data.input_db[band] - data.output_db[band]).max(0.0);
    let label = if frequency >= 1_000.0 {
        format!("{:.2} kHz", frequency / 1_000.0)
    } else {
        format!("{frequency:.0} Hz")
    };
    let text = format!(
        "{label}\nin    {:>6.1} dB\nout   {:>6.1} dB\nnoise {:>6.1} dB\n−     {removed:>6.1} dB",
        data.input_db[band], data.output_db[band], data.noise_db[band]
    );
    let right = pointer.x > plot.center().x;
    painter.text(
        pos2(
            pointer.x + if right { -10.0 } else { 10.0 },
            plot.top() + 40.0,
        ),
        if right {
            Align2::RIGHT_TOP
        } else {
            Align2::LEFT_TOP
        },
        text,
        mono(10.0),
        TEXT_DIM,
    );
}

fn draw_legend(ui: &Ui, plot: Rect) {
    let painter = ui.painter();
    let mut x = plot.right() - 8.0;
    let y = plot.top() + 10.0;
    for (label, color) in [
        ("Noise", NOISE),
        ("Output", KEPT),
        ("Removed", REMOVED),
        ("Input", INPUT),
    ] {
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
    for (inputs, outputs) in input.windows(2).zip(output.windows(2)) {
        // Screen y grows downwards: removed where the output is lower on screen.
        let left_bottom = pos2(outputs[0].x, outputs[0].y.max(inputs[0].y));
        let right_bottom = pos2(outputs[1].x, outputs[1].y.max(inputs[1].y));
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(inputs[0], strong);
        mesh.colored_vertex(left_bottom, faint);
        mesh.colored_vertex(inputs[1], strong);
        mesh.colored_vertex(right_bottom, faint);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 2, base + 3);
    }
    Shape::mesh(mesh)
}

fn frequency_to_x(plot: Rect, frequency: f32) -> f32 {
    let position = (frequency.max(SPECTRUM_LOW_HZ) / SPECTRUM_LOW_HZ).ln()
        / (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).ln();
    plot.left() + plot.width() * position
}

fn x_to_frequency(plot: Rect, x: f32) -> f32 {
    let position = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0);
    SPECTRUM_LOW_HZ * (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).powf(position)
}

fn db_to_y(plot: Rect, db: f32) -> f32 {
    let position = ((db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0);
    plot.bottom() - plot.height() * position
}
