//! The sonogram, a scrolling time × frequency image with the newest column on the
//! right, and the spectral view, which overlays the waveform on it as in spectral
//! editors. Both show one signal, or two in lanes stacked one above the other.

use aud_ui::theme::{TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::panel;
use egui::{
    Align2, Color32, ColorImage, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, TextureHandle,
    TextureOptions, Ui, pos2, vec2,
};

use crate::analysis::Published;
use crate::analysis::fft::FLOOR_DB;
use crate::analysis::spectrum::{SONOGRAM_ROWS, WaveExtent};
use crate::settings::{Choice, Signal, SonogramSettings, SpectralSettings, WaveScale};
use crate::style::{HeatStyle, SonogramStyle, SpectralStyle};
use crate::view::colormap::{Colors, Palette, lighten, rgb};
use crate::view::plot::{
    FreqAxis, Header, format_db, format_hz_precise, freq_grid_horizontal, note_name,
    plot_background, tooltip,
};

/// Columns kept in the image.
const COLUMNS: usize = 1_024;
/// Lanes the image has room for.
const MAX_LANES: usize = 2;
/// Gap between two lanes in logical pixels.
const LANE_GAP: f32 = 6.0;
/// Lowest level of the waveform's dB scale, at the centre line.
const WAVE_FLOOR_DB: f32 = -60.0;

/// One colored image of the levels, for one view with its own heat map.
#[derive(Default)]
struct Image {
    palette: Palette,
    /// Maps levels to the color scale.
    tone: SonogramSettings,
    /// One texture with the lanes one above the other.
    texture: Option<TextureHandle>,
    /// Columns written into the levels when this image was last uploaded.
    uploaded: u64,
    /// The whole image must be uploaded again.
    stale: bool,
}

/// Which image a view draws: the sonogram's or the spectral view's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFor {
    Sonogram,
    Spectral,
}

/// The levels and the waveform as rings of columns, and one image per view.
pub struct SonogramView {
    /// Levels in dB, column by column, lane by lane, lowest frequency first.
    levels: Vec<f32>,
    waves: Vec<[WaveExtent; MAX_LANES]>,
    lanes: usize,
    /// Next column to write.
    head: usize,
    /// Columns written since the start, to know what each image lacks.
    written: u64,
    generation: u64,
    images: [Image; 2],
}

impl Default for SonogramView {
    fn default() -> Self {
        Self {
            levels: vec![FLOOR_DB; COLUMNS * MAX_LANES * SONOGRAM_ROWS],
            waves: vec![[WaveExtent::default(); MAX_LANES]; COLUMNS],
            lanes: 1,
            head: 0,
            written: 0,
            generation: 0,
            images: [Image::default(), Image::default()],
        }
    }
}

/// Image height in texels: every lane's rows.
const IMAGE_ROWS: usize = MAX_LANES * SONOGRAM_ROWS;

impl SonogramView {
    /// Takes the new columns out of `data` and writes them into the rings.
    pub fn take_columns(&mut self, data: &mut Published) {
        if data.sonogram.generation != self.generation {
            self.generation = data.sonogram.generation;
            self.levels.fill(FLOOR_DB);
            self.waves.fill([WaveExtent::default(); MAX_LANES]);
            self.images.iter_mut().for_each(|image| image.stale = true);
        }
        for column in data.sonogram.columns.drain(..) {
            // Each column says how many lanes it has.
            self.lanes = (column.levels.len() / SONOGRAM_ROWS).clamp(1, MAX_LANES);
            let start = self.head * MAX_LANES * SONOGRAM_ROWS;
            let count = column.levels.len().min(MAX_LANES * SONOGRAM_ROWS);
            self.levels[start..start + count].copy_from_slice(&column.levels[..count]);
            self.levels[start + count..start + MAX_LANES * SONOGRAM_ROWS].fill(FLOOR_DB);
            self.waves[self.head] = column.wave;
            self.head = (self.head + 1) % COLUMNS;
            self.written += 1;
        }
    }

    fn level(&self, column: usize, lane: usize, row: usize) -> f32 {
        self.levels[(column * MAX_LANES + lane) * SONOGRAM_ROWS + row]
    }

    /// Texel row of a lane's frequency row: lanes top to bottom, high frequencies up.
    fn image_row(lane: usize, row: usize) -> usize {
        lane * SONOGRAM_ROWS + (SONOGRAM_ROWS - 1 - row)
    }

    fn column_image(&self, image: &Image, column: usize) -> ColorImage {
        let mut pixels = ColorImage::new([1, IMAGE_ROWS], Color32::BLACK);
        for lane in 0..MAX_LANES {
            for row in 0..SONOGRAM_ROWS {
                pixels.pixels[Self::image_row(lane, row)] = image
                    .palette
                    .color(image.tone.tone(self.level(column, lane, row)));
            }
        }
        pixels
    }

    fn full_image(&self, image: &Image) -> ColorImage {
        let mut pixels = ColorImage::new([COLUMNS, IMAGE_ROWS], Color32::BLACK);
        for column in 0..COLUMNS {
            for lane in 0..MAX_LANES {
                for row in 0..SONOGRAM_ROWS {
                    pixels.pixels[Self::image_row(lane, row) * COLUMNS + column] = image
                        .palette
                        .color(image.tone.tone(self.level(column, lane, row)));
                }
            }
        }
        pixels
    }

    /// Uploads what changed in the image of `which`, colored with `heat` and `tone`.
    fn update(&mut self, ui: &Ui, which: ImageFor, heat: HeatStyle, tone: &SonogramSettings) {
        let index = which as usize;
        let toned = |settings: &SonogramSettings| {
            (settings.range, settings.brightness_db, settings.contrast)
        };
        {
            let image = &mut self.images[index];
            if heat != image.palette.heat() {
                image.palette = Palette::new(heat);
                image.stale = true;
            }
            if toned(tone) != toned(&image.tone) {
                image.tone = *tone;
                image.stale = true;
            }
        }
        let options = TextureOptions::LINEAR;
        let image = &self.images[index];
        let missing = self.written - image.uploaded;
        if image.stale || image.texture.is_none() || missing >= COLUMNS as u64 {
            let pixels = self.full_image(image);
            let image = &mut self.images[index];
            match &mut image.texture {
                Some(texture) => texture.set(pixels, options),
                None => {
                    let name = format!("o-aud-a-sonogram-{index}");
                    image.texture = Some(ui.ctx().load_texture(name, pixels, options));
                }
            }
            image.stale = false;
            image.uploaded = self.written;
            return;
        }
        let columns: Vec<_> = (0..missing as usize)
            .map(|age| (self.head + COLUMNS - 1 - age) % COLUMNS)
            .map(|column| (column, self.column_image(image, column)))
            .collect();
        let image = &mut self.images[index];
        if let Some(texture) = &mut image.texture {
            for (column, pixels) in columns {
                texture.set_partial([column, 0], pixels, options);
            }
        }
        image.uploaded = self.written;
    }

    /// Ring column at `fraction` (0 oldest, 1 newest) of the shown time.
    fn column_at(&self, fraction: f32) -> usize {
        (self.head + (fraction.clamp(0.0, 0.9999) * COLUMNS as f32) as usize) % COLUMNS
    }

    /// Draws one lane of the image of `which`, from the oldest column to the newest,
    /// across `plot`.
    fn draw_image(
        &self,
        painter: &Painter,
        plot: Rect,
        (which, lane): (ImageFor, usize),
        tint: Color32,
    ) {
        let Some(texture) = &self.images[which as usize].texture else {
            return;
        };
        let split = self.head as f32 / COLUMNS as f32;
        let top = lane as f32 / MAX_LANES as f32;
        let bottom = (lane + 1) as f32 / MAX_LANES as f32;
        let older = Rect::from_min_max(
            plot.min,
            pos2(plot.left() + plot.width() * (1.0 - split), plot.bottom()),
        );
        let newer = Rect::from_min_max(pos2(older.right(), plot.top()), plot.max);
        painter.image(
            texture.id(),
            older,
            Rect::from_min_max(pos2(split, top), pos2(1.0, bottom)),
            tint,
        );
        painter.image(
            texture.id(),
            newer,
            Rect::from_min_max(pos2(0.0, top), pos2(split, bottom)),
            tint,
        );
    }

    /// The palette of the image of `which`, for color bars.
    fn palette(&self, which: ImageFor) -> &Palette {
        &self.images[which as usize].palette
    }
}

/// The lanes' rectangles in `plot`, top to bottom.
fn lane_rects(plot: Rect, lanes: usize) -> Vec<Rect> {
    let lanes = lanes.clamp(1, MAX_LANES);
    let height = (plot.height() - LANE_GAP * (lanes - 1) as f32) / lanes as f32;
    (0..lanes)
        .map(|lane| {
            let top = plot.top() + lane as f32 * (height + LANE_GAP);
            Rect::from_min_max(pos2(plot.left(), top), pos2(plot.right(), top + height))
        })
        .collect()
}

/// The lane under `pointer` and the pointer's height in it from 0 (bottom) to 1.
fn lane_at(lanes: &[Rect], pointer: Pos2) -> Option<(usize, f32)> {
    lanes
        .iter()
        .position(|lane| lane.y_range().contains(pointer.y))
        .map(|index| {
            let lane = lanes[index];
            (
                index,
                ((lane.bottom() - pointer.y) / lane.height()).clamp(0.0, 0.9999),
            )
        })
}

/// Names of the lanes' signals.
fn lane_names(settings: &SonogramSettings) -> [&'static str; MAX_LANES] {
    let (first, second) = settings.signals();
    [
        Signal::LABELS[first.index()],
        second.map_or("", |signal| Signal::LABELS[signal.index()]),
    ]
}

/// A lane's name in its top left corner, when there are two.
fn lane_label(painter: &Painter, lane: Rect, name: &str) {
    let galley = painter.layout_no_wrap(name.to_owned(), font(10.0), TEXT);
    let size = galley.size();
    let tag = Rect::from_min_size(lane.min + vec2(4.0, 4.0), size + vec2(10.0, 4.0));
    painter.rect_filled(tag, 3.0, alpha(Color32::BLACK, 0.55));
    painter.galley(tag.min + vec2(5.0, 2.0), galley, TEXT);
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
    let [first, second] = lane_names(settings);
    let signals = if second.is_empty() {
        first.to_lowercase()
    } else {
        format!("{} / {}", first.to_lowercase(), second.to_lowercase())
    };
    let points = settings.fft.samples();
    [
        format!(
            "{signals} · {points} · {:.0} col/s",
            settings.speed.columns_per_s()
        ),
        format!("{signals} · {points}"),
        signals,
    ]
}

/// Draws the sonogram panel.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SonogramView,
    data: &Published,
    settings: &SonogramSettings,
    (colors, style): (&Colors, &SonogramStyle),
    compact: bool,
) {
    panel(ui, rect, "SONOGRAM", colors.accent);
    let legend_width = if compact { 0.0 } else { 46.0 };
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 12.0 - legend_width, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    view.update(ui, ImageFor::Sonogram, style.heat, settings);
    let lanes = lane_rects(plot, view.lanes);
    let names = lane_names(settings);
    let axis = freq_axis(data);
    for (index, lane) in lanes.iter().enumerate() {
        plot_background(&painter, *lane);
        view.draw_image(&painter, *lane, (ImageFor::Sonogram, index), Color32::WHITE);
        if let Some(axis) = axis {
            freq_grid_horizontal(&painter, *lane, axis, true);
        }
        if lanes.len() > 1 {
            lane_label(&painter, *lane, names[index]);
        }
    }
    let span_s = span_s(settings);
    time_axis(&painter, plot, span_s);
    Header::new(&painter, rect, "SONOGRAM").left_text(&info(settings), font(10.0), TEXT_DIM);
    let Some(axis) = axis else {
        return;
    };

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
            painter.rect_filled(slice, 0.0, view.palette(ImageFor::Sonogram).color(fraction));
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
        let Some((lane, y_fraction)) = lane_at(&lanes, pointer) else {
            return;
        };
        let x_fraction = (pointer.x - plot.left()) / plot.width();
        let column = view.column_at(x_fraction);
        let row = (y_fraction * SONOGRAM_ROWS as f32) as usize;
        let level = view.level(column, lane, row);
        let hz = axis.hz(y_fraction);
        let ago = (1.0 - x_fraction.clamp(0.0, 1.0)) * span_s;
        painter.hline(plot.x_range(), pointer.y, stroke(1.0, alpha(TEXT, 0.25)));
        let mut lines = vec![
            (
                format!("{}  {}", format_hz_precise(hz), note_name(hz)),
                TEXT,
            ),
            (format!("−{ago:.2} s"), TEXT_DIM),
        ];
        let level = format_db(level, "dB");
        lines.push((
            if lanes.len() > 1 {
                format!("{} {level}", names[lane].to_lowercase())
            } else {
                level
            },
            TEXT,
        ));
        tooltip(&painter, pointer, plot, &lines);
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

/// Amplitude labels right of a lane.
fn amplitude_scale(painter: &Painter, lane: Rect, scale: WaveScale, gain_db: f32) {
    let gain = 10.0_f32.powf(gain_db / 20.0);
    let centre = lane.center().y;
    let half = lane.height() * 0.5;
    let offsets: &[f32] = if lane.height() > 160.0 {
        &[1.0, 0.5, 0.0, -0.5, -1.0]
    } else {
        &[1.0, 0.0, -1.0]
    };
    for offset in offsets {
        let label = match scale {
            WaveScale::Linear => format!("{:.2}", offset / gain),
            WaveScale::Db => format!("{:.0}", WAVE_FLOOR_DB * (1.0 - offset.abs()) - gain_db),
        };
        let align = if *offset >= 1.0 {
            Align2::LEFT_TOP
        } else if *offset <= -1.0 {
            Align2::LEFT_BOTTOM
        } else {
            Align2::LEFT_CENTER
        };
        painter.text(
            pos2(lane.right() + 4.0, centre - offset * half),
            align,
            label,
            mono(9.0),
            TEXT_FAINT,
        );
    }
}

/// The waveform of one lane as peak extent and RMS core per pixel column.
fn draw_waveform(
    painter: &Painter,
    lane_rect: Rect,
    (view, lane): (&SonogramView, usize),
    spectral: &SpectralSettings,
    (color, opacity): (Color32, f32),
) {
    let gain_db = SpectralSettings::WAVEFORM_GAIN_DB.clamp(spectral.waveform_gain_db);
    let gain = 10.0_f32.powf(gain_db / 20.0);
    let centre = lane_rect.center().y;
    let half = lane_rect.height() * 0.5;
    painter.hline(
        lane_rect.x_range(),
        centre,
        stroke(1.0, alpha(TEXT, 0.2 * opacity)),
    );
    let envelope = alpha(color, 0.55 * opacity);
    let core = alpha(lighten(color, 0.45), 0.9 * opacity);
    let mut mesh = Mesh::default();
    let width = lane_rect.width().max(1.0) as usize;
    let per_pixel = COLUMNS as f32 / width as f32;
    let y_of = |value: f32| centre - wave_offset(value * gain, spectral.scale) * half;
    for pixel in 0..width {
        let first = (pixel as f32 * per_pixel) as usize;
        let last = (((pixel + 1) as f32 * per_pixel) as usize).clamp(first + 1, COLUMNS);
        let (min, max, rms) = (first..last)
            .map(|offset| view.waves[(view.head + offset) % COLUMNS][lane])
            .fold((0.0_f32, 0.0_f32, 0.0_f32), |(min, max, rms), wave| {
                (min.min(wave.min), max.max(wave.max), rms.max(wave.rms))
            });
        let x = lane_rect.left() + pixel as f32;
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
    painter.with_clip_rect(lane_rect).add(Shape::mesh(mesh));
}

/// Draws the spectral view: the sonogram with the waveform of the same signal on top.
pub fn draw_spectral(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SonogramView,
    data: &Published,
    (sonogram, spectral): (&SonogramSettings, &SpectralSettings),
    (colors, style): (&Colors, &SpectralStyle),
) {
    panel(ui, rect, "SPECTRAL", colors.accent);
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 40.0, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    view.update(ui, ImageFor::Spectral, style.heat, sonogram);
    let image_opacity = SpectralSettings::OPACITY.clamp(spectral.sonogram_opacity);
    let wave_opacity = SpectralSettings::OPACITY.clamp(spectral.waveform_opacity);
    let gain_db = SpectralSettings::WAVEFORM_GAIN_DB.clamp(spectral.waveform_gain_db);
    let lanes = lane_rects(plot, view.lanes);
    let names = lane_names(sonogram);
    let axis = freq_axis(data);
    let second = if sonogram.signals().1 == Some(Signal::Side) {
        style.side
    } else {
        style.right
    };
    let wave_colors = [rgb(style.first), rgb(second)];
    for (index, lane) in lanes.iter().enumerate() {
        plot_background(&painter, *lane);
        if image_opacity > 0.0 {
            view.draw_image(
                &painter,
                *lane,
                (ImageFor::Spectral, index),
                alpha(Color32::WHITE, image_opacity),
            );
        }
        if let Some(axis) = axis {
            freq_grid_horizontal(&painter, *lane, axis, true);
        }
        amplitude_scale(&painter, *lane, spectral.scale, gain_db);
        if wave_opacity > 0.0 {
            draw_waveform(
                &painter,
                *lane,
                (view, index),
                spectral,
                (wave_colors[index], wave_opacity),
            );
        }
        if lanes.len() > 1 {
            lane_label(&painter, *lane, names[index]);
        }
    }
    let span_s = span_s(sonogram);
    time_axis(&painter, plot, span_s);
    Header::new(&painter, rect, "SPECTRAL").left_text(&info(sonogram), font(10.0), TEXT_DIM);
    painter.text(
        pos2(plot.right() + 4.0, plot.top() - 10.0),
        Align2::LEFT_CENTER,
        if spectral.scale == WaveScale::Db {
            "dB"
        } else {
            "lin"
        },
        font(9.0),
        TEXT_FAINT,
    );

    let response = ui.interact(plot, Id::new("spectral-plot"), Sense::hover());
    if let Some(pointer) = response.hover_pos() {
        let Some((lane, y_fraction)) = lane_at(&lanes, pointer) else {
            return;
        };
        let x_fraction = (pointer.x - plot.left()) / plot.width();
        let column = view.column_at(x_fraction);
        let ago = (1.0 - x_fraction.clamp(0.0, 1.0)) * span_s;
        let wave = view.waves[column][lane];
        let peak = wave.max.abs().max(wave.min.abs());
        let mut lines = vec![(format!("−{ago:.2} s"), TEXT)];
        if lanes.len() > 1 {
            lines.push((names[lane].to_owned(), wave_colors[lane]));
        }
        if let Some(axis) = axis {
            let row = (y_fraction * SONOGRAM_ROWS as f32) as usize;
            let hz = axis.hz(y_fraction);
            lines.push((
                format!("{}  {}", format_hz_precise(hz), note_name(hz)),
                TEXT_DIM,
            ));
            lines.push((
                format!("level {}", format_db(view.level(column, lane, row), "dB")),
                TEXT,
            ));
        }
        let to_db = |value: f32| {
            if value > 1e-9 {
                20.0 * value.log10()
            } else {
                FLOOR_DB
            }
        };
        lines.push((
            format!(
                "peak {}  rms {}",
                format_db(to_db(peak), "dBFS"),
                format_db(to_db(wave.rms), "dBFS")
            ),
            wave_colors[lane],
        ));
        painter.hline(plot.x_range(), pointer.y, stroke(1.0, alpha(TEXT, 0.2)));
        painter.vline(pointer.x, plot.y_range(), stroke(1.0, alpha(TEXT, 0.2)));
        tooltip(&painter, pointer, plot, &lines);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2};

    use super::{LANE_GAP, lane_at, lane_rects, wave_offset};
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

    #[test]
    fn lanes_split_the_plot() {
        let plot = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 206.0));
        let lanes = lane_rects(plot, 2);
        assert_eq!(lanes[0].height(), 100.0);
        assert_eq!(lanes[1].top(), 100.0 + LANE_GAP);
        assert_eq!(
            lane_at(&lanes, pos2(50.0, 150.0)).map(|(lane, _)| lane),
            Some(1)
        );
        assert_eq!(lane_at(&lanes, pos2(50.0, 102.0)), None, "the gap");
        assert_eq!(lane_rects(plot, 1), vec![plot]);
    }
}
