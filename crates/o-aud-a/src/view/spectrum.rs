//! The spectrum analyzer view: the first shown signal in full, with a curve for each
//! other one.

use aud_ui::theme::{ALERT, TEXT, TEXT_DIM, alpha, font, stroke};
use aud_ui::widgets::panel;
use egui::{Color32, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, Ui, pos2};

use crate::analysis::SignalReading;
use crate::analysis::fft::{BandMap, FLOOR_DB};
use crate::settings::{ChannelMode, FreqLimit, FreqScale, SpectrumSettings};
use crate::sources::SourceList;
use crate::style::SpectrumStyle;
use crate::view::colormap::{Colors, paint};
use crate::view::plot::{
    FreqAxis, Header, format_db, format_hz_precise, freq_grid_vertical, level_grid, message,
    note_name, plot_background, sample_db, tooltip,
};
use crate::view::sources::{NO_SIGNALS, Trace, match_offset_db};

/// Top of the level scale in dBFS.
const TOP_DB: f32 = 6.0;
/// Pivot of the display slope in Hz.
const SLOPE_PIVOT_HZ: f32 = 1_000.0;
/// Two curves within this many dB of each other mask each other.
const MASKING_DB: f32 = 6.0;
/// Curves below this level in dBFS do not count for masking.
const MASKING_FLOOR_DB: f32 = -60.0;
/// Band maps kept for sources with other FFT sizes or rates.
const SOURCE_MAPS: usize = 8;

/// What a bin mapping depends on: plot width, axis, bins and bin width.
type MapKey = (usize, (u32, u32), bool, (u32, u32));

/// Per-pixel bin mapping, rebuilt when the plot or the FFT changes.
#[derive(Default)]
pub struct SpectrumView {
    key: (MapKey, u32),
    columns: BandMap,
    /// Mappings for sources, whose bins may differ from the input's.
    source_maps: Vec<(MapKey, BandMap)>,
    /// Display slope per column in dB.
    tilt: Vec<f32>,
    points: Vec<Pos2>,
    /// Shown level per column of every curve that counts for masking: the input's
    /// primary curve first, then each shown source.
    levels: Vec<Vec<f32>>,
}

fn map_key(width: usize, axis: FreqAxis, bins: usize, bin_hz: f32) -> MapKey {
    (
        width,
        (axis.low_hz.to_bits(), axis.high_hz.to_bits()),
        axis.scale == FreqScale::Linear,
        (bins as u32, bin_hz.to_bits()),
    )
}

impl SpectrumView {
    fn prepare(&mut self, width: usize, axis: FreqAxis, bins: usize, bin_hz: f32, slope: f32) {
        let key = (map_key(width, axis, bins, bin_hz), slope.to_bits());
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

    /// The index in `source_maps` of the mapping for a source's bins, built if needed.
    fn source_map(&mut self, width: usize, axis: FreqAxis, bins: usize, bin_hz: f32) -> usize {
        let key = map_key(width, axis, bins, bin_hz);
        if let Some(index) = self
            .source_maps
            .iter()
            .position(|(cached, _)| *cached == key)
        {
            return index;
        }
        if self.source_maps.len() >= SOURCE_MAPS {
            self.source_maps.remove(0);
        }
        let log = axis.scale != FreqScale::Linear;
        let map = BandMap::new(width, (axis.low_hz, axis.high_hz), log, bins, bin_hz);
        self.source_maps.push((key, map));
        self.source_maps.len() - 1
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

/// Draws the spectrum panel of `traces`. `compact` leaves out the legend details for the
/// overview.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SpectrumView,
    traces: &[Trace<'_>],
    (settings, sources): (&mut SpectrumSettings, &SourceList),
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
    let Some(first) = traces.first() else {
        message(&painter, plot, NO_SIGNALS);
        return;
    };
    let data = first.signal;
    let spectrum = &data.spectrum;
    if data.sample_rate == 0 || spectrum.primary_db.is_empty() {
        let mut header = Header::new(&painter, rect, "SPECTRUM");
        traces.iter().for_each(|trace| trace.legend(&mut header));
        message(&painter, plot, first.waiting_text());
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
    let (mut primary_color, secondary_color) = curve_colors(settings.channels, style);
    if first.bus.is_some() {
        // A bus drawn in full keeps its own color for the first curve.
        primary_color = first.color;
    }
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

    let others = other_curves(data, &traces[1..], sources.match_level);
    draw_others(view, data, (&others, width, axis), &clipped, (plot, &y_of));
    if sources.masking && !others.is_empty() {
        shade_masking(&clipped, plot, &view.levels);
    }

    let (primary_label, secondary_label) = curve_labels(settings.channels);
    let mut header = Header::new(&painter, rect, "SPECTRUM");
    if traces.len() > 1 || first.bus.is_some() {
        first.legend(&mut header);
    }
    header.legend(primary_label, primary_color);
    if let Some(label) = secondary_label {
        header.legend(label, secondary_color);
    }
    for (trace, _) in &others {
        trace.legend(&mut header);
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
        for (slot, (trace, _)) in others.iter().enumerate() {
            let level = view.levels[1 + slot][column];
            lines.push((
                format!("{:<5} {}", trace.name, format_db(level, "dB")),
                trace.color,
            ));
        }
        tooltip(&painter, pointer, plot, &lines);
    }
}

/// The signals after the first that have a spectrum, with their level shift in dB:
/// the loudness difference to the first with `match_level`.
fn other_curves<'t>(
    first: &SignalReading,
    traces: &'t [Trace<'t>],
    match_level: bool,
) -> Vec<(&'t Trace<'t>, f32)> {
    traces
        .iter()
        .filter(|trace| !trace.signal.spectrum.primary_db.is_empty())
        .map(|trace| {
            let offset = if match_level {
                match_offset_db(&first.loudness, &trace.signal.loudness)
            } else {
                0.0
            };
            (trace, offset)
        })
        .collect()
}

/// Fills `view.levels` with the first signal's primary curve and the `curves` of the
/// others, and draws the others' curves.
fn draw_others(
    view: &mut SpectrumView,
    first: &SignalReading,
    (curves, width, axis): (&[(&Trace<'_>, f32)], usize, FreqAxis),
    painter: &Painter,
    (plot, y_of): (Rect, &dyn Fn(f32) -> f32),
) {
    view.levels.resize_with(1 + curves.len(), Vec::new);
    let primary = &first.spectrum.primary_db;
    let input = &mut view.levels[0];
    input.clear();
    input.extend(
        view.columns
            .bands
            .iter()
            .zip(&view.tilt)
            .map(|(band, tilt)| shown(sample_db(primary, *band), *tilt)),
    );
    let step = plot.width() / width as f32;
    for (slot, (trace, offset)) in curves.iter().enumerate() {
        let spectrum = &trace.signal.spectrum;
        let map = view.source_map(width, axis, spectrum.primary_db.len(), spectrum.bin_hz);
        let bands = &view.source_maps[map].1.bands;
        let levels = &mut view.levels[1 + slot];
        levels.clear();
        levels.extend(
            bands
                .iter()
                .zip(&view.tilt)
                .map(|(band, tilt)| shown(sample_db(&spectrum.primary_db, *band), *tilt) + offset),
        );
        let points: Vec<Pos2> = levels
            .iter()
            .enumerate()
            .map(|(column, db)| pos2(plot.left() + (column as f32 + 0.5) * step, y_of(*db)))
            .collect();
        painter.add(Shape::line(points, stroke(1.4, trace.color)));
    }
}

/// Shades the columns where the two loudest curves are within [`MASKING_DB`] of each
/// other and above [`MASKING_FLOOR_DB`].
fn shade_masking(painter: &Painter, plot: Rect, levels: &[Vec<f32>]) {
    let width = levels.first().map_or(0, Vec::len);
    if width == 0 {
        return;
    }
    let step = plot.width() / width as f32;
    let masked = |column: usize| {
        let (mut loudest, mut second) = (f32::MIN, f32::MIN);
        for db in levels.iter().filter_map(|curve| curve.get(column)) {
            if *db > loudest {
                second = loudest;
                loudest = *db;
            } else if *db > second {
                second = *db;
            }
        }
        second >= MASKING_FLOOR_DB && loudest - second <= MASKING_DB
    };
    let mut start = None;
    for column in 0..=width {
        match (column < width && masked(column), start) {
            (true, None) => start = Some(column),
            (false, Some(first)) => {
                let x_range =
                    (plot.left() + first as f32 * step)..=(plot.left() + column as f32 * step);
                let span = Rect::from_x_y_ranges(x_range.clone(), plot.y_range());
                painter.rect_filled(span, 0.0, alpha(ALERT, 0.10));
                let strip = Rect::from_x_y_ranges(x_range, (plot.bottom() - 4.0)..=plot.bottom());
                painter.rect_filled(strip, 0.0, alpha(ALERT, 0.7));
                start = None;
            }
            _ => {}
        }
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
