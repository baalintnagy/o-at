//! The loudness meter: EBU R 128 readouts, bar meters and the loudness history.

use aud_ui::theme::{
    ALERT, BORDER, METER_WARN, PANEL_RAISED, TEXT, TEXT_DIM, TEXT_FAINT, TRACK, alpha, font, mono,
    stroke,
};
use aud_ui::widgets::{button, panel};
use egui::{
    Align2, Color32, CornerRadius, Id, Painter, Pos2, Rect, Shape, StrokeKind, Ui, pos2, vec2,
};

use crate::analysis::loudness::LoudnessReading;
use crate::analysis::{Published, history_entries};
use crate::settings::{LoudnessScale, LoudnessSettings};
use crate::style::LoudnessStyle;
use crate::view::colormap::{Colors, paint, rgb};
use crate::view::plot::{Header, grid, plot_background, text_width};

/// Shown loudness range around the target in LU.
const BELOW_TARGET_LU: f32 = 36.0;
const ABOVE_TARGET_LU: f32 = 9.0;
/// Integrated loudness within this distance of the target counts as on target, in LU.
const TOLERANCE_LU: f32 = 1.0;
/// True peaks above this level are flagged, in dBTP.
const TRUE_PEAK_LIMIT_DB: f32 = -1.0;

/// Loudness text in the selected scale.
fn loudness_text(lufs: f32, settings: &LoudnessSettings) -> String {
    if !lufs.is_finite() || lufs < -90.0 {
        return String::from("–∞");
    }
    match settings.scale {
        LoudnessScale::Absolute => format!("{lufs:.1}"),
        LoudnessScale::Relative => format!("{:+.1}", lufs - settings.target.lufs()),
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

/// Draws the loudness panel. Returns `true` when its RESET button was clicked.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    data: &Published,
    settings: &mut LoudnessSettings,
    (colors, style): (&Colors, &LoudnessStyle),
    compact: bool,
) -> bool {
    panel(ui, rect, "LOUDNESS", colors.accent);
    let painter = ui.painter_at(rect);
    let reading = &data.loudness;
    let target = settings.target.lufs();
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
        let readouts = Rect::from_min_max(
            content.min,
            pos2(content.left() + content.width() * 0.55, content.bottom()),
        );
        draw_readouts(&painter, readouts, reading, (settings, style), true);
        let meters = Rect::from_min_max(pos2(readouts.right() + 8.0, content.top()), content.max);
        draw_bars(&painter, meters, reading, (settings, style));
        return reset;
    }
    let readouts = Rect::from_min_size(content.min, vec2(300.0, content.height()));
    draw_readouts(&painter, readouts, reading, (settings, style), false);
    let bars = Rect::from_min_size(
        pos2(readouts.right() + 16.0, content.top()),
        vec2(170.0, content.height()),
    );
    draw_bars(&painter, bars, reading, (settings, style));
    let history = Rect::from_min_max(
        pos2(bars.right() + 44.0, content.top() + 18.0),
        pos2(content.right(), content.bottom() - 72.0),
    );
    painter.text(
        pos2(history.left() - 34.0, content.top() + 6.0),
        Align2::LEFT_CENTER,
        "HISTORY",
        font(10.0),
        TEXT_DIM,
    );
    let mut x = history.left() + 60.0;
    for (label, color) in [
        ("short-term", paint(style.short_term)),
        ("momentary", paint(style.momentary)),
    ] {
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
    draw_history(&painter, history, data, (settings, style));
    let peaks = Rect::from_min_max(
        pos2(history.left() - 34.0, history.bottom() + 26.0),
        pos2(content.right(), content.bottom()),
    );
    draw_true_peaks(&painter, peaks, reading, style);
    reset
}

fn draw_readouts(
    painter: &Painter,
    rect: Rect,
    reading: &LoudnessReading,
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
    compact: bool,
) {
    let target = settings.target.lufs();
    let unit = unit(settings);
    let big = if compact { 30.0 } else { 46.0 };
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
    let cells: Vec<(&str, String, &str, Color32)> = if compact {
        vec![
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
            ("RANGE", format!("{:.1}", reading.range_lu), "LU", TEXT),
            (
                "TRUE PEAK",
                peak_text(true_peak),
                "dBTP",
                peak_color(true_peak),
            ),
        ]
    } else {
        vec![
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
        ]
    };
    let top = rect.top() + big + 46.0;
    let value_font = mono(if compact { 16.0 } else { 20.0 });
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
    let row_height = if compact { 40.0 } else { 50.0 };
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

/// Vertical bars for momentary, short-term and integrated loudness against the target.
fn draw_bars(
    painter: &Painter,
    rect: Rect,
    reading: &LoudnessReading,
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    let target = settings.target.lufs();
    let (floor, top) = (target - BELOW_TARGET_LU, target + ABOVE_TARGET_LU);
    let scale_width = 30.0;
    let bars = Rect::from_min_max(
        pos2(rect.left() + scale_width, rect.top() + 4.0),
        pos2(rect.right(), rect.bottom() - 16.0),
    );
    let y_of = |lufs: f32| {
        bars.bottom() - bars.height() * ((lufs - floor) / (top - floor)).clamp(0.0, 1.0)
    };
    let mut lu = -36;
    while lu <= 9 {
        let lufs = target + lu as f32;
        let y = y_of(lufs);
        let label = match settings.scale {
            LoudnessScale::Absolute => format!("{lufs:.0}"),
            LoudnessScale::Relative => format!("{lu:+}"),
        };
        painter.text(
            pos2(bars.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            label,
            mono(9.0),
            TEXT_FAINT,
        );
        painter.hline(bars.x_range(), y, stroke(1.0, grid(painter)));
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
    let gap = 8.0;
    let width = (bars.width() - gap * 2.0) / 3.0;
    for (index, (label, value, max)) in values.into_iter().enumerate() {
        let left = bars.left() + index as f32 * (width + gap);
        let bar = Rect::from_min_max(pos2(left, bars.top()), pos2(left + width, bars.bottom()));
        painter.rect_filled(bar, CornerRadius::same(2), alpha(TRACK, 0.8));
        if value.is_finite() {
            painter.rect_filled(
                Rect::from_min_max(pos2(bar.left(), y_of(value)), bar.right_bottom()),
                CornerRadius::same(2),
                target_color(value, target, style),
            );
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
            pos2(bars.left() - 2.0, target_y),
            pos2(bars.right() + 2.0, target_y),
        ],
        stroke(1.5, alpha(TEXT, 0.9)),
        4.0,
        3.0,
    ));
}

fn draw_history(
    painter: &Painter,
    rect: Rect,
    data: &Published,
    (settings, style): (&LoudnessSettings, &LoudnessStyle),
) {
    plot_background(painter, rect);
    let target = settings.target.lufs();
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
    let history = &data.loudness_history;
    let shown = history.len().min(entries);
    let start = history.len() - shown;
    let x_of = |index: usize| {
        rect.right() - rect.width() * (shown - 1 - index) as f32 / entries.max(2) as f32
    };
    let clipped = painter.with_clip_rect(rect.shrink(1.0));
    for (side, color, width) in [
        (0, paint(style.momentary), 1.0),
        (1, paint(style.short_term), 1.8),
    ] {
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
        painter.rect(
            bar,
            CornerRadius::same(2),
            PANEL_RAISED,
            stroke(1.0, BORDER),
            StrokeKind::Inside,
        );
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
