//! The loudness meter: EBU R 128 readouts, bar meters and the loudness history. With
//! several shown signals, the readouts and bars are the first's, the history overlays
//! every signal's short-term loudness and a table compares them. PLATFORMS shows, in
//! place of the history, what each streaming platform's normalization does to them.

use aud_ui::theme::{
    ALERT, BORDER, METER_WARN, TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke,
};
use aud_ui::widgets::{button, panel};
use egui::{Align2, Color32, CornerRadius, Id, Painter, Pos2, Rect, Shape, Ui, pos2, vec2};

use crate::analysis::history_entries;
use crate::analysis::loudness::LoudnessReading;
use crate::platforms::{PLATFORMS, Playback, Verdict};
use crate::settings::{LoudnessScale, LoudnessSettings};
use crate::style::LoudnessStyle;
use crate::view::colormap::{Colors, paint, rgb};
use crate::view::plot::{Header, grid, message, plot_background, text_width};
use crate::view::sources::{NO_SIGNALS, Trace, name_traces};

/// Shown loudness range around the target in LU.
const BELOW_TARGET_LU: f32 = 36.0;
const ABOVE_TARGET_LU: f32 = 9.0;
/// Integrated loudness within this distance of the target counts as on target, in LU.
const TOLERANCE_LU: f32 = 1.0;
/// True peaks above this level are flagged, in dBTP.
const TRUE_PEAK_LIMIT_DB: f32 = -1.0;
/// Compact layout widths in logical pixels: the bars beside a history, the history's
/// level scale, and the narrowest history worth drawing.
const COMPACT_BARS_WIDTH: f32 = 84.0;
const HISTORY_SCALE_WIDTH: f32 = 30.0;
const MIN_HISTORY_WIDTH: f32 = 140.0;
/// Height of the compact readout strip and width of one of its cells.
const STRIP_HEIGHT: f32 = 44.0;
const STRIP_CELL_WIDTH: f32 = 84.0;
/// Row height of the comparison table in logical pixels.
const TABLE_ROW: f32 = 16.0;

/// Loudness text in the selected scale.
fn loudness_text(lufs: f32, settings: &LoudnessSettings) -> String {
    if !lufs.is_finite() || lufs < -90.0 {
        return String::from("–∞");
    }
    match settings.scale {
        LoudnessScale::Absolute => format!("{lufs:.1}"),
        LoudnessScale::Relative => format!("{:+.1}", lufs - settings.target_lufs()),
    }
}

fn unit(settings: &LoudnessSettings) -> &'static str {
    match settings.scale {
        LoudnessScale::Absolute => "LUFS",
        LoudnessScale::Relative => "LU",
    }
}

fn peak_text(db: f32) -> String {
    if !db.is_finite() || db < -150.0 {
        String::from("–∞")
    } else {
        format!("{db:.1}")
    }
}

/// Color of a loudness compared with the target.
fn target_color(lufs: f32, target: f32, style: &LoudnessStyle) -> Color32 {
    if !lufs.is_finite() {
        TEXT_DIM
    } else if lufs > target + TOLERANCE_LU {
        rgb(style.too_loud)
    } else if lufs < target - TOLERANCE_LU {
        rgb(style.too_quiet)
    } else {
        rgb(style.on_target)
    }
}

fn peak_color(db: f32) -> Color32 {
    if db > TRUE_PEAK_LIMIT_DB { ALERT } else { TEXT }
}

/// Minutes and seconds.
fn duration_text(seconds: f32) -> String {
    let total = seconds as u64;
    if total >= 3_600 {
        format!("{}:{:02}:{:02}", total / 3_600, total / 60 % 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

/// Draws the loudness panel of `traces`. Returns `true` when its RESET button was
/// clicked.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    traces: &[Trace<'_>],
    settings: &mut LoudnessSettings,
    (colors, style): (&Colors, &LoudnessStyle),
    compact: bool,
) -> bool {
    panel(ui, rect, "LOUDNESS", colors.accent);
    let painter = ui.painter_at(rect);
    let Some(first) = traces.first() else {
        message(&painter, rect, NO_SIGNALS);
        return false;
    };
    let overlay = traces.len() > 1;
    let reading = &first.signal.loudness;
    let target = settings.target_lufs();
    let duration = duration_text(reading.duration_s);
    let mut header = Header::new(&painter, rect, "LOUDNESS");
    let reset = header.right_slot(52.0, 18.0).is_some_and(|slot| {
        button(
            ui,
            Id::new("loudness-reset"),
            slot,
            "RESET",
            colors.accent,
            false,
        )
        .on_hover_text("Restart the integrated loudness, range, maxima and history")
        .clicked()
    });
    if let Some(slot) = header.right_slot(84.0, 18.0) {
        let response = button(
            ui,
            Id::new("loudness-platforms"),
            slot,
            "PLATFORMS",
            colors.accent,
            settings.platforms,
        )
        .on_hover_text("What streaming platforms' normalization does, in place of the history");
        if response.clicked() {
            settings.platforms = !settings.platforms;
        }
    }
    name_traces(&mut header, traces);
    header.right_text(
        &[
            format!(
                "target {target:.0} LUFS · {} · {duration}",
                settings.target.description()
            ),
            format!("target {target:.0} LUFS · {duration}"),
            format!("{target:.0} · {duration}"),
            duration.clone(),
        ],
        font(10.0),
        TEXT_DIM,
    );
    let content = Rect::from_min_max(rect.min + vec2(14.0, 36.0), rect.max - vec2(14.0, 12.0));
    if compact {
        // The diagrams come first: a strip of readouts on top, then the bars and, when it
        // fits, the history.
        let strip = Rect::from_min_size(content.min, vec2(content.width(), STRIP_HEIGHT));
        draw_readout_strip(&painter, strip, reading, (settings, style));
        let below = Rect::from_min_max(pos2(content.left(), strip.bottom() + 8.0), content.max);
        let with_history =
            below.width() >= COMPACT_BARS_WIDTH + HISTORY_SCALE_WIDTH + MIN_HISTORY_WIDTH;
        let bars_width = if with_history {
            COMPACT_BARS_WIDTH
        } else {
            below.width()
        };
        let bars = Rect::from_min_size(below.min, vec2(bars_width, below.height()));
        draw_bars(&painter, bars, reading, (settings, style));
        if with_history {
            let history = Rect::from_min_max(
                pos2(bars.right() + HISTORY_SCALE_WIDTH, below.top() + 4.0),
                pos2(below.right(), below.bottom() - 14.0),
            );
            if settings.platforms {
                draw_platforms(&painter, history, traces, (settings, style));
            } else {
                draw_history(&painter, history, traces, (settings, style));
            }
        }
        return reset;
    }
    let readouts = Rect::from_min_size(content.min, vec2(210.0, content.height()));
    draw_readouts(&painter, readouts, reading, (settings, style));
    let bars = Rect::from_min_size(
        pos2(readouts.right() + 16.0, content.top()),
        vec2(150.0, content.height()),
    );
    draw_bars(&painter, bars, reading, (settings, style));
    // Below the history: the true peaks, or with several signals the comparison table.
    let below = if overlay {
        (30.0 + TABLE_ROW * (traces.len() + 1) as f32).max(72.0)
    } else {
        72.0
    };
    let history = Rect::from_min_max(
        pos2(bars.right() + 44.0, content.top() + 18.0),
        pos2(content.right(), content.bottom() - below),
    );
    if settings.platforms {
        painter.text(
            pos2(history.left() - 34.0, content.top() + 6.0),
            Align2::LEFT_CENTER,
            "PLATFORMS",
            font(10.0),
            TEXT_DIM,
        );
        let table = Rect::from_min_max(pos2(history.left() - 34.0, history.top()), history.max);
        draw_platforms(&painter, table, traces, (settings, style));
    } else {
        draw_history_with_legend(&painter, (history, content), traces, (settings, style));
    }
    let peaks = Rect::from_min_max(
        pos2(history.left() - 34.0, history.bottom() + 26.0),
        pos2(content.right(), content.bottom()),
    );
    if overlay {
        draw_comparison(&painter, peaks, traces, (settings, style));
    } else {
        draw_true_peaks(&painter, peaks, reading, style);
    }
    reset
}

/// The history with its caption and legend above it.
fn draw_history_with_legend(
    painter: &Painter,
    (history, content): (Rect, Rect),
    traces: &[Trace<'_>],
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let overlay = traces.len() > 1;
    painter.text(
        pos2(history.left() - 34.0, content.top() + 6.0),
        Align2::LEFT_CENTER,
        "HISTORY",
        font(10.0),
        TEXT_DIM,
    );
    let mut x = history.left() + 60.0;
    let legend: Vec<(&str, Color32)> = if overlay {
        traces
            .iter()
            .map(|trace| (trace.name, trace.color))
            .collect()
    } else {
        vec![
            ("short-term", paint(style.short_term)),
            ("momentary", paint(style.momentary)),
        ]
    };
    for (label, color) in legend {
        painter.rect_filled(
            Rect::from_center_size(pos2(x, content.top() + 6.0), vec2(10.0, 3.0)),
            CornerRadius::same(1),
            color,
        );
        let end = painter.text(
            pos2(x + 9.0, content.top() + 6.0),
            Align2::LEFT_CENTER,
            label,
            font(10.0),
            TEXT,
        );
        x = end.right() + 16.0;
    }
    draw_history(painter, history, traces, (settings, style));
}

/// The color of what a platform does: on target, turned down (louder than needed), left
/// quieter or held by peak (quieter than the others), limited.
fn verdict_color(verdict: Verdict, style: &LoudnessStyle) -> Color32 {
    match verdict {
        Verdict::Unchanged => rgb(style.on_target),
        Verdict::TurnedDown => rgb(style.too_loud),
        Verdict::LeftQuiet | Verdict::HeldByPeak => rgb(style.too_quiet),
        Verdict::Limited => ALERT,
        Verdict::TurnedUp => TEXT,
    }
}

/// What each streaming platform does with the shown signals: its reference, and per
/// signal the gain, the loudness and true peak after it and a verdict. With several
/// signals, each gets a column with the gain and the resulting loudness.
fn draw_platforms(
    painter: &Painter,
    rect: Rect,
    traces: &[Trace<'_>],
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let rows = PLATFORMS.len() + 1;
    let footnote = 16.0;
    let row = ((rect.height() - footnote) / rows as f32).clamp(14.0, 24.0);
    let name_width = 104.0;
    let reference_width = 100.0;
    let first_value = rect.left() + name_width + reference_width;
    let single = traces.len() == 1;
    let columns: Vec<&str> = if single {
        vec!["GAIN", "LOUDNESS", "PEAK", ""]
    } else {
        traces.iter().map(|trace| trace.name).collect()
    };
    let cell = (rect.right() - first_value) / columns.len() as f32;
    let y_of = |index: usize| rect.top() + index as f32 * row;
    painter.text(
        pos2(rect.left(), y_of(0)),
        Align2::LEFT_TOP,
        "PLATFORM",
        font(10.0),
        TEXT_DIM,
    );
    painter.text(
        pos2(rect.left() + name_width, y_of(0)),
        Align2::LEFT_TOP,
        "REFERENCE",
        font(10.0),
        TEXT_DIM,
    );
    for (column, caption) in columns.iter().enumerate() {
        let color = if single {
            TEXT_DIM
        } else {
            traces[column].color
        };
        painter.text(
            pos2(first_value + column as f32 * cell, y_of(0)),
            Align2::LEFT_TOP,
            *caption,
            font(10.0),
            color,
        );
    }
    let measured: Vec<(f32, f32)> = traces
        .iter()
        .map(|trace| {
            let loudness = &trace.signal.loudness;
            (
                loudness.integrated_lufs,
                loudness.true_peak_max_db[0].max(loudness.true_peak_max_db[1]),
            )
        })
        .collect();
    let target = settings.target_lufs();
    for (index, platform) in PLATFORMS.iter().enumerate() {
        let y = y_of(index + 1);
        if index % 2 == 0 {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(rect.left() - 4.0, y - 2.0),
                    pos2(rect.right(), y + row - 2.0),
                ),
                CornerRadius::same(2),
                alpha(grid(painter), 0.25),
            );
        }
        painter.text(
            pos2(rect.left(), y),
            Align2::LEFT_TOP,
            platform.name,
            font(11.0),
            TEXT,
        );
        let reference = platform.reference_lufs.map_or_else(
            || String::from("none"),
            |lufs| format!("{lufs:.0} · {:.0} dBTP", platform.ceiling_dbtp),
        );
        painter.text(
            pos2(rect.left() + name_width, y),
            Align2::LEFT_TOP,
            reference,
            mono(11.0),
            TEXT_DIM,
        );
        for (column, (integrated, peak)) in measured.iter().enumerate() {
            let x = first_value + column as f32 * cell;
            let Some(playback) = platform.playback(*integrated, *peak) else {
                painter.text(pos2(x, y), Align2::LEFT_TOP, "–", mono(11.0), TEXT_DIM);
                continue;
            };
            let gain = format!("{:+.1} dB", playback.gain_db);
            let verdict = verdict_color(playback.verdict, style);
            if single {
                let cells = [
                    (gain, verdict),
                    (
                        loudness_text(playback.loudness_lufs, settings),
                        target_color(playback.loudness_lufs, target, style),
                    ),
                    (peak_text(playback.true_peak_db), peak_cell_color(&playback)),
                    (playback.verdict.text().to_owned(), verdict),
                ];
                for (offset, (text, color)) in cells.into_iter().enumerate() {
                    let font = if offset == 3 { font(11.0) } else { mono(11.0) };
                    painter.text(
                        pos2(x + offset as f32 * cell, y),
                        Align2::LEFT_TOP,
                        text,
                        font,
                        color,
                    );
                }
            } else {
                painter.text(
                    pos2(x, y),
                    Align2::LEFT_TOP,
                    format!(
                        "{gain} → {}",
                        loudness_text(playback.loudness_lufs, settings)
                    ),
                    mono(11.0),
                    verdict,
                );
            }
        }
    }
    painter.text(
        pos2(rect.left(), rect.bottom()),
        Align2::LEFT_BOTTOM,
        "From integrated loudness and true peak max · published and measured references, \
         which platforms change",
        font(9.0),
        TEXT_FAINT,
    );
}

fn peak_cell_color(playback: &Playback) -> Color32 {
    if playback.over_ceiling { ALERT } else { TEXT }
}

/// Every shown signal's loudness side by side, one row each.
fn draw_comparison(
    painter: &Painter,
    rect: Rect,
    traces: &[Trace<'_>],
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let target = settings.target_lufs();
    let name_width = (rect.width() * 0.3).clamp(60.0, 160.0);
    let columns = ["I", "S", "M", "LRA", "TP MAX"];
    let cell = (rect.width() - name_width) / columns.len() as f32;
    let x_of = |column: usize| rect.left() + name_width + column as f32 * cell;
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "SIGNAL",
        font(10.0),
        TEXT_DIM,
    );
    for (column, caption) in columns.iter().enumerate() {
        painter.text(
            pos2(x_of(column), rect.top()),
            Align2::LEFT_TOP,
            *caption,
            font(10.0),
            TEXT_DIM,
        );
    }
    for (row, trace) in traces.iter().enumerate() {
        let y = rect.top() + (row + 1) as f32 * TABLE_ROW + 2.0;
        if y + TABLE_ROW > rect.bottom() + 4.0 {
            break;
        }
        let reading = &trace.signal.loudness;
        painter.circle_filled(pos2(rect.left() + 4.0, y + 6.0), 3.5, trace.color);
        painter.text(
            pos2(rect.left() + 12.0, y),
            Align2::LEFT_TOP,
            trace.name,
            font(11.0),
            TEXT,
        );
        let true_peak = reading.true_peak_max_db[0].max(reading.true_peak_max_db[1]);
        let cells = [
            (
                loudness_text(reading.integrated_lufs, settings),
                target_color(reading.integrated_lufs, target, style),
            ),
            (loudness_text(reading.short_term_lufs, settings), TEXT),
            (loudness_text(reading.momentary_lufs, settings), TEXT),
            (format!("{:.1}", reading.range_lu), TEXT),
            (peak_text(true_peak), peak_color(true_peak)),
        ];
        for (column, (text, color)) in cells.into_iter().enumerate() {
            painter.text(
                pos2(x_of(column), y),
                Align2::LEFT_TOP,
                text,
                mono(11.0),
                color,
            );
        }
    }
}

fn draw_readouts(
    painter: &Painter,
    rect: Rect,
    reading: &LoudnessReading,
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let target = settings.target_lufs();
    let unit = unit(settings);
    let big = 40.0;
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "INTEGRATED",
        font(10.0),
        TEXT_DIM,
    );
    let value = painter.text(
        rect.left_top() + vec2(0.0, 14.0),
        Align2::LEFT_TOP,
        loudness_text(reading.integrated_lufs, settings),
        mono(big),
        target_color(reading.integrated_lufs, target, style),
    );
    painter.text(
        pos2(value.right() + 6.0, value.bottom() - 6.0),
        Align2::LEFT_BOTTOM,
        unit,
        font(12.0),
        TEXT_DIM,
    );
    let delta = if reading.integrated_lufs.is_finite() {
        let delta = reading.integrated_lufs - target;
        if delta.abs() <= TOLERANCE_LU {
            format!("on target ({delta:+.1} LU)")
        } else if delta > 0.0 {
            format!("{delta:.1} LU too loud")
        } else {
            format!("{:.1} LU too quiet", -delta)
        }
    } else {
        String::from("measuring…")
    };
    painter.text(
        pos2(rect.left(), value.bottom() + 4.0),
        Align2::LEFT_TOP,
        delta,
        font(11.0),
        target_color(reading.integrated_lufs, target, style),
    );
    let true_peak = reading.true_peak_max_db[0].max(reading.true_peak_max_db[1]);
    let plr = if true_peak.is_finite() && reading.integrated_lufs.is_finite() {
        format!("{:.1}", true_peak - reading.integrated_lufs)
    } else {
        String::from("–")
    };
    let cells: Vec<(&str, String, &str, Color32)> = vec![
        (
            "SHORT-TERM",
            loudness_text(reading.short_term_lufs, settings),
            unit,
            TEXT,
        ),
        (
            "MOMENTARY",
            loudness_text(reading.momentary_lufs, settings),
            unit,
            TEXT,
        ),
        (
            "SHORT-TERM MAX",
            loudness_text(reading.short_term_max_lufs, settings),
            unit,
            TEXT,
        ),
        (
            "MOMENTARY MAX",
            loudness_text(reading.momentary_max_lufs, settings),
            unit,
            TEXT,
        ),
        (
            "LOUDNESS RANGE",
            format!("{:.1}", reading.range_lu),
            "LU",
            TEXT,
        ),
        (
            "TRUE PEAK MAX",
            peak_text(true_peak),
            "dBTP",
            peak_color(true_peak),
        ),
        ("PEAK TO LOUDNESS", plr, "LU", TEXT),
        (
            "SAMPLE PEAK MAX",
            peak_text(reading.sample_peak_max_db[0].max(reading.sample_peak_max_db[1])),
            "dBFS",
            TEXT,
        ),
    ];
    let top = rect.top() + big + 46.0;
    let value_font = mono(20.0);
    // Two columns only if every cell fits half the width, so captions and values never
    // run into the next column.
    let widest = cells
        .iter()
        .map(|(caption, value, unit, _)| {
            let value = text_width(painter, value, &value_font)
                + 4.0
                + text_width(painter, unit, &font(10.0));
            text_width(painter, caption, &font(10.0)).max(value) + 12.0
        })
        .fold(0.0, f32::max);
    let columns = if 2.0 * widest <= rect.width() { 2 } else { 1 };
    let column_width = rect.width() / columns as f32;
    let row_height = 50.0;
    for (index, (caption, value, unit, color)) in cells.into_iter().enumerate() {
        let position = pos2(
            rect.left() + (index % columns) as f32 * column_width,
            top + (index / columns) as f32 * row_height,
        );
        if position.y + 30.0 > rect.bottom() {
            break;
        }
        painter.text(position, Align2::LEFT_TOP, caption, font(10.0), TEXT_DIM);
        let shown = painter.text(
            position + vec2(0.0, 14.0),
            Align2::LEFT_TOP,
            value,
            value_font.clone(),
            color,
        );
        painter.text(
            pos2(shown.right() + 4.0, shown.bottom() - 3.0),
            Align2::LEFT_BOTTOM,
            unit,
            font(10.0),
            TEXT_FAINT,
        );
    }
}

/// The compact readouts in one strip: integrated loudness and its distance from the target,
/// then as many of short-term, momentary, range and true peak as fit, in up to two rows.
fn draw_readout_strip(
    painter: &Painter,
    rect: Rect,
    reading: &LoudnessReading,
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let target = settings.target_lufs();
    let color = target_color(reading.integrated_lufs, target, style);
    let value = painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        loudness_text(reading.integrated_lufs, settings),
        mono(24.0),
        color,
    );
    painter.text(
        pos2(value.right() + 4.0, value.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        unit(settings),
        font(10.0),
        TEXT_DIM,
    );
    let delta = if reading.integrated_lufs.is_finite() {
        format!("I {:+.1} LU", reading.integrated_lufs - target)
    } else {
        String::from("measuring…")
    };
    painter.text(
        pos2(rect.left(), value.bottom() + 2.0),
        Align2::LEFT_TOP,
        delta,
        font(10.0),
        color,
    );
    let true_peak = reading.true_peak_max_db[0].max(reading.true_peak_max_db[1]);
    let cells = [
        ("S", loudness_text(reading.short_term_lufs, settings), TEXT),
        ("M", loudness_text(reading.momentary_lufs, settings), TEXT),
        ("LRA", format!("{:.1}", reading.range_lu), TEXT),
        ("TP", peak_text(true_peak), peak_color(true_peak)),
    ];
    let left = (rect.left() + 104.0).max(value.right() + 40.0);
    let columns = (((rect.right() - left) / STRIP_CELL_WIDTH).floor() as usize).min(2);
    for (index, (caption, text, color)) in cells.into_iter().enumerate().take(columns * 2) {
        let position = pos2(
            left + (index % columns) as f32 * STRIP_CELL_WIDTH,
            rect.top() + 4.0 + (index / columns) as f32 * 20.0,
        );
        let caption = painter.text(position, Align2::LEFT_TOP, caption, font(10.0), TEXT_DIM);
        painter.text(
            pos2(position.x + 30.0, caption.center().y),
            Align2::LEFT_CENTER,
            text,
            mono(12.0),
            color,
        );
    }
}

/// Vertical bars for momentary, short-term and integrated loudness against the target.
fn draw_bars(
    painter: &Painter,
    rect: Rect,
    reading: &LoudnessReading,
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let target = settings.target_lufs();
    let (floor, top) = (target - BELOW_TARGET_LU, target + ABOVE_TARGET_LU);
    let scale_width = 30.0;
    // The meters stand in a diagram with the shared background and grid.
    let diagram = Rect::from_min_max(
        pos2(rect.left() + scale_width, rect.top()),
        pos2(rect.right(), rect.bottom() - 16.0),
    );
    plot_background(painter, diagram);
    let bars = diagram.shrink2(vec2(8.0, 6.0));
    let y_of = |lufs: f32| {
        bars.bottom() - bars.height() * ((lufs - floor) / (top - floor)).clamp(0.0, 1.0)
    };
    painter.rect_filled(
        Rect::from_x_y_ranges(
            diagram.shrink(1.0).x_range(),
            y_of(target + TOLERANCE_LU)..=y_of(target - TOLERANCE_LU),
        ),
        CornerRadius::ZERO,
        alpha(rgb(style.on_target), 0.08),
    );
    let mut lu = -36;
    while lu <= 9 {
        let lufs = target + lu as f32;
        let y = y_of(lufs);
        let label = match settings.scale {
            LoudnessScale::Absolute => format!("{lufs:.0}"),
            LoudnessScale::Relative => format!("{lu:+}"),
        };
        painter.text(
            pos2(diagram.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            label,
            mono(9.0),
            TEXT_FAINT,
        );
        painter.hline(diagram.shrink(1.0).x_range(), y, stroke(1.0, grid(painter)));
        lu += 3;
    }
    let values = [
        (
            "M",
            reading.momentary_lufs,
            Some(reading.momentary_max_lufs),
        ),
        (
            "S",
            reading.short_term_lufs,
            Some(reading.short_term_max_lufs),
        ),
        ("I", reading.integrated_lufs, None),
    ];
    let gap = (bars.width() * 0.08).clamp(6.0, 16.0);
    let width = (bars.width() - gap * 2.0) / 3.0;
    let track = alpha(grid(painter), 0.45);
    for (index, (label, value, max)) in values.into_iter().enumerate() {
        let left = bars.left() + index as f32 * (width + gap);
        let bar = Rect::from_min_max(pos2(left, bars.top()), pos2(left + width, bars.bottom()));
        painter.rect_filled(bar, CornerRadius::same(3), track);
        if value.is_finite() {
            let color = target_color(value, target, style);
            let fill = Rect::from_min_max(pos2(bar.left(), y_of(value)), bar.right_bottom());
            painter.rect_filled(fill, CornerRadius::same(3), alpha(color, 0.85));
            // A brighter cap marks the level.
            painter.hline(fill.x_range(), fill.top() + 1.0, stroke(2.0, color));
        }
        if let Some(max) = max.filter(|max| max.is_finite()) {
            painter.hline(bar.x_range(), y_of(max), stroke(1.5, TEXT));
        }
        painter.text(
            pos2(bar.center().x, rect.bottom()),
            Align2::CENTER_BOTTOM,
            label,
            font(10.0),
            TEXT_DIM,
        );
    }
    let target_y = y_of(target);
    painter.add(Shape::dashed_line(
        &[
            pos2(diagram.left() + 2.0, target_y),
            pos2(diagram.right() - 2.0, target_y),
        ],
        stroke(1.5, alpha(TEXT, 0.9)),
        4.0,
        3.0,
    ));
}

/// The loudness history: momentary and short-term loudness of one signal, or the
/// short-term loudness of each of several.
fn draw_history(
    painter: &Painter,
    rect: Rect,
    traces: &[Trace<'_>],
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    plot_background(painter, rect);
    let target = settings.target_lufs();
    let (floor, top) = (target - 30.0, target + 12.0);
    let y_of = |lufs: f32| {
        let lufs = if lufs.is_finite() { lufs } else { floor };
        rect.bottom() - rect.height() * ((lufs - floor) / (top - floor)).clamp(0.0, 1.0)
    };
    painter.rect_filled(
        Rect::from_x_y_ranges(
            rect.x_range(),
            y_of(target + TOLERANCE_LU)..=y_of(target - TOLERANCE_LU),
        ),
        CornerRadius::ZERO,
        alpha(rgb(style.on_target), 0.08),
    );
    let mut lu = -30;
    while lu <= 12 {
        let lufs = target + lu as f32;
        let y = y_of(lufs);
        painter.hline(
            rect.x_range(),
            y,
            stroke(1.0, if lu == 0 { BORDER } else { grid(painter) }),
        );
        let label = match settings.scale {
            LoudnessScale::Absolute => format!("{lufs:.0}"),
            LoudnessScale::Relative => format!("{lu:+}"),
        };
        painter.text(
            pos2(rect.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            label,
            mono(9.0),
            TEXT_FAINT,
        );
        lu += 6;
    }
    painter.add(Shape::dashed_line(
        &[
            pos2(rect.left(), y_of(target)),
            pos2(rect.right(), y_of(target)),
        ],
        stroke(1.0, alpha(TEXT, 0.7)),
        5.0,
        4.0,
    ));
    let entries = history_entries(settings.span);
    let span_s = settings.span.seconds();
    let step_s = if span_s > 300.0 {
        60.0
    } else if span_s > 60.0 {
        20.0
    } else {
        5.0
    };
    let mut seconds = 0.0;
    while seconds <= span_s {
        let x = rect.right() - rect.width() * seconds / span_s;
        painter.vline(x, rect.y_range(), stroke(1.0, alpha(grid(painter), 0.6)));
        painter.text(
            pos2(x, rect.bottom() + 3.0),
            Align2::CENTER_TOP,
            if seconds == 0.0 {
                String::from("now")
            } else {
                format!("−{seconds:.0} s")
            },
            mono(9.0),
            TEXT_FAINT,
        );
        seconds += step_s;
    }
    let clipped = painter.with_clip_rect(rect.shrink(1.0));
    let overlay = traces.len() > 1;
    // Later signals under earlier ones, so the first stays on top.
    for trace in traces.iter().rev() {
        let history = &trace.signal.loudness_history;
        let shown = history.len().min(entries);
        let start = history.len() - shown;
        let x_of = |index: usize| {
            rect.right() - rect.width() * (shown - 1 - index) as f32 / entries.max(2) as f32
        };
        let lines = if overlay {
            vec![(1, trace.color, 1.6)]
        } else {
            vec![
                (0, paint(style.momentary), 1.0),
                (1, paint(style.short_term), 1.8),
            ]
        };
        for (side, color, width) in lines {
            let points: Vec<Pos2> = history
                .range(start..)
                .enumerate()
                .map(|(index, point)| pos2(x_of(index), y_of(point[side])))
                .collect();
            if points.len() > 1 {
                clipped.add(Shape::line(points, stroke(width, color)));
            }
        }
    }
}

fn draw_true_peaks(
    painter: &Painter,
    rect: Rect,
    reading: &LoudnessReading,
    style: &LoudnessStyle,
) {
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        "TRUE PEAK",
        font(10.0),
        TEXT_DIM,
    );
    let (floor, top) = (-60.0, 3.0);
    let bar_left = rect.left() + 20.0;
    let bar_right = rect.right() - 120.0;
    let x_of = |db: f32| {
        bar_left + (bar_right - bar_left) * ((db - floor) / (top - floor)).clamp(0.0, 1.0)
    };
    for (channel, label) in ["L", "R"].into_iter().enumerate() {
        let y = rect.top() + 22.0 + channel as f32 * 16.0;
        painter.text(
            pos2(rect.left(), y),
            Align2::LEFT_CENTER,
            label,
            font(10.0),
            TEXT_DIM,
        );
        let bar = Rect::from_min_max(pos2(bar_left, y - 4.0), pos2(bar_right, y + 4.0));
        plot_background(painter, bar);
        let level = reading.true_peak_db[channel];
        if level.is_finite() {
            let color = if level > TRUE_PEAK_LIMIT_DB {
                ALERT
            } else if level > -9.0 {
                METER_WARN
            } else {
                rgb(style.on_target)
            };
            painter.rect_filled(
                Rect::from_min_max(bar.min, pos2(x_of(level), bar.bottom())),
                CornerRadius::same(2),
                color,
            );
        }
        let max = reading.true_peak_max_db[channel];
        if max.is_finite() {
            painter.vline(
                x_of(max),
                bar.y_range().expand(2.0),
                stroke(1.5, peak_color(max)),
            );
        }
        painter.text(
            pos2(rect.right(), y),
            Align2::RIGHT_CENTER,
            format!("max {} dBTP", peak_text(max)),
            mono(11.0),
            peak_color(max),
        );
    }
    let limit = x_of(TRUE_PEAK_LIMIT_DB);
    painter.vline(
        limit,
        (rect.top() + 14.0)..=(rect.top() + 46.0),
        stroke(1.0, alpha(ALERT, 0.7)),
    );
    for db in [-48.0, -36.0, -24.0, -12.0, -6.0, 0.0] {
        painter.text(
            pos2(x_of(db), rect.top() + 50.0),
            Align2::CENTER_TOP,
            format!("{db:.0}"),
            mono(9.0),
            TEXT_FAINT,
        );
    }
}
