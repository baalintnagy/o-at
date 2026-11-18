//! The oscilloscope: waveforms over a selectable time span, triggered on a level
//! crossing with sub-sample alignment, so periodic signals stand still.

use std::ops::Range;

use aud_ui::theme::{TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::{panel, pill};
use egui::{
    Align2, Color32, CursorIcon, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, Ui, pos2, vec2,
};

use crate::analysis::{ENVELOPE_BUCKET_S, ENVELOPE_LEN, EnvelopeBucket, Published};
use crate::settings::{
    ChannelMode, Choice, OscilloscopeSettings, ScopeLayout, Signal, TriggerMode,
};
use crate::style::ScopeStyle;
use crate::view::colormap::{Colors, paint};
use crate::view::plot::{Header, grid, grid_strong, plot_background, tooltip};

/// Share of the span shown before the trigger point.
const PRE_TRIGGER: f32 = 0.1;
/// Trigger hysteresis as a share of the screen's half height.
const HYSTERESIS: f32 = 0.02;
/// Highest automatic gain.
const MAX_GAIN: f32 = 1_000.0;
/// Automatic gain rise per frame, as a factor.
const GAIN_RISE: f32 = 1.03;
/// Horizontal and vertical grid divisions per lane.
const DIVISIONS_X: usize = 10;
const DIVISIONS_Y: usize = 8;

/// Value of `signal` in a stereo frame.
fn value(frame: [f32; 2], signal: Signal) -> f32 {
    let [left, right] = frame;
    match signal {
        Signal::Mid => 0.5 * (left + right),
        Signal::Side => 0.5 * (left - right),
        Signal::Left => left,
        Signal::Right => right,
    }
}

/// A trigger position and the period between the last two crossings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trigger {
    /// Fractional frame index of the crossing.
    pub position: f32,
    /// Frames between the last two crossings, if two were found.
    pub period: Option<f32>,
}

/// The latest crossing of `level` in `values[first..=last]`, with hysteresis.
pub fn find_trigger(
    values: impl Iterator<Item = f32>,
    (first, last): (usize, usize),
    level: f32,
    hysteresis: f32,
    rising: bool,
) -> Option<Trigger> {
    let mut armed = false;
    let mut previous_value = 0.0;
    let mut found: Option<f32> = None;
    let mut before: Option<f32> = None;
    for (index, sample) in values.enumerate().take(last + 1) {
        let (sample, threshold) = if rising {
            (sample, level)
        } else {
            (-sample, -level)
        };
        if index >= first && armed && sample >= threshold && index > 0 {
            let step = sample - previous_value;
            let fraction = if step > 0.0 {
                ((threshold - previous_value) / step).clamp(0.0, 1.0)
            } else {
                0.0
            };
            before = found;
            found = Some(index as f32 - 1.0 + fraction);
            armed = false;
        }
        if sample < threshold - hysteresis {
            armed = true;
        }
        previous_value = sample;
    }
    found.map(|position| Trigger {
        position,
        period: before.map(|before| position - before),
    })
}

/// Gain and trigger state kept between frames.
pub struct OscilloscopeView {
    gain: f32,
    last_sequence: u64,
    trigger: Option<Trigger>,
    /// Display start as a fractional index into the published frames.
    start: f32,
    points: Vec<Pos2>,
    /// Envelope buckets for long spans, as a ring; `head` is the next to write.
    envelope: Vec<EnvelopeBucket>,
    head: usize,
    /// Buckets written so far, up to the ring's length.
    filled: usize,
    envelope_generation: u64,
}

impl Default for OscilloscopeView {
    fn default() -> Self {
        Self {
            gain: 1.0,
            last_sequence: u64::MAX,
            trigger: None,
            start: 0.0,
            points: Vec::new(),
            envelope: vec![[[0.0; 2]; 4]; ENVELOPE_LEN],
            head: 0,
            filled: 0,
            envelope_generation: 0,
        }
    }
}

impl OscilloscopeView {
    /// Takes the new envelope buckets out of `data`.
    pub fn take_envelope(&mut self, data: &mut Published) {
        if data.envelope_generation != self.envelope_generation {
            self.envelope_generation = data.envelope_generation;
            self.head = 0;
            self.filled = 0;
        }
        for bucket in data.envelope.drain(..) {
            self.envelope[self.head] = bucket;
            self.head = (self.head + 1) % ENVELOPE_LEN;
            self.filled = (self.filled + 1).min(ENVELOPE_LEN);
        }
    }

    /// The bucket `age` buckets before the newest, if recorded.
    fn bucket(&self, age: usize) -> Option<&EnvelopeBucket> {
        (age < self.filled)
            .then(|| &self.envelope[(self.head + ENVELOPE_LEN - 1 - age) % ENVELOPE_LEN])
    }

    /// Minimum and maximum of `signal` over the buckets `ages` (newest is age 0).
    fn extent(&self, signal: Signal, ages: Range<usize>) -> Option<(f32, f32)> {
        let index = signal.index();
        ages.filter_map(|age| self.bucket(age))
            .map(|bucket| (bucket[index][0], bucket[index][1]))
            .reduce(|(low, high), (min, max)| (low.min(min), high.max(max)))
    }
}

fn trace_colors(mode: ChannelMode, style: &ScopeStyle) -> [(Signal, &'static str, Color32); 2] {
    let (primary, secondary) = mode.signals();
    let secondary_color = paint(if mode == ChannelMode::MidSide {
        style.side
    } else {
        style.right
    });
    let label = |signal: Signal| match signal {
        Signal::Mid => "MID",
        Signal::Side => "SIDE",
        Signal::Left => "LEFT",
        Signal::Right => "RIGHT",
    };
    [
        (primary, label(primary), paint(style.first)),
        (
            secondary.unwrap_or(primary),
            secondary.map_or("", label),
            secondary_color,
        ),
    ]
}

/// Draws the oscilloscope panel.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut OscilloscopeView,
    data: &Published,
    settings: &mut OscilloscopeSettings,
    (colors, style): (&Colors, &ScopeStyle),
    frozen: bool,
) {
    panel(ui, rect, "OSCILLOSCOPE", colors.accent);
    let plot = Rect::from_min_max(
        pos2(rect.left() + 28.0, rect.top() + 34.0),
        pos2(rect.right() - 14.0, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    plot_background(&painter, plot);
    if settings.span.is_roll() {
        draw_roll(ui, (rect, plot), view, settings, style, frozen);
        return;
    }
    let frames = &data.scope;
    let rate = data.sample_rate as f32;
    if rate <= 0.0 || frames.len() < 16 {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Waiting for audio",
            font(12.0),
            TEXT_DIM,
        );
        return;
    }
    let len = frames.len();
    let wanted = (settings.span.seconds() * rate).round() as usize;
    let span = wanted.clamp(8, len - 2);
    let pre = (span as f32 * PRE_TRIGGER) as usize;
    let traces = trace_colors(settings.channels, style);
    let shown = if settings.channels.signals().1.is_some() {
        2
    } else {
        1
    };

    if !frozen && data.sequence != view.last_sequence {
        view.last_sequence = data.sequence;
        let window = &frames[len.saturating_sub(2 * span)..];
        let peak = window
            .iter()
            .flat_map(|frame| {
                traces[..shown]
                    .iter()
                    .map(|(signal, _, _)| value(*frame, *signal).abs())
            })
            .fold(0.0, f32::max);
        view.gain = match settings.zoom.factor() {
            Some(factor) => factor,
            None => {
                let target = if peak > 1e-6 {
                    (0.9 / peak).clamp(1.0, MAX_GAIN)
                } else {
                    view.gain
                };
                if target < view.gain {
                    target
                } else {
                    (view.gain * GAIN_RISE).min(target)
                }
            }
        };
        view.trigger = match settings.trigger {
            TriggerMode::Free => None,
            mode => find_trigger(
                frames.iter().map(|frame| value(*frame, settings.source)),
                (pre + 1, len - 1 - (span - pre)),
                settings.level / view.gain,
                HYSTERESIS / view.gain,
                mode == TriggerMode::Rising,
            ),
        };
        view.start = view.trigger.map_or((len - 1 - span) as f32, |trigger| {
            trigger.position - pre as f32
        });
    }

    let lanes = lanes(plot, settings.layout, shown);
    let gain = view.gain;
    for lane in &lanes {
        draw_grid(&painter, *lane, gain);
    }
    let x_of = |index: f32| plot.left() + plot.width() * (index - view.start) / span as f32;
    let trigger_x = x_of(view.start + pre as f32);
    if settings.trigger != TriggerMode::Free {
        painter.add(Shape::dashed_line(
            &[pos2(trigger_x, plot.top()), pos2(trigger_x, plot.bottom())],
            stroke(1.0, alpha(TEXT, 0.35)),
            4.0,
            4.0,
        ));
    }

    let clipped = painter.with_clip_rect(plot.shrink(1.0));
    for (trace, (signal, _, color)) in traces[..shown].iter().enumerate().rev() {
        let lane = lanes[trace.min(lanes.len() - 1)];
        let y_of = |sample: f32| lane.center().y - sample * gain * lane.height() * 0.5;
        let first = view.start.floor().max(0.0) as usize;
        let last = ((view.start + span as f32).ceil() as usize + 1).min(len - 1);
        let width = plot.width().max(1.0);
        if (last - first) as f32 > 2.0 * width {
            // Dense: the extent of each pixel column.
            let mut mesh = Mesh::default();
            let per_column = (last - first) as f32 / width;
            let mut previous: Option<(f32, f32)> = None;
            for column in 0..width as usize {
                let from = first + (column as f32 * per_column) as usize;
                let to = (first + ((column + 1) as f32 * per_column) as usize).min(last);
                let (mut low, mut high) = frames[from..=to]
                    .iter()
                    .map(|frame| value(*frame, *signal))
                    .fold((f32::MAX, f32::MIN), |(low, high), v| {
                        (low.min(v), high.max(v))
                    });
                if let Some((previous_low, previous_high)) = previous {
                    low = low.min(previous_high);
                    high = high.max(previous_low);
                }
                previous = Some((low, high));
                let x = plot.left() + column as f32;
                let top = y_of(high);
                let bottom = y_of(low).max(top + 1.0);
                mesh.add_colored_rect(
                    Rect::from_min_max(pos2(x, top), pos2(x + 1.0, bottom)),
                    *color,
                );
            }
            clipped.add(Shape::mesh(mesh));
        } else {
            view.points.clear();
            view.points.extend(
                (first..=last)
                    .map(|index| pos2(x_of(index as f32), y_of(value(frames[index], *signal)))),
            );
            let sparse = (last - first) as f32 * 6.0 < width;
            if sparse {
                for point in &view.points {
                    clipped.circle_filled(*point, 2.0, *color);
                }
            }
            clipped.add(Shape::line(view.points.clone(), stroke(1.4, *color)));
        }
    }

    // Trigger level marker, draggable on the left edge.
    if settings.trigger != TriggerMode::Free {
        let lane = lanes[0];
        let y = lane.center().y - settings.level * lane.height() * 0.5;
        clipped.hline(plot.x_range(), y, stroke(1.0, alpha(TEXT, 0.25)));
        let tip = pos2(plot.left() - 2.0, y);
        let marker = paint(style.trigger);
        painter.add(Shape::convex_polygon(
            vec![tip, tip + vec2(-9.0, -6.0), tip + vec2(-9.0, 6.0)],
            marker,
            stroke(0.0, marker),
        ));
        painter.text(
            tip + vec2(-12.0, 0.0),
            Align2::RIGHT_CENTER,
            "T",
            font(10.0),
            TEXT_DIM,
        );
        let handle = Rect::from_min_max(
            pos2(plot.left() - 24.0, lane.top()),
            pos2(plot.left() + 10.0, lane.bottom()),
        );
        let response = ui
            .interact(
                handle,
                Id::new("oscilloscope-level"),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::ResizeVertical)
            .on_hover_text("Drag to set the trigger level; double-click to centre it");
        if response.double_clicked() {
            settings.level = 0.0;
        } else if let Some(pointer) = response
            .interact_pointer_pos()
            .filter(|_| response.dragged())
        {
            settings.level =
                ((lane.center().y - pointer.y) / (lane.height() * 0.5)).clamp(-1.0, 1.0);
        }
    }

    // Header: trigger state, legend and measurements, in that priority.
    let mut header = Header::new(&painter, rect, "OSCILLOSCOPE");
    let (state, color, lit) = match (settings.trigger, view.trigger) {
        (TriggerMode::Free, _) => ("FREE RUN", TEXT_DIM, false),
        (_, Some(_)) => ("TRIGGERED", colors.accent, true),
        (_, None) => ("AUTO", TEXT, true),
    };
    if let Some(slot) = header.right_slot(86.0, 18.0) {
        pill(ui, slot, state, color, lit);
    }
    for (_, label, color) in &traces[..shown] {
        header.legend(label, *color);
    }
    let per_division = format!(
        "{} /div",
        format_ms(1_000.0 * span as f32 / rate / DIVISIONS_X as f32)
    );
    let scale = format!(
        "±{} · {:+.0} dB",
        format_amplitude(1.0 / gain),
        20.0 * gain.log10()
    );
    let frequency = view
        .trigger
        .and_then(|trigger| trigger.period)
        .map(|period| format!(" · ≈ {:.1} Hz", rate / period))
        .unwrap_or_default();
    let limited = if span < wanted {
        format!(
            " · span limited to {}",
            format_ms(1_000.0 * span as f32 / rate)
        )
    } else {
        String::new()
    };
    header.right_text(
        &[
            format!("{per_division} · {scale}{frequency}{limited}"),
            format!("{per_division} · {scale}{frequency}"),
            format!("{per_division}{frequency}"),
            per_division.clone(),
        ],
        mono(10.0),
        TEXT_DIM,
    );

    // Time labels relative to the trigger, every second division; the last one carries
    // the unit, and the outer ones stay inside the plot's width.
    let step_ms = 2_000.0 * span as f32 / rate / DIVISIONS_X as f32;
    let decimals = if step_ms >= 10.0 {
        0
    } else if step_ms >= 1.0 {
        1
    } else {
        2
    };
    for division in (0..=DIVISIONS_X).step_by(2) {
        let fraction = division as f32 / DIVISIONS_X as f32;
        let ms = 1_000.0 * (fraction - PRE_TRIGGER) * span as f32 / rate;
        let mut text = if ms.abs() < 1e-3 {
            String::from("0")
        } else {
            format!("{ms:+.decimals$}")
        };
        let align = if division == 0 {
            Align2::LEFT_TOP
        } else if division == DIVISIONS_X {
            text.push_str(" ms");
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        painter.text(
            pos2(plot.left() + plot.width() * fraction, plot.bottom() + 3.0),
            align,
            text,
            mono(9.0),
            TEXT_FAINT,
        );
    }

    let response = ui.interact(plot, Id::new("oscilloscope-plot"), Sense::hover());
    if let Some(pointer) = response.hover_pos() {
        let index = view.start + (pointer.x - plot.left()) / plot.width() * span as f32;
        let sample = (index.round() as usize).min(len - 1);
        let ms = 1_000.0 * (index - view.start - pre as f32) / rate;
        clipped.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.3)));
        let mut lines = vec![(format!("{ms:+.3} ms"), TEXT)];
        for (signal, label, color) in &traces[..shown] {
            let v = value(frames[sample], *signal);
            let db = if v.abs() > 1e-9 {
                format!("{:.1} dBFS", 20.0 * v.abs().log10())
            } else {
                String::from("–∞")
            };
            lines.push((format!("{label:<5} {v:+.4}  {db}"), *color));
        }
        tooltip(&painter, pointer, plot, &lines);
    }
}

/// The traces' lanes in `plot`: one when overlaid, else one per trace.
fn lanes(plot: Rect, layout: ScopeLayout, shown: usize) -> Vec<Rect> {
    match layout {
        ScopeLayout::Stacked if shown == 2 => {
            let half = plot.height() * 0.5;
            vec![
                Rect::from_min_size(plot.min, vec2(plot.width(), half)),
                Rect::from_min_size(plot.min + vec2(0.0, half), vec2(plot.width(), half)),
            ]
        }
        _ => vec![plot],
    }
}

/// Long spans: the envelope history scrolling from right to left, newest on the right,
/// without a trigger.
fn draw_roll(
    ui: &mut Ui,
    (rect, plot): (Rect, Rect),
    view: &mut OscilloscopeView,
    settings: &OscilloscopeSettings,
    style: &ScopeStyle,
    frozen: bool,
) {
    let painter = ui.painter_at(rect);
    let span_s = settings.span.seconds();
    let buckets = (span_s / ENVELOPE_BUCKET_S).round() as usize;
    let traces = trace_colors(settings.channels, style);
    let shown = if settings.channels.signals().1.is_some() {
        2
    } else {
        1
    };
    if !frozen {
        let peak = traces[..shown]
            .iter()
            .filter_map(|(signal, _, _)| view.extent(*signal, 0..buckets))
            .map(|(min, max)| min.abs().max(max.abs()))
            .fold(0.0, f32::max);
        view.gain = match settings.zoom.factor() {
            Some(factor) => factor,
            None => {
                let target = if peak > 1e-6 {
                    (0.9 / peak).clamp(1.0, MAX_GAIN)
                } else {
                    view.gain
                };
                if target < view.gain {
                    target
                } else {
                    (view.gain * GAIN_RISE).min(target)
                }
            }
        };
    }
    let gain = view.gain;
    let lanes = lanes(plot, settings.layout, shown);
    for lane in &lanes {
        draw_grid(&painter, *lane, gain);
    }
    let clipped = painter.with_clip_rect(plot.shrink(1.0));
    let width = plot.width().max(1.0) as usize;
    let per_pixel = buckets as f32 / width as f32;
    for (trace, (signal, _, color)) in traces[..shown].iter().enumerate().rev() {
        let lane = lanes[trace.min(lanes.len() - 1)];
        let y_of = |sample: f32| lane.center().y - sample * gain * lane.height() * 0.5;
        let mut mesh = Mesh::default();
        for pixel in 0..width {
            // Pixel 0 is the oldest; ages count back from the newest bucket.
            let newest = ((width - 1 - pixel) as f32 * per_pixel) as usize;
            let oldest = (((width - pixel) as f32 * per_pixel) as usize).max(newest + 1);
            let Some((min, max)) = view.extent(*signal, newest..oldest) else {
                continue;
            };
            let x = plot.left() + pixel as f32;
            let top = y_of(max);
            let bottom = y_of(min).max(top + 1.0);
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, top), pos2(x + 1.0, bottom)),
                *color,
            );
        }
        clipped.add(Shape::mesh(mesh));
    }

    let mut header = Header::new(&painter, rect, "OSCILLOSCOPE");
    if let Some(slot) = header.right_slot(86.0, 18.0) {
        pill(ui, slot, "ROLL", TEXT, true);
    }
    for (_, label, color) in &traces[..shown] {
        header.legend(label, *color);
    }
    let per_division = format!("{} /div", format_seconds(span_s / DIVISIONS_X as f32));
    header.right_text(
        &[
            format!(
                "{per_division} · ±{} · {:+.0} dB",
                format_amplitude(1.0 / gain),
                20.0 * gain.log10()
            ),
            per_division.clone(),
        ],
        mono(10.0),
        TEXT_DIM,
    );
    for division in (0..=DIVISIONS_X).step_by(2) {
        let fraction = division as f32 / DIVISIONS_X as f32;
        let ago = (1.0 - fraction) * span_s;
        let (text, align) = if division == DIVISIONS_X {
            (String::from("now"), Align2::RIGHT_TOP)
        } else if division == 0 {
            (format!("−{} ", format_seconds(ago)), Align2::LEFT_TOP)
        } else {
            (format!("−{}", format_seconds(ago)), Align2::CENTER_TOP)
        };
        painter.text(
            pos2(plot.left() + plot.width() * fraction, plot.bottom() + 3.0),
            align,
            text,
            mono(9.0),
            TEXT_FAINT,
        );
    }
    if view.filled == 0 {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Waiting for audio",
            font(12.0),
            TEXT_DIM,
        );
    }

    let response = ui.interact(plot, Id::new("oscilloscope-roll"), Sense::hover());
    if let Some(pointer) = response.hover_pos() {
        let fraction = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        let age = ((1.0 - fraction) * buckets as f32) as usize;
        clipped.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.3)));
        let mut lines = vec![(format!("−{:.3} s", age as f32 * ENVELOPE_BUCKET_S), TEXT)];
        for (signal, label, color) in &traces[..shown] {
            let reach = (per_pixel.ceil() as usize).max(1);
            if let Some((min, max)) = view.extent(*signal, age..age + reach) {
                lines.push((format!("{label:<5} {min:+.3} … {max:+.3}"), *color));
            }
        }
        tooltip(&painter, pointer, plot, &lines);
    }
}

/// Seconds, compactly: `0.5 s`, `2 s`, `1 min`.
fn format_seconds(seconds: f32) -> String {
    if seconds >= 60.0 && (seconds % 60.0).abs() < 1e-3 {
        format!("{:.0} min", seconds / 60.0)
    } else if seconds >= 10.0 || (seconds.fract()).abs() < 1e-3 {
        format!("{seconds:.0} s")
    } else {
        format!("{seconds:.1} s")
    }
}

fn draw_grid(painter: &Painter, lane: Rect, gain: f32) {
    for division in 0..=DIVISIONS_X {
        let x = lane.left() + lane.width() * division as f32 / DIVISIONS_X as f32;
        let color = if division == DIVISIONS_X / 2 {
            grid_strong(painter)
        } else {
            grid(painter)
        };
        painter.vline(x, lane.y_range(), stroke(1.0, color));
    }
    for division in 0..=DIVISIONS_Y {
        let y = lane.top() + lane.height() * division as f32 / DIVISIONS_Y as f32;
        let color = if division == DIVISIONS_Y / 2 {
            grid_strong(painter)
        } else {
            grid(painter)
        };
        painter.hline(lane.x_range(), y, stroke(1.0, color));
        if division % 2 == 0 {
            // Inside the plot, above the line (below it for the top one), so the
            // labels never meet the trigger marker on the left edge.
            let amplitude = (1.0 - 2.0 * division as f32 / DIVISIONS_Y as f32) / gain;
            let (offset, align) = if division == 0 {
                (2.0, Align2::LEFT_TOP)
            } else {
                (-2.0, Align2::LEFT_BOTTOM)
            };
            painter.text(
                pos2(lane.left() + 4.0, y + offset),
                align,
                format_amplitude(amplitude),
                mono(9.0),
                TEXT_FAINT,
            );
        }
    }
}

/// Milliseconds, or microseconds below one.
fn format_ms(ms: f32) -> String {
    if ms < 1.0 {
        format!("{:.0} µs", ms * 1_000.0)
    } else {
        format!("{ms:.1} ms")
    }
}

/// A linear amplitude with enough digits for small values.
fn format_amplitude(amplitude: f32) -> String {
    let magnitude = amplitude.abs();
    if magnitude < 1e-9 {
        String::from("0")
    } else if magnitude >= 0.1 {
        format!("{amplitude:.2}")
    } else if magnitude >= 0.01 {
        format!("{amplitude:.3}")
    } else {
        format!("{amplitude:.1e}")
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::{OscilloscopeView, find_trigger, format_seconds};
    use crate::analysis::Published;
    use crate::settings::{Choice, Signal};

    #[test]
    fn roll_mode_reads_the_envelope_history() {
        let mut view = OscilloscopeView::default();
        let mut data = Published::default();
        // Buckets with the left channel rising from 0 to 0.99 in steps of 0.01.
        data.envelope.extend((0..100).map(|step| {
            let value = step as f32 / 100.0;
            let mut bucket = [[0.0; 2]; 4];
            bucket[Signal::Left.index()] = [-value, value];
            bucket
        }));
        view.take_envelope(&mut data);
        assert!(data.envelope.is_empty(), "the buckets are taken");
        assert_eq!(
            view.extent(Signal::Left, 0..1),
            Some((-0.99, 0.99)),
            "newest first"
        );
        assert_eq!(view.extent(Signal::Left, 50..100), Some((-0.49, 0.49)));
        assert_eq!(
            view.extent(Signal::Left, 100..200),
            None,
            "not recorded yet"
        );
        assert_eq!(format_seconds(0.5), "0.5 s");
        assert_eq!(format_seconds(6.0), "6 s");
        assert_eq!(format_seconds(60.0), "1 min");
    }

    #[test]
    fn triggers_on_the_latest_crossing_between_samples() {
        // 100 Hz at 48 kHz, starting at a quarter period: rising zero crossings at
        // 360, 840, … (480 frames apart).
        let samples: Vec<f32> = (0..2_000)
            .map(|n| (TAU * 100.0 * n as f32 / 48_000.0 + TAU * 0.25).sin())
            .collect();
        let trigger =
            find_trigger(samples.iter().copied(), (1, 1_500), 0.0, 0.02, true).expect("a crossing");
        assert!(
            (trigger.position - 1_320.0).abs() < 0.05,
            "at {}",
            trigger.position
        );
        assert!((trigger.period.expect("a period") - 480.0).abs() < 0.05);
        let falling = find_trigger(samples.iter().copied(), (1, 1_500), 0.0, 0.02, false)
            .expect("a falling crossing");
        assert!(
            (falling.position - 1_080.0).abs() < 0.05,
            "at {}",
            falling.position
        );
        assert!(find_trigger(samples.iter().copied(), (1, 1_500), 2.0, 0.02, true).is_none());
    }
}
