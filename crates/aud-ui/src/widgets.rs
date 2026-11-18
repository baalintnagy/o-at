//! Custom-painted controls and displays.

use std::f32::consts::PI;

use egui::text::{CCursor, CCursorRange};
use egui::{
    Align, Align2, Color32, Context, CornerRadius, CursorIcon, FontSelection, Id, Mesh, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, Vec2, pos2, vec2,
};

use crate::control::{Control, Switch};
use crate::theme::{
    BORDER, GRID, LOGO_WAVE, METER_CLIP, METER_WARN, PANEL, PANEL_RAISED, TEXT, TEXT_DIM,
    TEXT_FAINT, TRACK, alpha, font, mono, stroke,
};

/// Normalized change per pixel of vertical drag.
const DRAG_SPEED: f32 = 0.005;
/// Drag speed multiplier while Shift is held.
const FINE_FACTOR: f32 = 0.1;

const KNOB_START: f32 = 0.75 * PI;
const KNOB_SWEEP: f32 = 1.5 * PI;

/// Lowest level shown by meters, in dBFS.
pub const METER_FLOOR_DB: f32 = -60.0;

/// Panel background with an accent mark and a title.
pub fn panel(ui: &Ui, rect: Rect, title: &str, color: Color32) {
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(6),
        PANEL,
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
    painter.rect_filled(
        Rect::from_min_size(rect.min + vec2(12.0, 11.0), vec2(3.0, 12.0)),
        CornerRadius::same(1),
        color,
    );
    painter.text(
        rect.min + vec2(21.0, 17.0),
        Align2::LEFT_CENTER,
        title,
        font(12.0),
        TEXT,
    );
}

/// Diameter of the O-AT logo's ring, in logical pixels.
const LOGO_SIZE: f32 = 24.0;
/// Points of the wave across the logo's ring.
const LOGO_WAVE_POINTS: usize = 25;

/// The O-AT logo: a ring crossed by one period of a sine wave, followed by the `o-at`
/// wordmark and a divider. Drawn with its left edge at `left`, vertically centred on
/// `centre_y`; returns the logical width it takes, divider and spacing included.
pub fn logo(ui: &Ui, left: f32, centre_y: f32) -> f32 {
    let painter = ui.painter();
    let radius = LOGO_SIZE * 0.5;
    let centre = pos2(left + radius, centre_y);
    painter.circle_stroke(centre, radius - 1.0, stroke(2.0, TEXT));
    let half_width = radius - 4.0;
    let amplitude = radius * 0.38;
    let wave = (0..LOGO_WAVE_POINTS)
        .map(|index| {
            let phase = index as f32 / (LOGO_WAVE_POINTS - 1) as f32;
            pos2(
                centre.x - half_width + 2.0 * half_width * phase,
                centre.y - amplitude * (2.0 * PI * phase).sin(),
            )
        })
        .collect();
    painter.add(Shape::line(wave, stroke(2.0, LOGO_WAVE)));

    let wordmark = painter.layout_no_wrap(String::from("o-at"), font(17.0), TEXT);
    let wordmark_left = left + LOGO_SIZE + 6.0;
    let wordmark_width = wordmark.size().x;
    painter.galley(
        pos2(wordmark_left, centre_y - wordmark.size().y * 0.5),
        wordmark,
        TEXT,
    );
    let divider_x = wordmark_left + wordmark_width + 12.0;
    painter.vline(
        divider_x,
        (centre_y - 12.0)..=(centre_y + 12.0),
        stroke(1.0, BORDER),
    );
    divider_x + 12.0 - left
}

/// The O-AT logo, then a product title with a subtitle, left-aligned and vertically
/// centred in `rect`.
pub fn title(ui: &Ui, rect: Rect, name: &str, subtitle: &str, color: Color32) {
    let centre_y = rect.center().y;
    let left = rect.left() + logo(ui, rect.left(), centre_y);
    let painter = ui.painter();
    painter.rect_filled(
        Rect::from_center_size(pos2(left + 4.0, centre_y), vec2(8.0, 22.0)),
        CornerRadius::same(2),
        color,
    );
    let galley = painter.layout_no_wrap(name.to_owned(), font(24.0), TEXT);
    let width = galley.size().x;
    painter.galley(
        pos2(left + 16.0, centre_y - galley.size().y * 0.5),
        galley,
        TEXT,
    );
    painter.text(
        pos2(left + 28.0 + width, centre_y + 1.0),
        Align2::LEFT_CENTER,
        subtitle,
        font(12.0),
        TEXT_DIM,
    );
}

/// Rotary knob. `bipolar` draws the value arc from the centre, for gains.
pub fn knob(
    ui: &mut Ui,
    control: &dyn Control,
    rect: Rect,
    label: &str,
    color: Color32,
    bipolar: bool,
) -> Response {
    let id = Id::new(("knob", control.key()));
    let response = ui
        .interact(rect, id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::ResizeVertical);
    drive(ui, control, &response, id);

    let painter = ui.painter_at(rect.expand(4.0));
    let radius = (rect.width().min(rect.height() - 30.0) * 0.5 - 4.0).max(8.0);
    let centre = pos2(rect.center().x, rect.top() + radius + 4.0);
    let value = control.normalized();
    let active = response.hovered() || response.dragged();

    painter.add(arc(centre, radius, 0.0, 1.0, stroke(4.0, TRACK)));
    let (from, to) = if bipolar {
        (value.min(0.5), value.max(0.5))
    } else {
        (0.0, value)
    };
    if to - from > 0.002 {
        painter.add(arc(centre, radius, from, to, stroke(4.0, color)));
    }

    let body = radius - 7.0;
    painter.circle_filled(centre, body, if active { PANEL_RAISED } else { PANEL });
    painter.circle_stroke(centre, body, stroke(1.0, BORDER));
    let angle = KNOB_START + KNOB_SWEEP * value;
    let direction = vec2(angle.cos(), angle.sin());
    painter.line_segment(
        [
            centre + direction * body * 0.35,
            centre + direction * body * 0.85,
        ],
        stroke(2.5, if active { color } else { TEXT }),
    );

    let label_y = centre.y + radius + 9.0;
    painter.text(
        pos2(centre.x, label_y),
        Align2::CENTER_CENTER,
        label,
        font(11.0),
        TEXT_DIM,
    );
    painter.text(
        pos2(centre.x, label_y + 14.0),
        Align2::CENTER_CENTER,
        control.format(value),
        mono(11.0),
        if active { color } else { TEXT },
    );
    response
}

/// Handles drag, Shift-drag, wheel and double-click reset for a control.
pub fn drive(ui: &Ui, control: &dyn Control, response: &Response, id: Id) {
    if response.double_clicked() {
        control.begin_edit();
        control.set_normalized(control.default_normalized());
        control.end_edit();
        return;
    }
    let fine = ui.input(|input| input.modifiers.shift);
    let speed = if fine {
        DRAG_SPEED * FINE_FACTOR
    } else {
        DRAG_SPEED
    };
    if response.drag_started() {
        control.begin_edit();
        ui.memory_mut(|memory| memory.data.insert_temp(id, control.normalized()));
    }
    if response.dragged() {
        let start = ui
            .memory(|memory| memory.data.get_temp::<f32>(id))
            .unwrap_or_else(|| control.normalized());
        let value = (start - response.drag_delta().y * speed).clamp(0.0, 1.0);
        ui.memory_mut(|memory| memory.data.insert_temp(id, value));
        control.set_normalized(value);
    }
    if response.drag_stopped() {
        control.end_edit();
    }
    if response.hovered() && !response.dragged() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            let step = if fine { 0.002 } else { 0.01 };
            let value = control.normalized() + scroll.signum() * step;
            control.begin_edit();
            control.set_normalized(value.clamp(0.0, 1.0));
            control.end_edit();
        }
    }
}

/// Round power switch.
pub fn power_switch(ui: &mut Ui, switch: &dyn Switch, centre: Pos2, color: Color32) -> Response {
    let rect = Rect::from_center_size(centre, Vec2::splat(18.0));
    let response = ui
        .interact(rect, Id::new(("power", switch.key())), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        switch.set_on(!switch.is_on());
    }
    let on = switch.is_on();
    let painter = ui.painter();
    let ring = if on { color } else { TEXT_FAINT };
    painter.circle_filled(
        centre,
        8.5,
        if on { alpha(color, 0.18) } else { PANEL_RAISED },
    );
    painter.circle_stroke(centre, 8.5, stroke(1.0, ring));
    // Power glyph: an open ring with a vertical bar.
    painter.add(Shape::line(
        (0..=20)
            .map(|step| {
                let angle = -0.5 * PI + 0.35 * PI + (1.3 * PI) * step as f32 / 20.0;
                centre + vec2(angle.cos(), angle.sin()) * 4.5
            })
            .collect(),
        stroke(1.5, ring),
    ));
    painter.line_segment(
        [centre + vec2(0.0, -6.0), centre + vec2(0.0, -1.0)],
        stroke(1.5, ring),
    );
    response
}

/// Text button bound to a switch.
pub fn toggle_button(
    ui: &mut Ui,
    switch: &dyn Switch,
    rect: Rect,
    label: &str,
    color: Color32,
) -> Response {
    let response = button(
        ui,
        Id::new(("toggle", switch.key())),
        rect,
        label,
        color,
        switch.is_on(),
    );
    if response.clicked() {
        switch.set_on(!switch.is_on());
    }
    response
}

/// Text button; `lit` shows it in its active state.
pub fn button(ui: &mut Ui, id: Id, rect: Rect, label: &str, color: Color32, lit: bool) -> Response {
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();
    let fill = if lit {
        alpha(color, 0.22)
    } else if response.hovered() {
        PANEL_RAISED
    } else {
        PANEL
    };
    painter.rect(
        rect,
        CornerRadius::same(4),
        fill,
        stroke(1.0, if lit { color } else { BORDER }),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font(11.0),
        if lit || response.hovered() {
            color
        } else {
            TEXT_DIM
        },
    );
    response
}

/// Small status pill.
pub fn pill(ui: &Ui, rect: Rect, text: &str, color: Color32, lit: bool) {
    let painter = ui.painter();
    let (fill, outline, text_color) = if lit {
        (alpha(color, 0.2), color, color)
    } else {
        (PANEL, BORDER, TEXT_FAINT)
    };
    painter.rect(
        rect,
        CornerRadius::same(9),
        fill,
        stroke(1.0, outline),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font(10.0),
        text_color,
    );
}

/// Label and value above a horizontal bar. `fraction` is 0..1; `marker`, also 0..1,
/// draws a target tick.
pub fn bar_meter(
    ui: &Ui,
    rect: Rect,
    fraction: f32,
    marker: Option<f32>,
    color: Color32,
    label: &str,
    value: &str,
) {
    let painter = ui.painter();
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        label,
        font(10.0),
        TEXT_DIM,
    );
    painter.text(rect.right_top(), Align2::RIGHT_TOP, value, mono(10.0), TEXT);
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 6.0), rect.right_bottom());
    painter.rect_filled(bar, CornerRadius::same(3), TRACK);
    let filled = Rect::from_min_size(bar.min, vec2(bar.width() * fraction.clamp(0.0, 1.0), 6.0));
    painter.rect_filled(filled, CornerRadius::same(3), color);
    if let Some(marker) = marker {
        let x = bar.left() + bar.width() * marker.clamp(0.0, 1.0);
        painter.vline(x, bar.y_range().expand(3.0), stroke(1.5, TEXT));
    }
}

/// Label and value above a horizontal bar that fills from its centre, for gains that can
/// be positive or negative. `value` is shown relative to `range` in both directions.
pub fn bipolar_meter(
    ui: &Ui,
    rect: Rect,
    value: f32,
    range: f32,
    color: Color32,
    label: &str,
    text: &str,
) {
    let painter = ui.painter();
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        label,
        font(10.0),
        TEXT_DIM,
    );
    painter.text(rect.right_top(), Align2::RIGHT_TOP, text, mono(10.0), TEXT);
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 6.0), rect.right_bottom());
    painter.rect_filled(bar, CornerRadius::same(3), TRACK);
    let centre = bar.center().x;
    let fraction = if range > 0.0 {
        (value / range).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let end = centre + 0.5 * bar.width() * fraction;
    let filled = Rect::from_x_y_ranges(centre.min(end)..=centre.max(end), bar.y_range());
    painter.rect_filled(filled, CornerRadius::same(3), color);
    painter.vline(centre, bar.y_range().expand(3.0), stroke(1.0, TEXT_DIM));
}

/// Segmented selector. Returns the clicked segment if it differs from `selected`.
pub fn segmented(
    ui: &mut Ui,
    key: &str,
    rect: Rect,
    labels: &[&str],
    selected: usize,
    color: Color32,
) -> Option<usize> {
    let width = rect.width() / labels.len().max(1) as f32;
    ui.painter().rect(
        rect,
        CornerRadius::same(4),
        PANEL,
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
    let mut choice = None;
    for (index, label) in labels.iter().enumerate() {
        let segment = Rect::from_min_size(
            pos2(rect.left() + width * index as f32, rect.top()),
            vec2(width, rect.height()),
        );
        let response = ui
            .interact(segment, Id::new(("segment", key, index)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() && index != selected {
            choice = Some(index);
        }
        let is_selected = index == selected;
        if is_selected {
            ui.painter().rect(
                segment.shrink(2.0),
                CornerRadius::same(3),
                alpha(color, 0.22),
                stroke(1.0, color),
                StrokeKind::Inside,
            );
        }
        ui.painter().text(
            segment.center(),
            Align2::CENTER_CENTER,
            *label,
            font(11.0),
            if is_selected { color } else { TEXT_DIM },
        );
    }
    choice
}

/// Single vertical peak meter from the meter floor to 0 dBFS, with warning and clip
/// zones and a label below.
pub fn vertical_meter(ui: &Ui, rect: Rect, level_db: f32, color: Color32, label: &str) {
    let painter = ui.painter();
    let bar = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 14.0));
    painter.rect_filled(bar, CornerRadius::same(2), TRACK);
    let fraction = |db: f32| ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0);
    let top = bar.bottom() - bar.height() * fraction(level_db);
    let fill = if level_db > -1.0 {
        METER_CLIP
    } else if level_db > -9.0 {
        METER_WARN
    } else {
        color
    };
    painter.rect_filled(
        Rect::from_min_max(pos2(bar.left(), top), bar.right_bottom()),
        CornerRadius::same(2),
        fill,
    );
    for db in [-6.0, -18.0, -36.0] {
        let y = bar.bottom() - bar.height() * fraction(db);
        painter.hline(bar.x_range(), y, stroke(1.0, alpha(Color32::BLACK, 0.4)));
    }
    painter.text(
        pos2(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        label,
        font(10.0),
        TEXT_DIM,
    );
}

/// Labelled readout: a dim caption with a monospaced value below it.
pub fn readout(ui: &Ui, position: Pos2, caption: &str, value: &str, color: Color32) {
    let painter = ui.painter();
    painter.text(position, Align2::LEFT_TOP, caption, font(10.0), TEXT_DIM);
    painter.text(
        position + vec2(0.0, 14.0),
        Align2::LEFT_TOP,
        value,
        mono(13.0),
        color,
    );
}

/// Level meter ballistics, kept by the editor between frames.
#[derive(Clone, Copy, Debug)]
pub struct MeterState {
    level_db: f32,
    peak_db: f32,
    peak_age_s: f32,
}

impl Default for MeterState {
    fn default() -> Self {
        Self {
            level_db: METER_FLOOR_DB,
            peak_db: METER_FLOOR_DB,
            peak_age_s: 0.0,
        }
    }
}

impl MeterState {
    /// Release rate of the bar in dB per second.
    const RELEASE_DB_PER_S: f32 = 26.0;
    /// Peak hold time in seconds before the peak marker falls.
    const HOLD_S: f32 = 1.5;
    const PEAK_FALL_DB_PER_S: f32 = 12.0;

    /// Feeds the highest absolute sample since the previous frame, `elapsed_s` later.
    pub fn update(&mut self, peak: f32, elapsed_s: f32) {
        let input_db = gain_to_db(peak);
        self.level_db = input_db.max(self.level_db - Self::RELEASE_DB_PER_S * elapsed_s);
        if input_db >= self.peak_db {
            self.peak_db = input_db;
            self.peak_age_s = 0.0;
        } else {
            self.peak_age_s += elapsed_s;
            if self.peak_age_s > Self::HOLD_S {
                self.peak_db -= Self::PEAK_FALL_DB_PER_S * elapsed_s;
            }
        }
        self.peak_db = self.peak_db.max(self.level_db);
    }

    pub fn level_db(&self) -> f32 {
        self.level_db
    }

    pub fn peak_db(&self) -> f32 {
        self.peak_db
    }
}

/// Converts a linear gain to dBFS, limited to the meter floor.
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        METER_FLOOR_DB
    } else {
        (20.0 * gain.log10()).max(METER_FLOOR_DB)
    }
}

/// Vertical meters for several channels with a shared dB scale on the left.
pub fn channel_meters(ui: &Ui, rect: Rect, meters: &[MeterState], labels: &[&str], color: Color32) {
    let painter = ui.painter();
    let scale_width = 22.0;
    let label_height = 14.0;
    let bars = Rect::from_min_max(
        pos2(rect.left() + scale_width, rect.top()),
        pos2(rect.right(), rect.bottom() - label_height),
    );
    let fraction = |db: f32| ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0);
    let y_of = |db: f32| bars.bottom() - bars.height() * fraction(db);
    for db in [0.0, -6.0, -12.0, -24.0, -36.0, -48.0] {
        let y = y_of(db);
        painter.text(
            pos2(rect.left() + scale_width - 5.0, y),
            Align2::RIGHT_CENTER,
            format!("{}", -db as i32),
            mono(9.0),
            TEXT_FAINT,
        );
        painter.hline(bars.x_range(), y, stroke(1.0, GRID));
    }
    let count = meters.len().max(1) as f32;
    let gap = 4.0;
    let width = (bars.width() - gap * (count - 1.0)) / count;
    for (index, meter) in meters.iter().enumerate() {
        let left = bars.left() + index as f32 * (width + gap);
        let bar = Rect::from_min_max(pos2(left, bars.top()), pos2(left + width, bars.bottom()));
        painter.rect_filled(bar, CornerRadius::same(2), alpha(TRACK, 0.8));
        let level = meter.level_db();
        let top = y_of(level);
        let fill = if level > -1.0 {
            METER_CLIP
        } else if level > -9.0 {
            METER_WARN
        } else {
            color
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(bar.left(), top), bar.right_bottom()),
            CornerRadius::same(2),
            fill,
        );
        if meter.peak_db() > METER_FLOOR_DB {
            let peak_color = if meter.peak_db() > -1.0 {
                METER_CLIP
            } else {
                TEXT
            };
            painter.hline(
                bar.x_range(),
                y_of(meter.peak_db()),
                stroke(1.5, peak_color),
            );
        }
        if let Some(label) = labels.get(index) {
            painter.text(
                pos2(bar.center().x, rect.bottom()),
                Align2::CENTER_BOTTOM,
                *label,
                font(10.0),
                TEXT_DIM,
            );
        }
    }
}

/// Fixed-length history of values, recorded by the editor once per frame.
#[derive(Clone, Debug)]
pub struct History {
    values: Vec<f32>,
    head: usize,
}

impl History {
    pub fn new(len: usize, fill: f32) -> Self {
        Self {
            values: vec![fill; len.max(2)],
            head: 0,
        }
    }

    pub fn push(&mut self, value: f32) {
        self.values[self.head] = value;
        self.head = (self.head + 1) % self.values.len();
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Values from the oldest to the newest.
    pub fn chronological(&self) -> impl Iterator<Item = f32> + '_ {
        let (newer, older) = self.values.split_at(self.head);
        older.iter().chain(newer).copied()
    }
}

/// A horizontal reference line in a [`history_graph`].
pub struct GuideLine<'a> {
    pub value: f32,
    pub label: &'a str,
    pub color: Color32,
}

/// Scrolling line graph of `history` between `min` and `max`, newest on the right,
/// filled below the line.
pub fn history_graph(
    ui: &Ui,
    rect: Rect,
    history: &History,
    (min, max): (f32, f32),
    color: Color32,
    guides: &[GuideLine<'_>],
) {
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same(4),
        alpha(Color32::BLACK, 0.25),
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
    let inner = rect.shrink(1.0);
    let y_of = |value: f32| {
        let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
        inner.bottom() - inner.height() * fraction
    };
    let step = inner.width() / (history.len() - 1) as f32;
    let points: Vec<Pos2> = history
        .chronological()
        .enumerate()
        .map(|(index, value)| pos2(inner.left() + index as f32 * step, y_of(value)))
        .collect();
    // One mesh for the area below the line: separate polygons would show seams.
    let fill = alpha(color, 0.16);
    let mut area = Mesh::default();
    for (index, point) in points.iter().enumerate() {
        area.colored_vertex(*point, fill);
        area.colored_vertex(pos2(point.x, inner.bottom()), fill);
        if index > 0 {
            let top = 2 * index as u32;
            area.add_triangle(top - 2, top - 1, top);
            area.add_triangle(top - 1, top + 1, top);
        }
    }
    painter.add(Shape::mesh(area));
    painter.add(Shape::line(points, stroke(1.5, color)));
    for guide in guides {
        let y = y_of(guide.value);
        painter.add(Shape::dashed_line(
            &[pos2(inner.left(), y), pos2(inner.right(), y)],
            stroke(1.0, alpha(guide.color, 0.8)),
            5.0,
            4.0,
        ));
        painter.text(
            pos2(inner.left() + 6.0, y - 2.0),
            Align2::LEFT_BOTTOM,
            guide.label,
            font(10.0),
            guide.color,
        );
    }
}

/// Single-line text input in `rect`. Returns the response of the text edit; check
/// `lost_focus()` or Enter to commit. Focusing it selects the whole text, so typing
/// replaces it.
pub fn text_field(
    ui: &mut Ui,
    rect: Rect,
    key: &str,
    text: &mut String,
    hint: &str,
    color: Color32,
) -> Response {
    let id = Id::new(("text", key));
    let focused = ui.memory(|memory| memory.has_focus(id));
    ui.painter().rect(
        rect,
        CornerRadius::same(4),
        PANEL_RAISED,
        stroke(1.0, if focused { color } else { BORDER }),
        StrokeKind::Inside,
    );
    let edit = TextEdit::singleline(text)
        .id(id)
        .frame(false)
        .font(FontSelection::FontId(font(14.0)))
        .text_color(TEXT)
        .hint_text(hint)
        .margin(vec2(8.0, 4.0))
        .vertical_align(Align::Center);
    let response = ui.put(rect, edit);
    if response.gained_focus() {
        select_all(ui.ctx(), id, text.chars().count());
    }
    response
}

/// Selects the first `chars` characters, the whole text, of the text edit `id`.
pub fn select_all(context: &Context, id: Id, chars: usize) {
    let mut state = TextEdit::load_state(context, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(0),
        CCursor::new(chars),
    )));
    state.store(context, id);
}

/// A selectable list row with a status dot, a title and a right-aligned detail.
pub fn list_row(
    ui: &mut Ui,
    rect: Rect,
    key: &str,
    selected: bool,
    (title, detail): (&str, &str),
    dot: Option<Color32>,
    color: Color32,
) -> Response {
    let response = ui
        .interact(rect, Id::new(("row", key)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();
    let fill = if selected {
        alpha(color, 0.18)
    } else if response.hovered() {
        PANEL_RAISED
    } else {
        Color32::TRANSPARENT
    };
    painter.rect(
        rect,
        CornerRadius::same(4),
        fill,
        stroke(
            1.0,
            if selected {
                color
            } else {
                Color32::TRANSPARENT
            },
        ),
        StrokeKind::Inside,
    );
    let centre_y = rect.center().y;
    if let Some(dot) = dot {
        painter.circle_filled(pos2(rect.left() + 12.0, centre_y), 3.5, dot);
    }
    painter.text(
        pos2(rect.left() + 24.0, centre_y),
        Align2::LEFT_CENTER,
        title,
        font(12.0),
        if selected { color } else { TEXT },
    );
    painter.text(
        pos2(rect.right() - 10.0, centre_y),
        Align2::RIGHT_CENTER,
        detail,
        mono(10.0),
        if selected { TEXT } else { TEXT_FAINT },
    );
    response
}

/// Arc between two normalized knob positions.
fn arc(centre: Pos2, radius: f32, from: f32, to: f32, line: Stroke) -> Shape {
    let steps = ((to - from) * 48.0).ceil().max(2.0) as usize;
    let points = (0..=steps)
        .map(|step| {
            let position = from + (to - from) * step as f32 / steps as f32;
            let angle = KNOB_START + KNOB_SWEEP * position;
            centre + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    Shape::line(points, line)
}

#[cfg(test)]
mod tests {
    use super::{History, METER_FLOOR_DB, MeterState};

    #[test]
    fn history_is_chronological() {
        let mut history = History::new(3, 0.0);
        (1..=4).for_each(|value| history.push(value as f32));
        assert_eq!(history.chronological().collect::<Vec<_>>(), [2.0, 3.0, 4.0]);
    }

    #[test]
    fn meter_holds_peaks_and_releases() {
        let mut meter = MeterState::default();
        meter.update(1.0, 0.016);
        assert_eq!(meter.level_db(), 0.0);
        meter.update(0.0, 1.0);
        assert!(meter.level_db() < -20.0);
        assert_eq!(meter.peak_db(), 0.0, "peak is held");
        (0..200).for_each(|_| meter.update(0.0, 0.05));
        assert_eq!(meter.peak_db(), METER_FLOOR_DB.max(meter.level_db()));
    }
}
