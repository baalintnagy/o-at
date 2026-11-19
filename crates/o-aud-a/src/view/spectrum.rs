//! The spectrum analyzer view.

use aud_ui::theme::{TEXT, TEXT_DIM, alpha, font, stroke};
use aud_ui::widgets::panel;
use egui::{Align2, Color32, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, Ui, pos2};

use crate::analysis::Published;
use crate::analysis::fft::{BandMap, FLOOR_DB};
use crate::settings::{ChannelMode, FreqLimit, FreqScale, SpectrumSettings};
use crate::style::SpectrumStyle;
use crate::view::colormap::{Colors, paint};
use crate::view::plot::{
    FreqAxis, Header, format_db, format_hz_precise, freq_grid_vertical, level_grid, note_name,
    plot_background, sample_db, tooltip,
};

/// Top of the level scale in dBFS.
const TOP_DB: f32 = 6.0;
/// Pivot of the display slope in Hz.
const SLOPE_PIVOT_HZ: f32 = 1_000.0;

/// Per-pixel bin mapping, rebuilt when the plot or the FFT changes.
#[derive(Default)]
pub struct SpectrumView {
    key: (usize, (u32, u32), bool, (u32, u32), u32),
    columns: BandMap,
    /// Display slope per column in dB.
    tilt: Vec<f32>,
    points: Vec<Pos2>,
}

impl SpectrumView {
    fn prepare(&mut self, width: usize, axis: FreqAxis, bins: usize, bin_hz: f32, slope: f32) {
        let key = (
            width,
            (axis.low_hz.to_bits(), axis.high_hz.to_bits()),
            axis.scale == FreqScale::Linear,
            (bins as u32, bin_hz.to_bits()),
            slope.to_bits(),
        );
        if key == self.key {
            return;
        }
        self.key = key;
        let log = axis.scale != FreqScale::Linear;
        self.columns = BandMap::new(width, (axis.low_hz, axis.high_hz), log, bins, bin_hz);
        self.tilt = (0..width)
            .map(|column| {
                let hz = axis.hz((column as f32 + 0.5) / width as f32);
                slope * (hz.max(1.0) / SLOPE_PIVOT_HZ).log2()
            })
            .collect();
    }
}

/// Curve colors for a channel mode: the first curve and the second.
fn curve_colors(mode: ChannelMode, style: &SpectrumStyle) -> (Color32, Color32) {
    let second = if mode == ChannelMode::MidSide {
        style.side
    } else {
        style.right
    };
    (paint(style.first), paint(second))
}

fn curve_labels(mode: ChannelMode) -> (&'static str, Option<&'static str>) {
    match mode {
        ChannelMode::LeftRight => ("LEFT", Some("RIGHT")),
        ChannelMode::MidSide => ("MID", Some("SIDE")),
        ChannelMode::Mid => ("MID", None),
        ChannelMode::Left => ("LEFT", None),
        ChannelMode::Right => ("RIGHT", None),
    }
}

/// Draws the spectrum panel. `compact` leaves out the legend details for the overview.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SpectrumView,
    data: &Published,
    settings: &mut SpectrumSettings,
    (colors, style): (&Colors, &SpectrumStyle),
    compact: bool,
) {
    panel(ui, rect, "SPECTRUM", colors.accent);
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 12.0, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    plot_background(&painter, plot);
    let spectrum = &data.spectrum;
    if data.sample_rate == 0 || spectrum.primary_db.is_empty() {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Waiting for audio",
            font(12.0),
            TEXT_DIM,
        );
        return;
    }

    let axis = FreqAxis {
        low_hz: if settings.scale == FreqScale::Linear {
            0.0
        } else {
            FreqLimit::LOW_HZ
        },
        high_hz: settings.limit.high_hz(data.sample_rate as f32),
        scale: settings.scale,
    };
    let floor = TOP_DB - settings.range;
    freq_grid_vertical(&painter, plot, axis, true);
    level_grid(
        &painter,
        plot,
        (floor, TOP_DB),
        if settings.range > 90.0 { 20.0 } else { 12.0 },
    );

    let width = plot.width().max(1.0) as usize;
    view.prepare(
        width,
        axis,
        spectrum.primary_db.len(),
        spectrum.bin_hz,
        settings.slope,
    );
    let (primary_color, secondary_color) = curve_colors(settings.channels, style);
    let y_of = |db: f32| {
        let fraction = ((db - floor) / (TOP_DB - floor)).clamp(0.0, 1.0);
        plot.bottom() - plot.height() * fraction
    };
    let curve = |view: &mut SpectrumView, levels: &[f32]| {
        view.points.clear();
        let step = plot.width() / width as f32;
        for (column, (band, tilt)) in view.columns.bands.iter().zip(&view.tilt).enumerate() {
            let db = sample_db(levels, *band) + tilt;
            view.points
                .push(pos2(plot.left() + (column as f32 + 0.5) * step, y_of(db)));
        }
    };
    let clipped = ui.painter_at(plot);
    let peak_color = paint(style.peak);
    for peaks in [&spectrum.secondary_peak_db, &spectrum.primary_peak_db] {
        if !peaks.is_empty() {
            curve(view, peaks);
            clipped.add(Shape::line(view.points.clone(), stroke(1.0, peak_color)));
        }
    }
    let fill = style.fill.clamp(0.0, 1.0);
    if !spectrum.secondary_db.is_empty() {
        curve(view, &spectrum.secondary_db);
        fill_below(
            &clipped,
            &view.points,
            plot.bottom(),
            fade(secondary_color, 0.6 * fill),
        );
        clipped.add(Shape::line(
            view.points.clone(),
            stroke(1.3, secondary_color),
        ));
    }
    curve(view, &spectrum.primary_db);
    fill_below(
        &clipped,
        &view.points,
        plot.bottom(),
        fade(primary_color, fill),
    );
    clipped.add(Shape::line(view.points.clone(), stroke(1.5, primary_color)));

    let (primary_label, secondary_label) = curve_labels(settings.channels);
    let mut header = Header::new(&painter, rect, "SPECTRUM");
    header.legend(primary_label, primary_color);
    if let Some(label) = secondary_label {
        header.legend(label, secondary_color);
    }
    let points = (spectrum.primary_db.len() - 1) * 2;
    header.left_text(
        &[
            format!(
                "{points} pt · {:.2} Hz/bin · {:.0} ms",
                spectrum.bin_hz,
                1_000.0 / spectrum.bin_hz
            ),
            format!("{points} pt · {:.2} Hz/bin", spectrum.bin_hz),
            format!("{points} pt"),
        ],
        font(10.0),
        TEXT_DIM,
    );

    let response = ui.interact(plot, Id::new(("spectrum-plot", compact)), Sense::click());
    if response.double_clicked() {
        settings.peak_hold = !settings.peak_hold;
    }
    if let Some(pointer) = response.hover_pos() {
        let fraction = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        let hz = axis.hz(fraction);
        let column = ((fraction * width as f32) as usize).min(width - 1);
        let band = view.columns.bands[column];
        let tilt = view.tilt[column];
        clipped.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.35)));
        let pointer_db = floor + (plot.bottom() - pointer.y) / plot.height() * (TOP_DB - floor);
        clipped.hline(plot.x_range(), pointer.y, stroke(1.0, alpha(TEXT, 0.15)));
        let mut lines = vec![
            (
                format!("{}  {}", format_hz_precise(hz), note_name(hz)),
                TEXT,
            ),
            (format!("cursor {}", format_db(pointer_db, "dB")), TEXT_DIM),
        ];
        let level = sample_db(&spectrum.primary_db, band);
        lines.push((
            format!("{primary_label:<5} {}", format_db(shown(level, tilt), "dB")),
            primary_color,
        ));
        if let Some(label) = secondary_label {
            let level = sample_db(&spectrum.secondary_db, band);
            lines.push((
                format!("{label:<5} {}", format_db(shown(level, tilt), "dB")),
                secondary_color,
            ));
        }
        tooltip(&painter, pointer, plot, &lines);
    }
}

/// A level with the display slope, keeping silence silent.
fn shown(db: f32, tilt: f32) -> f32 {
    if db <= FLOOR_DB + 1.0 { db } else { db + tilt }
}

/// `color` with its opacity scaled by `factor`.
fn fade(color: Color32, factor: f32) -> Color32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (f32::from(a) * factor).round() as u8)
}

/// Fills the area between a curve and `bottom`.
fn fill_below(painter: &Painter, points: &[Pos2], bottom: f32, color: Color32) {
    let mut mesh = Mesh::default();
    for (index, point) in points.iter().enumerate() {
        mesh.colored_vertex(*point, color);
        mesh.colored_vertex(pos2(point.x, bottom), color);
        if index > 0 {
            let top = 2 * index as u32;
            mesh.add_triangle(top - 2, top - 1, top);
            mesh.add_triangle(top - 1, top + 1, top);
        }
    }
    painter.add(Shape::mesh(mesh));
}
