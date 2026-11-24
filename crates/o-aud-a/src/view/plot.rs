//! Plot helpers shared by the views: axes, grids, labels and readouts.

use aud_ui::theme::{BORDER, PANEL_RAISED, TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Id, Painter, Pos2, Rect, StrokeKind, pos2, vec2,
};

use crate::analysis::fft::{BandBins, FLOOR_DB};
use crate::settings::FreqScale;
use crate::view::SETTINGS_BUTTON_WIDTH;
use crate::view::colormap::Colors;

/// Where the editor keeps the frame's colors in the egui context.
const COLORS_ID: &str = "o-aud-a-colors";

/// Note names from C.
const NOTES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Frequencies with grid lines and the labelled ones, in Hz.
const GRID_HZ: [f32; 27] = [
    20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0,
    700.0, 800.0, 900.0, 1_000.0, 2_000.0, 3_000.0, 4_000.0, 5_000.0, 6_000.0, 7_000.0, 8_000.0,
    9_000.0, 10_000.0,
];
const LABEL_HZ: [f32; 10] = [
    20.0, 50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0,
];

/// A frequency axis across a horizontal or vertical extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreqAxis {
    /// Frequency range in Hz.
    pub low_hz: f32,
    pub high_hz: f32,
    pub scale: FreqScale,
}

/// Grid lines closer than this are left out, in logical pixels.
const MIN_GRID_SPACING: f32 = 3.0;

impl FreqAxis {
    fn log(&self) -> bool {
        self.scale != FreqScale::Linear
    }

    /// Position of `hz` from 0 at the low end to 1 at the high end.
    pub fn fraction(&self, hz: f32) -> f32 {
        if self.log() {
            (hz.max(1e-3) / self.low_hz).ln() / (self.high_hz / self.low_hz).ln()
        } else {
            (hz - self.low_hz) / (self.high_hz - self.low_hz)
        }
    }

    /// Frequency at `fraction` along the axis in Hz.
    pub fn hz(&self, fraction: f32) -> f32 {
        if self.log() {
            self.low_hz * (self.high_hz / self.low_hz).powf(fraction)
        } else {
            self.low_hz + (self.high_hz - self.low_hz) * fraction
        }
    }

    /// Grid frequencies in range for an axis `extent` pixels long, with whether each is
    /// labelled.
    pub fn ticks(&self, extent: f32) -> Vec<(f32, bool)> {
        match self.scale {
            FreqScale::Log => GRID_HZ
                .iter()
                .chain(&[20_000.0, 30_000.0, 40_000.0])
                .copied()
                .filter(|hz| *hz >= self.low_hz && *hz <= self.high_hz * 1.001)
                .map(|hz| (hz, LABEL_HZ.contains(&hz)))
                .collect(),
            FreqScale::Linear => {
                let step = if self.high_hz > 30_000.0 {
                    5_000.0
                } else {
                    2_000.0
                };
                (0..=(self.high_hz / step) as usize)
                    .map(|index| index as f32 * step)
                    .filter(|hz| *hz >= self.low_hz)
                    .map(|hz| (hz, ((hz / step) as usize).is_multiple_of(2)))
                    .collect()
            }
            FreqScale::Semitone => {
                // Every semitone where they are far enough apart, labelled at each C;
                // only the Cs when they are not.
                let octaves = (self.high_hz / self.low_hz).log2().max(1e-3);
                let semitones = extent / (octaves * 12.0) >= MIN_GRID_SPACING;
                let first = midi(self.low_hz).ceil() as i32;
                let last = midi(self.high_hz * 1.001).floor() as i32;
                (first..=last)
                    .filter(|note| semitones || note.rem_euclid(12) == 0)
                    .map(|note| (note_hz(note), note.rem_euclid(12) == 0))
                    .collect()
            }
        }
    }

    /// Label of a grid frequency.
    pub fn label(&self, hz: f32) -> String {
        match self.scale {
            FreqScale::Semitone => {
                let note = midi(hz).round() as i32;
                format!(
                    "{}{}",
                    NOTES[note.rem_euclid(12) as usize],
                    note.div_euclid(12) - 1
                )
            }
            _ => format_hz(hz),
        }
    }
}

/// MIDI note number of `hz`, with A4 at 440 Hz.
fn midi(hz: f32) -> f32 {
    69.0 + 12.0 * (hz.max(1e-3) / 440.0).log2()
}

/// Frequency of MIDI note `note` in Hz.
fn note_hz(note: i32) -> f32 {
    440.0 * 2.0_f32.powf((note - 69) as f32 / 12.0)
}

/// Short frequency text: `850`, `1.2k`, `12k`.
pub fn format_hz(hz: f32) -> String {
    if hz >= 10_000.0 {
        format!("{:.0}k", hz / 1_000.0)
    } else if hz >= 1_000.0 {
        let text = format!("{:.1}", hz / 1_000.0);
        format!("{}k", text.trim_end_matches(".0"))
    } else {
        format!("{hz:.0}")
    }
}

/// Precise frequency text with unit.
pub fn format_hz_precise(hz: f32) -> String {
    if hz >= 1_000.0 {
        format!("{:.2} kHz", hz / 1_000.0)
    } else {
        format!("{hz:.1} Hz")
    }
}

/// The nearest note and the deviation from it, for example `A4 +3¢`.
pub fn note_name(hz: f32) -> String {
    if hz < 8.0 {
        return String::from("–");
    }
    let midi = 69.0 + 12.0 * (hz / 440.0).log2();
    let nearest = midi.round();
    let cents = ((midi - nearest) * 100.0).round() as i32;
    let index = (nearest as i32).rem_euclid(12) as usize;
    let octave = (nearest as i32).div_euclid(12) - 1;
    format!("{}{octave} {cents:+}¢", NOTES[index])
}

/// Level text in dB, with a dash for silence.
pub fn format_db(db: f32, unit: &str) -> String {
    if db <= FLOOR_DB + 1.0 || !db.is_finite() {
        format!("–∞ {unit}")
    } else {
        format!("{db:+.1} {unit}")
    }
}

/// Level of a band of per-bin levels: the highest bin, or interpolated between two.
pub fn sample_db(levels: &[f32], band: BandBins) -> f32 {
    match band {
        BandBins::Range(first, last) => levels
            .get(first..=last.min(levels.len().saturating_sub(1)))
            .map_or(FLOOR_DB, |range| {
                range.iter().copied().fold(FLOOR_DB, f32::max)
            }),
        BandBins::Between(position) => {
            let index = position.floor() as usize;
            let fraction = position - index as f32;
            let here = levels.get(index).copied().unwrap_or(FLOOR_DB);
            let next = levels.get(index + 1).copied().unwrap_or(here);
            here + (next - here) * fraction
        }
    }
}

/// Makes `colors` the frame's colors for the plot helpers.
pub fn set_colors(context: &Context, colors: Colors) {
    context.data_mut(|data| data.insert_temp(Id::new(COLORS_ID), colors));
}

/// The frame's colors.
pub fn colors(painter: &Painter) -> Colors {
    painter
        .ctx()
        .data(|data| data.get_temp::<Colors>(Id::new(COLORS_ID)))
        .unwrap_or_default()
}

/// The diagrams' background color.
pub fn background(painter: &Painter) -> Color32 {
    colors(painter).plot
}

/// Grid line color.
pub fn grid(painter: &Painter) -> Color32 {
    colors(painter).grid
}

/// Color of emphasized grid lines.
pub fn grid_strong(painter: &Painter) -> Color32 {
    colors(painter).grid_strong
}

/// Plot background with a border.
pub fn plot_background(painter: &Painter, rect: Rect) {
    painter.rect(
        rect,
        CornerRadius::same(4),
        colors(painter).plot,
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
}

/// Vertical frequency grid lines with labels below `rect`.
pub fn freq_grid_vertical(painter: &Painter, rect: Rect, axis: FreqAxis, labels: bool) {
    for (hz, labelled) in axis.ticks(rect.width()) {
        let x = rect.left() + rect.width() * axis.fraction(hz);
        let color = if labelled {
            grid_strong(painter)
        } else {
            grid(painter)
        };
        painter.vline(x, rect.y_range(), stroke(1.0, color));
        if labels && labelled {
            painter.text(
                pos2(x, rect.bottom() + 3.0),
                Align2::CENTER_TOP,
                axis.label(hz),
                mono(9.0),
                TEXT_FAINT,
            );
        }
    }
}

/// Horizontal frequency grid lines with labels left of `rect`, low frequencies at the
/// bottom.
pub fn freq_grid_horizontal(painter: &Painter, rect: Rect, axis: FreqAxis, labels: bool) {
    for (hz, labelled) in axis.ticks(rect.height()) {
        let y = rect.bottom() - rect.height() * axis.fraction(hz);
        if labelled {
            painter.hline(
                rect.x_range(),
                y,
                stroke(1.0, alpha(grid_strong(painter), 0.5)),
            );
            if labels {
                painter.text(
                    pos2(rect.left() - 4.0, y),
                    Align2::RIGHT_CENTER,
                    axis.label(hz),
                    mono(9.0),
                    TEXT_FAINT,
                );
            }
        }
    }
}

/// Horizontal level grid lines every `step_db` with labels left of `rect`.
pub fn level_grid(painter: &Painter, rect: Rect, (floor, top): (f32, f32), step_db: f32) {
    let first = (floor / step_db).ceil() as i32;
    let last = (top / step_db).floor() as i32;
    for index in first..=last {
        let db = index as f32 * step_db;
        let y = rect.bottom() - rect.height() * (db - floor) / (top - floor);
        let color = if index == 0 {
            grid_strong(painter)
        } else {
            grid(painter)
        };
        painter.hline(rect.x_range(), y, stroke(1.0, color));
        painter.text(
            pos2(rect.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{db:.0}"),
            mono(9.0),
            TEXT_FAINT,
        );
    }
}

/// A readout box of text lines near `anchor`, kept inside `bounds`.
pub fn tooltip(painter: &Painter, anchor: Pos2, bounds: Rect, lines: &[(String, Color32)]) {
    let galleys: Vec<_> = lines
        .iter()
        .map(|(text, color)| painter.layout_no_wrap(text.clone(), mono(11.0), *color))
        .collect();
    let width = galleys
        .iter()
        .map(|galley| galley.size().x)
        .fold(0.0, f32::max)
        + 16.0;
    let height = galleys.iter().map(|galley| galley.size().y).sum::<f32>() + 10.0;
    let mut min = anchor + vec2(14.0, 14.0);
    if min.x + width > bounds.right() {
        min.x = anchor.x - 14.0 - width;
    }
    if min.y + height > bounds.bottom() {
        min.y = anchor.y - 14.0 - height;
    }
    let rect = Rect::from_min_size(min, vec2(width, height));
    painter.rect(
        rect,
        CornerRadius::same(4),
        alpha(PANEL_RAISED, 0.94),
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
    let mut y = rect.top() + 5.0;
    for galley in galleys {
        let height = galley.size().y;
        painter.galley(pos2(rect.left() + 8.0, y), galley, TEXT);
        y += height;
    }
}

/// Highest automatic gain of the scopes.
pub const MAX_GAIN: f32 = 1_000.0;
/// Automatic gain rise per update, as a factor.
const GAIN_RISE: f32 = 1.03;

/// The scopes' automatic gain after an update whose loudest value is `peak`: it falls at
/// once so the peak fills `fill` of the full scale, and rises by 3 % per update.
pub fn follow_gain(gain: f32, peak: f32, fill: f32) -> f32 {
    let target = if peak > 1e-6 {
        (fill / peak).clamp(1.0, MAX_GAIN)
    } else {
        gain
    };
    if target < gain {
        target
    } else {
        (gain * GAIN_RISE).min(target)
    }
}

/// A message across the middle of `rect`, such as why a view has nothing to draw.
pub fn message(painter: &Painter, rect: Rect, text: impl ToString) {
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font(12.0),
        TEXT_DIM,
    );
}

/// A lane's name on a dark tag, `inset` pixels from the lane's top left corner.
pub fn lane_label(painter: &Painter, lane: Rect, name: &str, inset: f32) {
    let galley = painter.layout_no_wrap(name.to_owned(), font(10.0), TEXT);
    let tag = Rect::from_min_size(
        lane.min + vec2(inset, inset),
        galley.size() + vec2(10.0, 4.0),
    );
    painter.rect_filled(tag, 3.0, alpha(Color32::BLACK, 0.55));
    painter.galley(tag.min + vec2(5.0, 2.0), galley, TEXT);
}

/// Places a panel's header items after its title so they never overlap: legends and
/// texts flow from the left, pills and texts are aligned to the right, and a text that
/// does not fit is replaced by its first shorter variant that does, or left out.
pub struct Header<'a> {
    painter: &'a Painter,
    left: f32,
    right: f32,
    centre_y: f32,
}

impl<'a> Header<'a> {
    /// Gap between header items in logical pixels.
    const GAP: f32 = 12.0;

    /// The header of the panel `rect` titled `title` (as `aud_ui::widgets::panel` draws it),
    /// leaving room for the panel's settings button at the right end.
    pub fn new(painter: &'a Painter, rect: Rect, title: &str) -> Self {
        let title_width = text_width(painter, title, &font(12.0));
        Self {
            painter,
            left: rect.left() + 21.0 + title_width + 16.0,
            right: rect.right() - 12.0 - SETTINGS_BUTTON_WIDTH - Self::GAP,
            centre_y: rect.top() + 17.0,
        }
    }

    /// Room left between the items placed so far.
    pub fn room(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    /// Reserves `width` at the right end, for a pill or a button.
    pub fn right_slot(&mut self, width: f32, height: f32) -> Option<Rect> {
        if width > self.room() {
            return None;
        }
        let rect = Rect::from_min_max(
            pos2(self.right - width, self.centre_y - height * 0.5),
            pos2(self.right, self.centre_y + height * 0.5),
        );
        self.right -= width + Self::GAP;
        Some(rect)
    }

    /// A coloured legend entry at the left; `false` if it does not fit.
    pub fn legend(&mut self, label: &str, color: Color32) -> bool {
        let width = 14.0 + text_width(self.painter, label, &font(10.0));
        if width > self.room() {
            return false;
        }
        self.painter.rect_filled(
            Rect::from_center_size(pos2(self.left + 5.0, self.centre_y), vec2(10.0, 3.0)),
            CornerRadius::same(1),
            color,
        );
        self.painter.text(
            pos2(self.left + 14.0, self.centre_y),
            Align2::LEFT_CENTER,
            label,
            font(10.0),
            TEXT,
        );
        self.left += width + Self::GAP;
        true
    }

    /// The first of `variants` that fits, at the left.
    pub fn left_text(&mut self, variants: &[String], font: FontId, color: Color32) {
        if let Some((text, width)) = self.fitting(variants, &font) {
            self.painter.text(
                pos2(self.left, self.centre_y),
                Align2::LEFT_CENTER,
                text,
                font,
                color,
            );
            self.left += width + Self::GAP;
        }
    }

    /// The first of `variants` that fits, aligned to the right.
    pub fn right_text(&mut self, variants: &[String], font: FontId, color: Color32) {
        if let Some((text, width)) = self.fitting(variants, &font) {
            self.painter.text(
                pos2(self.right, self.centre_y),
                Align2::RIGHT_CENTER,
                text,
                font,
                color,
            );
            self.right -= width + Self::GAP;
        }
    }

    fn fitting<'v>(&self, variants: &'v [String], font: &FontId) -> Option<(&'v str, f32)> {
        variants
            .iter()
            .filter(|text| !text.is_empty())
            .map(|text| (text.as_str(), text_width(self.painter, text, font)))
            .find(|(_, width)| *width <= self.room())
    }
}

/// Width of `text` in `font`, in logical pixels.
pub fn text_width(painter: &Painter, text: &str, font: &FontId) -> f32 {
    painter
        .layout_no_wrap(text.to_owned(), font.clone(), TEXT)
        .size()
        .x
}

#[cfg(test)]
mod tests {
    use super::{FreqAxis, format_hz, note_name};
    use crate::settings::FreqScale;

    #[test]
    fn labels_read_naturally() {
        assert_eq!(note_name(440.0), "A4 +0¢");
        assert_eq!(note_name(261.63), "C4 +0¢");
        assert_eq!(format_hz(1_000.0), "1k");
        assert_eq!(format_hz(2_500.0), "2.5k");
        assert_eq!(format_hz(12_000.0), "12k");
        let mut axis = FreqAxis {
            low_hz: 20.0,
            high_hz: 20_000.0,
            scale: FreqScale::Log,
        };
        assert!((axis.hz(axis.fraction(1_000.0)) - 1_000.0).abs() < 0.1);
        axis.scale = FreqScale::Semitone;
        let dense = axis.ticks(2_000.0);
        assert_eq!(
            dense.iter().filter(|(_, labelled)| *labelled).count(),
            10,
            "C1 to C10"
        );
        assert!(dense.len() > 100, "every semitone when there is room");
        assert_eq!(axis.ticks(200.0).len(), 10, "only the Cs when there is not");
        assert_eq!(axis.label(261.63), "C4");
        axis.scale = FreqScale::Log;
        assert!(
            axis.ticks(1_000.0)
                .iter()
                .any(|(hz, labelled)| *hz == 20_000.0 && *labelled)
        );
    }
}
