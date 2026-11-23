//! The sonogram, a scrolling time × frequency image with the newest column on the
//! right, and the spectral view, which overlays the waveform on it as in spectral
//! editors. Both show one signal's levels, or two in lanes stacked one above the other.
//! Several shown signals (the input and buses) overlay their images, each tinted in its
//! color.

use aud_ui::theme::{TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::panel;
use egui::{
    Align2, Color32, ColorImage, Context, Id, Mesh, Painter, Pos2, Rect, Sense, Shape,
    TextureHandle, TextureOptions, Ui, pos2,
};

use crate::analysis::fft::FLOOR_DB;
use crate::analysis::spectrum::{SONOGRAM_ROWS, SonogramFeed, WaveExtent};
use crate::settings::{Signal, SonogramSettings, SpectralSettings, WaveScale};
use crate::style::{HeatStyle, SonogramStyle, SpectralStyle};
use crate::view::colormap::{Colors, Palette, lighten, rgb};
use crate::view::plot::{
    FreqAxis, Header, format_db, format_hz_precise, freq_grid_horizontal, lane_label, message,
    note_name, plot_background, tooltip,
};
use crate::view::sources::{NO_SIGNALS, Trace, name_traces};

/// Columns kept in the image.
const COLUMNS: usize = 1_024;
/// Lanes the image has room for.
const MAX_LANES: usize = 2;
/// Gap between two lanes in logical pixels.
const LANE_GAP: f32 = 6.0;
/// Lowest level of the waveform's dB scale, at the centre line.
const WAVE_FLOOR_DB: f32 = -60.0;
/// Share of the color scale that fades out in overlaid images.
const OVERLAY_FADE: f32 = 0.2;

/// The colored image of the levels.
#[derive(Default)]
struct Image {
    palette: Palette,
    /// Maps levels to the color scale.
    tone: SonogramSettings,
    /// White with the level as opacity, to be tinted, instead of the heat map.
    overlay: bool,
    /// One texture with the lanes one above the other.
    texture: Option<TextureHandle>,
    /// The context that owns `texture`. The view outlives the editor window, and a
    /// reopened window has a new context that does not know the old texture.
    context: Option<Context>,
    /// Columns written into the levels when this image was last uploaded.
    uploaded: u64,
    /// The whole image must be uploaded again.
    stale: bool,
}

/// The levels and the waveform as rings of columns, and their image. The sonogram and the
/// spectral view each have one, fed by their own analysis.
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
    image: Image,
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
            image: Image::default(),
        }
    }
}

/// Image height in texels: every lane's rows.
const IMAGE_ROWS: usize = MAX_LANES * SONOGRAM_ROWS;

impl SonogramView {
    /// Takes the new columns out of `feed` and writes them into the rings.
    pub fn take_columns(&mut self, feed: &mut SonogramFeed) {
        if feed.generation != self.generation {
            self.generation = feed.generation;
            self.levels.fill(FLOOR_DB);
            self.waves.fill([WaveExtent::default(); MAX_LANES]);
            self.image.stale = true;
        }
        for column in feed.columns.drain(..) {
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

    fn texel(&self, image: &Image, column: usize, lane: usize, row: usize) -> Color32 {
        let tone = image.tone.tone(self.level(column, lane, row));
        if image.overlay {
            let opacity = ((tone - OVERLAY_FADE) / (1.0 - OVERLAY_FADE)).clamp(0.0, 1.0);
            Color32::from_white_alpha((opacity * 255.0).round() as u8)
        } else {
            image.palette.image_color(tone)
        }
    }

    fn column_image(&self, image: &Image, column: usize) -> ColorImage {
        let mut pixels = ColorImage::new([1, IMAGE_ROWS], Color32::TRANSPARENT);
        for lane in 0..MAX_LANES {
            for row in 0..SONOGRAM_ROWS {
                pixels.pixels[Self::image_row(lane, row)] = self.texel(image, column, lane, row);
            }
        }
        pixels
    }

    fn full_image(&self, image: &Image) -> ColorImage {
        let mut pixels = ColorImage::new([COLUMNS, IMAGE_ROWS], Color32::TRANSPARENT);
        for column in 0..COLUMNS {
            for lane in 0..MAX_LANES {
                for row in 0..SONOGRAM_ROWS {
                    pixels.pixels[Self::image_row(lane, row) * COLUMNS + column] =
                        self.texel(image, column, lane, row);
                }
            }
        }
        pixels
    }

    /// Uploads what changed in the image, colored with `heat` and `tone`, or as a tintable
    /// `overlay`.
    fn update(&mut self, ui: &Ui, heat: HeatStyle, tone: &SonogramSettings, overlay: bool) {
        let toned = |settings: &SonogramSettings| {
            (settings.range, settings.brightness_db, settings.contrast)
        };
        {
            let image = &mut self.image;
            if image.context.as_ref() != Some(ui.ctx()) {
                image.context = Some(ui.ctx().clone());
                image.texture = None;
            }
            if heat != image.palette.heat() {
                image.palette = Palette::new(heat);
                image.stale = true;
            }
            if toned(tone) != toned(&image.tone) {
                image.tone = *tone;
                image.stale = true;
            }
            if overlay != image.overlay {
                image.overlay = overlay;
                image.stale = true;
            }
        }
        let options = TextureOptions::LINEAR;
        let image = &self.image;
        let missing = self.written - image.uploaded;
        if image.stale || image.texture.is_none() || missing >= COLUMNS as u64 {
            let pixels = self.full_image(image);
            let image = &mut self.image;
            match &mut image.texture {
                Some(texture) => texture.set(pixels, options),
                None => {
                    let texture = ui.ctx().load_texture("o-aud-a-sonogram", pixels, options);
                    image.texture = Some(texture);
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
        let image = &mut self.image;
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

    /// Draws one lane of the image, from the oldest column to the newest, across `plot`.
    fn draw_image(&self, painter: &Painter, plot: Rect, lane: usize, tint: Color32) {
        let Some(texture) = &self.image.texture else {
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

    /// The palette of the image, for color bars.
    fn palette(&self) -> &Palette {
        &self.image.palette
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

/// Inset of a lane's name tag in logical pixels.
const LABEL_INSET: f32 = 4.0;

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

fn freq_axis(feed: &SonogramFeed) -> Option<FreqAxis> {
    let (low_hz, high_hz) = feed.range_hz;
    (high_hz > low_hz).then_some(FreqAxis {
        low_hz,
        high_hz,
        scale: feed.scale,
    })
}

/// Header text variants, longest first.
fn info(settings: &SonogramSettings) -> [String; 3] {
    let [first, second] = settings.lane_names();
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

/// The title, the shown signals and the analysis settings in a panel's header.
fn header(
    painter: &Painter,
    rect: Rect,
    title: &str,
    traces: &[Trace<'_>],
    settings: &SonogramSettings,
) {
    let mut header = Header::new(painter, rect, title);
    name_traces(&mut header, traces);
    header.left_text(&info(settings), font(10.0), TEXT_DIM);
}

/// A message across the plot: no signals chosen, or no image yet of the only one.
fn waiting(painter: &Painter, plot: Rect, traces: &[Trace<'_>], feed: impl Fn(&Trace<'_>) -> u64) {
    let text = match traces {
        [] => NO_SIGNALS.to_owned(),
        [only] if feed(only) == 0 => only.waiting_text(),
        _ => return,
    };
    message(painter, plot, text);
}

/// Draws the sonogram panel of `traces`, each with its view, overlaid when there are
/// several.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    (views, traces): (&mut [&mut SonogramView], &[Trace<'_>]),
    settings: &SonogramSettings,
    (colors, style): (&Colors, &SonogramStyle),
    compact: bool,
) {
    panel(ui, rect, "SONOGRAM", colors.accent);
    let overlay = traces.len() > 1;
    let legend_width = if compact || overlay { 0.0 } else { 46.0 };
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 12.0 - legend_width, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    views
        .iter_mut()
        .for_each(|view| view.update(ui, style.heat, settings, overlay));
    let lanes = lane_rects(plot, views.first().map_or(1, |view| view.lanes));
    let names = settings.lane_names();
    let axis = traces
        .first()
        .and_then(|trace| freq_axis(&trace.signal.sonogram));
    for (index, lane) in lanes.iter().enumerate() {
        plot_background(&painter, *lane);
        for (view, trace) in views.iter().zip(traces) {
            let tint = if overlay { trace.color } else { Color32::WHITE };
            view.draw_image(&painter, *lane, index, tint);
        }
        if let Some(axis) = axis {
            freq_grid_horizontal(&painter, *lane, axis, true);
        }
        if lanes.len() > 1 {
            lane_label(&painter, *lane, names[index], LABEL_INSET);
        }
    }
    let span_s = span_s(settings);
    time_axis(&painter, plot, span_s);
    waiting(&painter, plot, traces, |trace| {
        trace.signal.sonogram.generation
    });
    header(&painter, rect, "SONOGRAM", traces, settings);
    let (Some(axis), Some(first)) = (axis, views.first()) else {
        return;
    };

    if !compact && !overlay {
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
            painter.rect_filled(slice, 0.0, first.palette().color(fraction));
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
        let row = (y_fraction * SONOGRAM_ROWS as f32) as usize;
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
        for (view, trace) in views.iter().zip(traces) {
            let level = format_db(view.level(view.column_at(x_fraction), lane, row), "dB");
            let (text, color) = match (overlay, lanes.len() > 1) {
                (true, _) => (format!("{} {level}", trace.name), trace.color),
                (false, true) => (format!("{} {level}", names[lane].to_lowercase()), TEXT),
                (false, false) => (level, TEXT),
            };
            lines.push((text, color));
        }
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

/// Draws the spectral view of `traces`: their own sonograms with the waveforms of the
/// same signals on top, overlaid in their colors when there are several.
pub fn draw_spectral(
    ui: &mut Ui,
    rect: Rect,
    (views, traces): (&mut [&mut SonogramView], &[Trace<'_>]),
    (sonogram, spectral): (&SonogramSettings, &SpectralSettings),
    (colors, style): (&Colors, &SpectralStyle),
) {
    panel(ui, rect, "SPECTRAL", colors.accent);
    let plot = Rect::from_min_max(
        pos2(rect.left() + 40.0, rect.top() + 34.0),
        pos2(rect.right() - 40.0, rect.bottom() - 24.0),
    );
    let painter = ui.painter_at(rect);
    let overlay = traces.len() > 1;
    views
        .iter_mut()
        .for_each(|view| view.update(ui, style.heat, sonogram, overlay));
    let image_opacity = SpectralSettings::OPACITY.clamp(spectral.sonogram_opacity);
    let wave_opacity = SpectralSettings::OPACITY.clamp(spectral.waveform_opacity);
    let gain_db = SpectralSettings::WAVEFORM_GAIN_DB.clamp(spectral.waveform_gain_db);
    let lanes = lane_rects(plot, views.first().map_or(1, |view| view.lanes));
    let names = sonogram.lane_names();
    let axis = traces
        .first()
        .and_then(|trace| freq_axis(&trace.signal.spectral));
    let second = if sonogram.signals().1 == Some(Signal::Side) {
        style.side
    } else {
        style.right
    };
    let lane_colors = [rgb(style.first), rgb(second)];
    // One signal keeps the lane colors; overlaid signals take their own, lighter in the
    // second lane.
    let wave_color = |trace: &Trace<'_>, lane: usize| match (overlay, lane) {
        (false, _) => lane_colors[lane],
        (true, 0) => trace.color,
        (true, _) => lighten(trace.color, 0.35),
    };
    for (index, lane) in lanes.iter().enumerate() {
        plot_background(&painter, *lane);
        if image_opacity > 0.0 {
            for (view, trace) in views.iter().zip(traces) {
                let tint = if overlay { trace.color } else { Color32::WHITE };
                view.draw_image(&painter, *lane, index, alpha(tint, image_opacity));
            }
        }
        if let Some(axis) = axis {
            freq_grid_horizontal(&painter, *lane, axis, true);
        }
        amplitude_scale(&painter, *lane, spectral.scale, gain_db);
        if wave_opacity > 0.0 {
            for (view, trace) in views.iter().zip(traces) {
                draw_waveform(
                    &painter,
                    *lane,
                    (view, index),
                    spectral,
                    (wave_color(trace, index), wave_opacity),
                );
            }
        }
        if lanes.len() > 1 {
            lane_label(&painter, *lane, names[index], LABEL_INSET);
        }
    }
    let span_s = span_s(sonogram);
    time_axis(&painter, plot, span_s);
    waiting(&painter, plot, traces, |trace| {
        trace.signal.spectral.generation
    });
    header(&painter, rect, "SPECTRAL", traces, sonogram);
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
        let ago = (1.0 - x_fraction.clamp(0.0, 1.0)) * span_s;
        let mut lines = vec![(format!("−{ago:.2} s"), TEXT)];
        if lanes.len() > 1 {
            lines.push((names[lane].to_owned(), lane_colors[lane]));
        }
        let row = (y_fraction * SONOGRAM_ROWS as f32) as usize;
        if let Some(axis) = axis {
            let hz = axis.hz(y_fraction);
            lines.push((
                format!("{}  {}", format_hz_precise(hz), note_name(hz)),
                TEXT_DIM,
            ));
        }
        let to_db = |value: f32| {
            if value > 1e-9 {
                20.0 * value.log10()
            } else {
                FLOOR_DB
            }
        };
        for (view, trace) in views.iter().zip(traces) {
            let column = view.column_at(x_fraction);
            let wave = view.waves[column][lane];
            let peak = wave.max.abs().max(wave.min.abs());
            let prefix = if overlay {
                format!("{} ", trace.name)
            } else {
                String::new()
            };
            let color = wave_color(trace, lane);
            if axis.is_some() {
                lines.push((
                    format!(
                        "{prefix}level {}",
                        format_db(view.level(column, lane, row), "dB")
                    ),
                    if overlay { color } else { TEXT },
                ));
            }
            lines.push((
                format!(
                    "{prefix}peak {}  rms {}",
                    format_db(to_db(peak), "dBFS"),
                    format_db(to_db(wave.rms), "dBFS")
                ),
                color,
            ));
        }
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
