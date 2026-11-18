//! The sonogram, a scrolling time × frequency image with the newest column on the
//! right, and the spectral view, which overlays the waveform on it as in spectral
//! editors.

use aud_ui::theme::{TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::panel;
use egui::{
    Align2, Color32, ColorImage, Id, Mesh, Painter, Rect, Sense, Shape, TextureHandle,
    TextureOptions, Ui, pos2,
};

use crate::analysis::Published;
use crate::analysis::fft::FLOOR_DB;
use crate::analysis::spectrum::{SONOGRAM_ROWS, SonogramColumn, WaveExtent};
use crate::settings::{Choice, Signal, SonogramSettings, SpectralSettings, WaveScale};
use crate::view::colormap::{Colors, Heat, Palette, lighten};
use crate::view::plot::{
    FreqAxis, Header, format_db, format_hz_precise, freq_grid_horizontal, note_name,
    plot_background, tooltip,
};

/// Columns kept in the image.
const COLUMNS: usize = 1_024;
/// Lowest level of the waveform's dB scale, at the centre line.
const WAVE_FLOOR_DB: f32 = -60.0;

/// The image, the levels it shows and the waveform, as rings of columns.
pub struct SonogramView {
    /// Levels in dB, column by column, lowest frequency first.
    levels: Vec<f32>,
    waves: Vec<WaveExtent>,
    /// Next column to write.
    head: usize,
    generation: u64,
    palette: Palette,
    /// Maps levels to the color scale.
    tone: SonogramSettings,
    texture: Option<TextureHandle>,
    /// Columns waiting to be written, taken from the published data.
    pending: Vec<SonogramColumn>,
    /// The whole image must be uploaded again.
    stale: bool,
}

impl Default for SonogramView {
    fn default() -> Self {
        Self {
            levels: vec![FLOOR_DB; COLUMNS * SONOGRAM_ROWS],
            waves: vec![WaveExtent::default(); COLUMNS],
            head: 0,
            generation: 0,
            palette: Palette::default(),
            tone: SonogramSettings::default(),
            texture: None,
            pending: Vec::new(),
            stale: true,
        }
    }
}

impl SonogramView {
    /// Takes the new columns out of `data`.
    pub fn take_columns(&mut self, data: &mut Published) {
        if data.sonogram.generation != self.generation {
            self.generation = data.sonogram.generation;
            self.levels.fill(FLOOR_DB);
            self.waves.fill(WaveExtent::default());
            self.pending.clear();
            self.stale = true;
        }
        self.pending.extend(data.sonogram.columns.drain(..));
    }

    fn column_image(&self, column: usize) -> ColorImage {
        let levels = &self.levels[column * SONOGRAM_ROWS..(column + 1) * SONOGRAM_ROWS];
        let mut image = ColorImage::new([1, SONOGRAM_ROWS], Color32::BLACK);
        for (pixel, level) in image.pixels.iter_mut().zip(levels.iter().rev()) {
            *pixel = self.palette.color(self.tone.tone(*level));
        }
        image
    }

    fn full_image(&self) -> ColorImage {
        let mut image = ColorImage::new([COLUMNS, SONOGRAM_ROWS], Color32::BLACK);
        for column in 0..COLUMNS {
            let levels = &self.levels[column * SONOGRAM_ROWS..(column + 1) * SONOGRAM_ROWS];
            for (row, level) in levels.iter().rev().enumerate() {
                image.pixels[row * COLUMNS + column] = self.palette.color(self.tone.tone(*level));
            }
        }
        image
    }

    /// Writes the pending columns and uploads what changed.
    fn update(&mut self, ui: &Ui, heat: Heat, tone: &SonogramSettings) {
        if heat != self.palette.heat() {
            self.palette = Palette::new(heat);
            self.stale = true;
        }
        let toned = |settings: &SonogramSettings| {
            (settings.range, settings.brightness_db, settings.contrast)
        };
        if toned(tone) != toned(&self.tone) {
            self.tone = *tone;
            self.stale = true;
        }
        let mut written = Vec::new();
        for column in self.pending.drain(..) {
            let start = self.head * SONOGRAM_ROWS;
            let count = column.levels.len().min(SONOGRAM_ROWS);
            self.levels[start..start + count].copy_from_slice(&column.levels[..count]);
            self.waves[self.head] = column.wave;
            written.push(self.head);
            self.head = (self.head + 1) % COLUMNS;
        }
        let options = TextureOptions::LINEAR;
        if self.stale || self.texture.is_none() {
            let image = self.full_image();
            match &mut self.texture {
                Some(texture) => texture.set(image, options),
                None => {
                    self.texture = Some(ui.ctx().load_texture("o-aud-a-sonogram", image, options));
                }
            }
            self.stale = false;
            return;
        }
        let images: Vec<_> = written
            .into_iter()
            .map(|column| (column, self.column_image(column)))
            .collect();
        if let Some(texture) = &mut self.texture {
            for (column, image) in images {
                texture.set_partial([column, 0], image, options);
            }
        }
    }

    /// Ring column at `fraction` (0 oldest, 1 newest) of the shown time.
    fn column_at(&self, fraction: f32) -> usize {
        (self.head + (fraction.clamp(0.0, 0.9999) * COLUMNS as f32) as usize) % COLUMNS
    }

    /// Draws the ring from the oldest column to the newest across `plot`.
    fn draw_image(&self, painter: &Painter, plot: Rect, tint: Color32) {
        let Some(texture) = &self.texture else {
            return;
        };
        let split = self.head as f32 / COLUMNS as f32;
        let older = Rect::from_min_max(
            plot.min,
            pos2(plot.left() + plot.width() * (1.0 - split), plot.bottom()),
        );
        let newer = Rect::from_min_max(pos2(older.right(), plot.top()), plot.max);
        painter.image(
            texture.id(),
            older,
            Rect::from_min_max(pos2(split, 0.0), pos2(1.0, 1.0)),
            tint,
        );
        painter.image(
            texture.id(),
            newer,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(split, 1.0)),
            tint,
        );
    }
}

/// Shown time in seconds.
fn span_s(settings: &SonogramSettings) -> f32 {
    COLUMNS as f32 / settings.speed.columns_per_s()
}

/// Time grid with labels below `plot`, newest on the right.
fn time_axis(painter: &Painter, plot: Rect, span_s: f32) {
    let step_s = [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0]
        .into_iter()
        .find(|step| span_s / step <= 8.0)
        .unwrap_or(30.0);
    let mut seconds = 0.0;
    while seconds <= span_s {
        let x = plot.right() - plot.width() * seconds / span_s;
        painter.vline(x, plot.y_range(), stroke(1.0, alpha(Color32::WHITE, 0.06)));
        painter.text(
            pos2(x, plot.bottom() + 3.0),
            Align2::CENTER_TOP,
            if seconds == 0.0 {
                String::from("now")
            } else {
                format!("−{seconds} s")
            },
            mono(9.0),
            TEXT_FAINT,
        );
        seconds += step_s;
    }
}

fn freq_axis(data: &Published) -> Option<FreqAxis> {
    let (low_hz, high_hz) = data.sonogram.range_hz;
    (high_hz > low_hz).then_some(FreqAxis {
        low_hz,
        high_hz,
        scale: data.sonogram.scale,
    })
}

/// Header text variants, longest first.
fn info(settings: &SonogramSettings) -> [String; 3] {
    let signal = Signal::LABELS[settings.signal.index()].to_lowercase();
    let points = settings.fft.samples();
    [
        format!(
            "{signal} · {points} · {:.0} col/s",
            settings.speed.columns_per_s()
        ),
        format!("{signal} · {points}"),
        signal,
    ]
}

/// Draws the sonogram panel.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SonogramView,
    data: &Published,
    settings: &SonogramSettings,
    colors: &Colors,
    compact: bool,
) {
    panel(ui, rect, "SONOGRAM", colors.accent);
    let legend_width = if compact { 0.0 } else { 46.0 };
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 12.0 - legend_width, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    plot_background(&painter, plot);
    view.update(ui, colors.heat, settings);
    view.draw_image(&painter, plot, Color32::WHITE);
    let Some(axis) = freq_axis(data) else {
        return;
    };
    freq_grid_horizontal(&painter, plot, axis, true);
    let span_s = span_s(settings);
    time_axis(&painter, plot, span_s);
    Header::new(&painter, rect, "SONOGRAM").left_text(&info(settings), font(10.0), TEXT_DIM);

    if !compact {
        let bar = Rect::from_min_max(
            pos2(plot.right() + 12.0, plot.top()),
            pos2(plot.right() + 22.0, plot.bottom()),
        );
        let steps = 64;
        for step in 0..steps {
            let fraction = step as f32 / steps as f32;
            let slice = Rect::from_min_max(
                pos2(
                    bar.left(),
                    bar.bottom() - bar.height() * (fraction + 1.0 / steps as f32),
                ),
                pos2(bar.right(), bar.bottom() - bar.height() * fraction),
            );
            painter.rect_filled(slice, 0.0, view.palette.color(fraction));
        }
        for db in [0.0, -30.0, -60.0, -90.0, -120.0] {
            let fraction = settings.tone(db);
            if fraction <= 0.0 || (fraction >= 1.0 && db < 0.0) {
                continue;
            }
            let y = bar.bottom() - bar.height() * fraction;
            painter.text(
                pos2(bar.right() + 3.0, y),
                Align2::LEFT_CENTER,
                format!("{db:.0}"),
                mono(9.0),
                TEXT_FAINT,
            );
        }
    }

    let response = ui.interact(plot, Id::new("sonogram-plot"), Sense::hover());
    if let Some(pointer) = response.hover_pos() {
        let x_fraction = (pointer.x - plot.left()) / plot.width();
        let y_fraction = ((plot.bottom() - pointer.y) / plot.height()).clamp(0.0, 0.9999);
        let column = view.column_at(x_fraction);
        let row = (y_fraction * SONOGRAM_ROWS as f32) as usize;
        let level = view.levels[column * SONOGRAM_ROWS + row];
        let hz = axis.hz(y_fraction);
        let ago = (1.0 - x_fraction.clamp(0.0, 1.0)) * span_s;
        painter.hline(plot.x_range(), pointer.y, stroke(1.0, alpha(TEXT, 0.25)));
        tooltip(
            &painter,
            pointer,
            plot,
            &[
                (
                    format!("{}  {}", format_hz_precise(hz), note_name(hz)),
                    TEXT,
                ),
                (format!("−{ago:.2} s"), TEXT_DIM),
                (format_db(level, "dB"), colors.primary),
            ],
        );
    }
}

/// Vertical offset of a waveform value from the centre, −1..1.
fn wave_offset(value: f32, scale: WaveScale) -> f32 {
    match scale {
        WaveScale::Linear => value.clamp(-1.0, 1.0),
        WaveScale::Db => {
            let magnitude = value.abs();
            if magnitude <= 1e-9 {
                return 0.0;
            }
            let db = 20.0 * magnitude.log10();
            value.signum() * ((db - WAVE_FLOOR_DB) / -WAVE_FLOOR_DB).clamp(0.0, 1.0)
        }
    }
}

/// Draws the spectral view: the sonogram with the waveform of the same signal on top.
pub fn draw_spectral(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SonogramView,
    data: &Published,
    (sonogram, spectral): (&SonogramSettings, &SpectralSettings),
    colors: &Colors,
) {
    panel(ui, rect, "SPECTRAL", colors.accent);
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 40.0, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    plot_background(&painter, plot);
    view.update(ui, colors.heat, sonogram);
    let image_opacity = SpectralSettings::OPACITY.clamp(spectral.sonogram_opacity);
    let wave_opacity = SpectralSettings::OPACITY.clamp(spectral.waveform_opacity);
    let gain_db = SpectralSettings::WAVEFORM_GAIN_DB.clamp(spectral.waveform_gain_db);
    let gain = 10.0_f32.powf(gain_db / 20.0);
    if image_opacity > 0.0 {
        view.draw_image(&painter, plot, alpha(Color32::WHITE, image_opacity));
    }
    let axis = freq_axis(data);
    if let Some(axis) = axis {
        freq_grid_horizontal(&painter, plot, axis, true);
    }
    let span_s = span_s(sonogram);
    time_axis(&painter, plot, span_s);
    Header::new(&painter, rect, "SPECTRAL").left_text(&info(sonogram), font(10.0), TEXT_DIM);

    // Amplitude scale on the right.
    let centre = plot.center().y;
    let half = plot.height() * 0.5;
    for offset in [1.0_f32, 0.5, 0.0, -0.5, -1.0] {
        let label = match spectral.scale {
            WaveScale::Linear => format!("{:.2}", offset / gain),
            WaveScale::Db => format!("{:.0}", WAVE_FLOOR_DB * (1.0 - offset.abs()) - gain_db),
        };
        painter.text(
            pos2(plot.right() + 4.0, centre - offset * half),
            Align2::LEFT_CENTER,
            label,
            mono(9.0),
            TEXT_FAINT,
        );
    }
    let unit = if spectral.scale == WaveScale::Db {
        "dB"
    } else {
        "lin"
    };
    painter.text(
        pos2(plot.right() + 4.0, plot.top() - 10.0),
        Align2::LEFT_CENTER,
        unit,
        font(9.0),
        TEXT_FAINT,
    );

    if wave_opacity > 0.0 {
        painter.hline(
            plot.x_range(),
            centre,
            stroke(1.0, alpha(TEXT, 0.2 * wave_opacity)),
        );
        let envelope = alpha(colors.primary, 0.55 * wave_opacity);
        let core = alpha(lighten(colors.primary, 0.45), 0.9 * wave_opacity);
        let mut mesh = Mesh::default();
        let width = plot.width().max(1.0) as usize;
        let per_pixel = COLUMNS as f32 / width as f32;
        for pixel in 0..width {
            let first = (pixel as f32 * per_pixel) as usize;
            let last = (((pixel + 1) as f32 * per_pixel) as usize).clamp(first + 1, COLUMNS);
            let (min, max, rms) = (first..last)
                .map(|offset| view.waves[(view.head + offset) % COLUMNS])
                .fold((0.0_f32, 0.0_f32, 0.0_f32), |(min, max, rms), wave| {
                    (min.min(wave.min), max.max(wave.max), rms.max(wave.rms))
                });
            let x = plot.left() + pixel as f32;
            let y_of = |value: f32| centre - wave_offset(value * gain, spectral.scale) * half;
            let top = y_of(max);
            let bottom = y_of(min).max(top + 1.0);
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x, top), pos2(x + 1.0, bottom)),
                envelope,
            );
            if rms > 0.0 {
                let rms_top = y_of(rms).max(top);
                let rms_bottom = y_of(-rms).min(bottom);
                if rms_bottom > rms_top {
                    mesh.add_colored_rect(
                        Rect::from_min_max(pos2(x, rms_top), pos2(x + 1.0, rms_bottom)),
                        core,
                    );
                }
            }
        }
        painter.with_clip_rect(plot).add(Shape::mesh(mesh));
    }

    let response = ui.interact(plot, Id::new("spectral-plot"), Sense::hover());
    if let Some(pointer) = response.hover_pos() {
        let x_fraction = (pointer.x - plot.left()) / plot.width();
        let y_fraction = ((plot.bottom() - pointer.y) / plot.height()).clamp(0.0, 0.9999);
        let column = view.column_at(x_fraction);
        let ago = (1.0 - x_fraction.clamp(0.0, 1.0)) * span_s;
        let wave = view.waves[column];
        let peak = wave.max.abs().max(wave.min.abs());
        let mut lines = vec![(format!("−{ago:.2} s"), TEXT)];
        if let Some(axis) = axis {
            let row = (y_fraction * SONOGRAM_ROWS as f32) as usize;
            let hz = axis.hz(y_fraction);
            lines.push((
                format!("{}  {}", format_hz_precise(hz), note_name(hz)),
                TEXT_DIM,
            ));
            lines.push((
                format!(
                    "level {}",
                    format_db(view.levels[column * SONOGRAM_ROWS + row], "dB")
                ),
                colors.primary,
            ));
        }
        let peak_db = if peak > 1e-9 {
            20.0 * peak.log10()
        } else {
            FLOOR_DB
        };
        let rms_db = if wave.rms > 1e-9 {
            20.0 * wave.rms.log10()
        } else {
            FLOOR_DB
        };
        lines.push((
            format!(
                "peak {}  rms {}",
                format_db(peak_db, "dBFS"),
                format_db(rms_db, "dBFS")
            ),
            colors.primary,
        ));
        painter.hline(plot.x_range(), pointer.y, stroke(1.0, alpha(TEXT, 0.2)));
        painter.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.2)));
        tooltip(&painter, pointer, plot, &lines);
    }
}

#[cfg(test)]
mod tests {
    use super::wave_offset;
    use crate::settings::WaveScale;

    #[test]
    fn waveform_scales_map_full_scale_to_the_edges() {
        assert_eq!(wave_offset(-1.0, WaveScale::Linear), -1.0);
        assert!((wave_offset(1.0, WaveScale::Db) - 1.0).abs() < 1e-6);
        assert!(
            (wave_offset(-0.001, WaveScale::Db)).abs() < 1e-6,
            "−60 dB at the centre"
        );
        assert_eq!(wave_offset(0.0, WaveScale::Db), 0.0);
    }
}
