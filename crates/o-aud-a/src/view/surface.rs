//! The 3D views: the spectrum history as ridges or a shaded surface, and the stereogram,
//! a surface of level over stereo position and frequency.

use aud_ui::theme::{TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::panel;
use egui::{
    Align2, Color32, CursorIcon, Id, Mesh, Painter, PointerButton, Pos2, Rect, Sense, Shape, Ui,
    pos2, vec2,
};

use crate::analysis::Published;
use crate::analysis::spectrum::{PAN_COLUMNS, SURFACE_RATE_HZ, SurfaceFrame};
use crate::settings::{SurfaceMode, SurfaceSettings};
use crate::style::SurfaceStyle;
use crate::view::colormap::{Colors, Palette, rgb};
use crate::view::plot::{Header, background, format_hz, grid, grid_strong, plot_background};

/// Height of a full-scale level in scene units; the floor spans −1..1 in x and z.
const HEIGHT: f32 = 0.7;
/// Camera distance from the scene centre in scene units.
const DISTANCE: f32 = 3.6;
/// Direction towards the light, normalized below.
const LIGHT: [f32; 3] = [-0.35, 0.85, -0.4];
/// Gap between two stacked lanes in logical pixels.
const LANE_GAP: f32 = 6.0;
/// Camera rotation per dragged pixel in degrees.
const DRAG_DEG_PER_PX: f32 = 0.35;
/// Zoom per scrolled pixel, as the natural logarithm of the factor.
const ZOOM_PER_SCROLL_PX: f32 = 0.002;

/// Projects scene points to the screen.
#[derive(Clone, Copy, Debug)]
struct Camera {
    yaw: (f32, f32),
    pitch: (f32, f32),
    focal: f32,
    centre: Pos2,
}

impl Camera {
    /// The camera of a scene drawn in `rect`, shifted by the pan as a fraction of `plot`.
    fn new(settings: &SurfaceSettings, rect: Rect, plot: Rect) -> Self {
        let pan = vec2(
            settings.pan_x * plot.width(),
            settings.pan_y * plot.height(),
        );
        Self {
            yaw: settings.yaw_deg.to_radians().sin_cos(),
            pitch: settings.pitch_deg.to_radians().sin_cos(),
            focal: (rect.width() * 1.05).min(rect.height() * 1.8) * settings.zoom,
            centre: rect.center() + vec2(0.0, rect.height() * 0.06) + pan,
        }
    }

    /// Screen position and distance from the camera of a scene point.
    fn project(&self, [x, y, z]: [f32; 3]) -> (Pos2, f32) {
        let (sin_yaw, cos_yaw) = self.yaw;
        let (sin_pitch, cos_pitch) = self.pitch;
        let x1 = x * cos_yaw - z * sin_yaw;
        let z1 = x * sin_yaw + z * cos_yaw;
        let y1 = y - 0.3;
        let y2 = y1 * cos_pitch + z1 * sin_pitch;
        let depth = DISTANCE - y1 * sin_pitch + z1 * cos_pitch;
        let scale = self.focal / depth.max(0.1);
        (self.centre + vec2(x1 * scale, -y2 * scale), depth)
    }

    fn screen(&self, point: [f32; 3]) -> Pos2 {
        self.project(point).0
    }
}

/// Scratch buffers kept between frames.
#[derive(Default)]
pub struct SurfaceView {
    points: Vec<[f32; 3]>,
    colors: Vec<Color32>,
    quads: Vec<(f32, u32)>,
    palette: Palette,
}

/// Draws the 3D panel.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut SurfaceView,
    data: &Published,
    settings: &mut SurfaceSettings,
    (colors, style, lane_names): (&Colors, &SurfaceStyle, [&str; 2]),
) {
    let title = match settings.mode {
        SurfaceMode::Stereogram => "3D STEREOGRAM",
        _ => "3D SPECTRUM",
    };
    panel(ui, rect, title, colors.accent);
    let plot = Rect::from_min_max(rect.min + vec2(12.0, 34.0), rect.max - vec2(12.0, 12.0));
    let painter = ui.painter_at(plot);
    plot_background(&painter, plot);
    Header::new(&ui.painter_at(rect), rect, title).right_text(
        &[
            String::from(
                "drag to rotate · shift- or right-drag to move · wheel to zoom · \
                 double-click to reset",
            ),
            String::from("drag · shift-drag · wheel · double-click"),
        ],
        font(10.0),
        TEXT_DIM,
    );
    camera_input(ui, plot, settings);

    if view.palette.heat() != style.heat {
        view.palette = Palette::new(style.heat);
    }
    let surface = &data.surface;
    if surface.rows.is_empty() || surface.bands < 2 {
        painter.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Waiting for audio",
            font(12.0),
            TEXT_DIM,
        );
        return;
    }
    let floor_db = -settings.range;
    let level = |db: f32| ((db - floor_db) / -floor_db).clamp(0.0, 1.0);

    match settings.mode {
        SurfaceMode::Ridges | SurfaceMode::Surface => {
            // One scene per lane, stacked, each with the same camera.
            let lanes = surface.lanes.clamp(1, 2);
            let height = (plot.height() - LANE_GAP * (lanes - 1) as f32) / lanes as f32;
            for (lane, name) in lane_names.iter().take(lanes).enumerate() {
                let top = plot.top() + lane as f32 * (height + LANE_GAP);
                let lane_rect =
                    Rect::from_min_max(pos2(plot.left(), top), pos2(plot.right(), top + height));
                let camera = Camera::new(settings, lane_rect, plot);
                let clipped = painter.with_clip_rect(lane_rect.shrink(1.0));
                draw_floor(&clipped, &camera);
                if settings.grid {
                    draw_grid(&clipped, &camera, settings.range);
                }
                draw_spectrum_lane(&clipped, &camera, view, (surface, lane), settings, &level);
                if lanes > 1 {
                    lane_label(&painter, lane_rect, name);
                    if lane > 0 {
                        painter.hline(
                            plot.x_range(),
                            top - LANE_GAP * 0.5,
                            stroke(1.0, alpha(TEXT_DIM, 0.4)),
                        );
                    }
                }
            }
        }
        SurfaceMode::Stereogram => {
            let camera = Camera::new(settings, plot, plot);
            let clipped = painter.with_clip_rect(plot.shrink(1.0));
            draw_floor(&clipped, &camera);
            if settings.grid {
                draw_grid(&clipped, &camera, settings.range);
            }
            let bands = surface.bands;
            if surface.stereo_db.len() != bands * PAN_COLUMNS {
                return;
            }
            draw_stereogram_axes(&clipped, &camera, surface.range_hz);
            view.points.clear();
            view.colors.clear();
            for band in 0..bands {
                let z = -1.0 + 2.0 * band as f32 / (bands - 1) as f32;
                for column in 0..PAN_COLUMNS {
                    let cell = band * PAN_COLUMNS + column;
                    let value = level(surface.stereo_db[cell]);
                    let x = -1.0 + 2.0 * column as f32 / (PAN_COLUMNS - 1) as f32;
                    view.points.push([x, value * HEIGHT, z]);
                    let base = view.palette.color(value);
                    let opposite =
                        (-surface.stereo_correlation[cell]).clamp(0.0, 1.0) * value.sqrt();
                    view.colors
                        .push(mix(base, rgb(style.opposite), 0.8 * opposite));
                }
            }
            let mesh = shaded_mesh(&camera, view, PAN_COLUMNS, bands);
            clipped.add(Shape::mesh(mesh));
        }
    }
    painter.text(
        plot.left_bottom() + vec2(8.0, -6.0),
        Align2::LEFT_BOTTOM,
        format!(
            "yaw {:.0}° · pitch {:.0}° · zoom {:.1}×",
            settings.yaw_deg, settings.pitch_deg, settings.zoom
        ),
        mono(9.0),
        TEXT_FAINT,
    );
}

/// Moves the camera: a drag rotates the scene with the pointer, a drag with the shift
/// key or the right or middle button moves it, the wheel (or a pinch) zooms and a
/// double click resets the view.
fn camera_input(ui: &mut Ui, plot: Rect, settings: &mut SurfaceSettings) {
    let response = ui
        .interact(plot, Id::new("surface-plot"), Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::Grab);
    if response.double_clicked() {
        settings.reset_camera();
    }
    let shift = ui.input(|input| input.modifiers.shift);
    let delta = response.drag_delta();
    let panning = response.dragged_by(PointerButton::Secondary)
        || response.dragged_by(PointerButton::Middle)
        || (shift && response.dragged_by(PointerButton::Primary));
    if panning {
        let limit = SurfaceSettings::PAN_LIMIT;
        settings.pan_x = (settings.pan_x + delta.x / plot.width().max(1.0)).clamp(-limit, limit);
        settings.pan_y = (settings.pan_y + delta.y / plot.height().max(1.0)).clamp(-limit, limit);
    } else if response.dragged_by(PointerButton::Primary) {
        // The scene turns the way the pointer moves.
        settings.yaw_deg =
            (settings.yaw_deg + delta.x * DRAG_DEG_PER_PX + 180.0).rem_euclid(360.0) - 180.0;
        let (low, high) = SurfaceSettings::PITCH_RANGE_DEG;
        settings.pitch_deg = (settings.pitch_deg - delta.y * DRAG_DEG_PER_PX).clamp(low, high);
    }
    if response.hovered() {
        let (scroll, pinch) = ui.input(|input| (input.smooth_scroll_delta.y, input.zoom_delta()));
        let factor = pinch * (scroll * ZOOM_PER_SCROLL_PX).exp();
        if factor != 1.0 {
            settings.zoom = SurfaceSettings::ZOOM.clamp(settings.zoom * factor);
        }
    }
}

/// One lane's spectrum history as ridges or a shaded surface.
fn draw_spectrum_lane(
    clipped: &Painter,
    camera: &Camera,
    view: &mut SurfaceView,
    (surface, lane): (&SurfaceFrame, usize),
    settings: &SurfaceSettings,
    level: &dyn Fn(f32) -> f32,
) {
    let bands = surface.bands;
    let depth = settings.depth.count();
    let rows = surface.rows.len().min(depth);
    let skip = surface.rows.len() - rows;
    let band_range = lane * bands..(lane + 1) * bands;
    let z_of = |row: usize| -1.0 + 2.0 * (rows - 1 - row) as f32 / (depth - 1) as f32;
    let x_of = |band: usize| -1.0 + 2.0 * band as f32 / (bands - 1) as f32;
    draw_spectrum_axes(clipped, camera, surface.range_hz, depth);
    if settings.mode == SurfaceMode::Surface {
        view.points.clear();
        view.colors.clear();
        for (row, levels) in surface.rows.iter().skip(skip).enumerate() {
            let age = (rows - 1 - row) as f32 / (depth - 1) as f32;
            for (band, db) in levels[band_range.clone()].iter().enumerate() {
                let value = level(*db);
                view.points.push([x_of(band), value * HEIGHT, z_of(row)]);
                let color = view.palette.color(value);
                view.colors.push(fade(color, 1.0 - 0.35 * age));
            }
        }
        let mesh = shaded_mesh(camera, view, bands, rows);
        clipped.add(Shape::mesh(mesh));
        return;
    }
    let mut order: Vec<(f32, usize)> = (0..rows)
        .map(|row| (camera.project([0.0, 0.0, z_of(row)]).1, row))
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, row) in order {
        let levels = &surface.rows[skip + row][band_range.clone()];
        let z = z_of(row);
        let recent = row as f32 / (rows.max(2) - 1) as f32;
        let points: Vec<Pos2> = levels
            .iter()
            .enumerate()
            .map(|(band, db)| camera.screen([x_of(band), level(*db) * HEIGHT, z]))
            .collect();
        let base: Vec<Pos2> = (0..bands)
            .map(|band| camera.screen([x_of(band), 0.0, z]))
            .collect();
        // Ridges hide the ones behind them with the diagram background.
        let hide = alpha(background(clipped), 0.94);
        let mut fill = Mesh::default();
        for (index, (top, bottom)) in points.iter().zip(&base).enumerate() {
            fill.colored_vertex(*top, hide);
            fill.colored_vertex(*bottom, hide);
            if index > 0 {
                let vertex = 2 * index as u32;
                fill.add_triangle(vertex - 2, vertex - 1, vertex);
                fill.add_triangle(vertex - 1, vertex + 1, vertex);
            }
        }
        clipped.add(Shape::mesh(fill));
        let color = view.palette.color(0.45 + 0.55 * recent);
        clipped.add(Shape::line(points, stroke(1.0 + 0.6 * recent, color)));
    }
}

/// A lane's name in its top left corner.
fn lane_label(painter: &Painter, lane: Rect, name: &str) {
    let galley = painter.layout_no_wrap(name.to_owned(), font(10.0), TEXT);
    let tag = Rect::from_min_size(lane.min + vec2(6.0, 6.0), galley.size() + vec2(10.0, 4.0));
    painter.rect_filled(tag, 3.0, alpha(Color32::BLACK, 0.55));
    painter.galley(tag.min + vec2(5.0, 2.0), galley, TEXT);
}

/// `color` darkened by `factor` (one keeps it).
fn fade(color: Color32, factor: f32) -> Color32 {
    let [r, g, b, a] = color.to_array();
    let scale = |value: u8| (f32::from(value) * factor.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(scale(r), scale(g), scale(b), a)
}

/// Blend from `from` to `to` by `amount` (0..1).
fn mix(from: Color32, to: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let [r0, g0, b0, _] = from.to_array();
    let [r1, g1, b1, _] = to.to_array();
    let blend =
        |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
    Color32::from_rgb(blend(r0, r1), blend(g0, g1), blend(b0, b1))
}

/// A shaded mesh of the `columns × rows` grid in `view.points` and `view.colors`, with
/// its quads sorted from far to near.
fn shaded_mesh(camera: &Camera, view: &mut SurfaceView, columns: usize, rows: usize) -> Mesh {
    let mut mesh = Mesh::default();
    if columns < 2 || rows < 2 {
        return mesh;
    }
    let length = (LIGHT[0] * LIGHT[0] + LIGHT[1] * LIGHT[1] + LIGHT[2] * LIGHT[2]).sqrt();
    let light = LIGHT.map(|value| value / length);
    let points = &view.points;
    let mut depths = Vec::with_capacity(points.len());
    for row in 0..rows {
        for column in 0..columns {
            let index = row * columns + column;
            let point = points[index];
            let (screen, depth) = camera.project(point);
            depths.push(depth);
            // Normal from the neighbouring heights.
            let left = points[row * columns + column.saturating_sub(1)];
            let right = points[row * columns + (column + 1).min(columns - 1)];
            let back = points[row.saturating_sub(1) * columns + column];
            let front = points[(row + 1).min(rows - 1) * columns + column];
            let dx = [right[0] - left[0], right[1] - left[1], right[2] - left[2]];
            let dz = [front[0] - back[0], front[1] - back[1], front[2] - back[2]];
            let normal = [
                dx[1] * dz[2] - dx[2] * dz[1],
                dx[2] * dz[0] - dx[0] * dz[2],
                dx[0] * dz[1] - dx[1] * dz[0],
            ];
            let norm = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
                .sqrt()
                .max(1e-9);
            let facing =
                ((normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]) / norm).abs();
            mesh.colored_vertex(screen, fade(view.colors[index], 0.45 + 0.55 * facing));
        }
    }
    view.quads.clear();
    for row in 0..rows - 1 {
        for column in 0..columns - 1 {
            let corner = row * columns + column;
            let depth = (depths[corner]
                + depths[corner + 1]
                + depths[corner + columns]
                + depths[corner + columns + 1])
                * 0.25;
            view.quads.push((depth, corner as u32));
        }
    }
    view.quads.sort_by(|a, b| b.0.total_cmp(&a.0));
    let stride = columns as u32;
    for (_, corner) in &view.quads {
        let (a, b, c, d) = (*corner, corner + 1, corner + stride, corner + stride + 1);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(b, d, c);
    }
    mesh
}

/// The floor outline.
fn draw_floor(painter: &Painter, camera: &Camera) {
    let corners = [
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [-1.0, 0.0, -1.0],
    ];
    let outline: Vec<Pos2> = corners.iter().map(|point| camera.screen(*point)).collect();
    painter.add(Shape::line(outline, stroke(1.0, grid_strong(painter))));
    let axis = [
        camera.screen([-1.0, 0.0, 1.0]),
        camera.screen([-1.0, HEIGHT, 1.0]),
    ];
    painter.line_segment(axis, stroke(1.0, grid_strong(painter)));
}

/// Grid divisions along each side of the floor and up the walls.
const GRID_DIVISIONS: usize = 4;

/// A grid on the floor and on the two walls behind the scene, the ones farther from the
/// camera, with level lines labelled in dB every 10, 20 or 30 dB down from 0 dB; the
/// floor is at `-range_db`.
fn draw_grid(painter: &Painter, camera: &Camera, range_db: f32) {
    let line = stroke(1.0, alpha(grid(painter), 0.8));
    let steps =
        || (0..=GRID_DIVISIONS).map(|step| -1.0 + 2.0 * step as f32 / GRID_DIVISIONS as f32);
    let depth = |point: [f32; 3]| camera.project(point).1;
    let middle = HEIGHT * 0.5;
    let wall_x = if depth([-1.0, middle, 0.0]) > depth([1.0, middle, 0.0]) {
        -1.0
    } else {
        1.0
    };
    let wall_z = if depth([0.0, middle, -1.0]) > depth([0.0, middle, 1.0]) {
        -1.0
    } else {
        1.0
    };
    let segment = |from: [f32; 3], to: [f32; 3]| {
        painter.line_segment([camera.screen(from), camera.screen(to)], line);
    };
    for along in steps() {
        // Floor lines in both directions.
        segment([along, 0.0, -1.0], [along, 0.0, 1.0]);
        segment([-1.0, 0.0, along], [1.0, 0.0, along]);
        // Vertical lines up the back walls.
        segment([wall_x, 0.0, along], [wall_x, HEIGHT, along]);
        segment([along, 0.0, wall_z], [along, HEIGHT, wall_z]);
    }
    // Level lines on round steps down from 0 dB at the top.
    let step_db = if range_db <= 60.0 {
        10.0
    } else if range_db <= 120.0 {
        20.0
    } else {
        30.0
    };
    let levels = (0..)
        .map(|step| -(step as f32) * step_db)
        .take_while(|db| *db > -range_db);
    for db in levels {
        let fraction = 1.0 + db / range_db;
        let y = HEIGHT * fraction;
        segment([wall_x, y, -1.0], [wall_x, y, 1.0]);
        segment([-1.0, y, wall_z], [1.0, y, wall_z]);
        // The level at the corner where the walls meet.
        let corner = camera.screen([wall_x, y, wall_z]);
        let (offset, align) = if corner.x < camera.centre.x {
            (vec2(-6.0, 0.0), Align2::RIGHT_CENTER)
        } else {
            (vec2(6.0, 0.0), Align2::LEFT_CENTER)
        };
        painter.text(
            corner + offset,
            align,
            format!("{db:.0}"),
            mono(9.0),
            TEXT_FAINT,
        );
    }
}

/// Frequency lines across the floor and the time axis of the spectrum history.
fn draw_spectrum_axes(painter: &Painter, camera: &Camera, (low, high): (f32, f32), depth: usize) {
    if high <= low {
        return;
    }
    for hz in [
        50.0, 100.0, 200.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 10_000.0, 20_000.0,
    ] {
        if hz < low || hz > high {
            continue;
        }
        let x = -1.0 + 2.0 * (hz / low).ln() / (high / low).ln();
        let front = camera.screen([x, 0.0, -1.0]);
        let back = camera.screen([x, 0.0, 1.0]);
        painter.line_segment([front, back], stroke(1.0, alpha(grid_strong(painter), 0.6)));
        painter.text(
            front + vec2(0.0, 4.0),
            Align2::CENTER_TOP,
            format_hz(hz),
            mono(9.0),
            TEXT_FAINT,
        );
    }
    let span_s = depth as f32 / SURFACE_RATE_HZ;
    let now = camera.screen([1.0, 0.0, -1.0]);
    let then = camera.screen([1.0, 0.0, 1.0]);
    painter.text(
        now + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        "now",
        mono(9.0),
        TEXT_FAINT,
    );
    painter.text(
        then + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        format!("−{span_s:.1} s"),
        mono(9.0),
        TEXT_FAINT,
    );
}

/// Pan lines and frequency lines of the stereogram floor.
fn draw_stereogram_axes(painter: &Painter, camera: &Camera, (low, high): (f32, f32)) {
    for (x, label) in [(-1.0, "L"), (0.0, "C"), (1.0, "R")] {
        let front = camera.screen([x, 0.0, -1.0]);
        let back = camera.screen([x, 0.0, 1.0]);
        painter.line_segment([front, back], stroke(1.0, alpha(grid_strong(painter), 0.6)));
        painter.text(
            front + vec2(0.0, 4.0),
            Align2::CENTER_TOP,
            label,
            font(11.0),
            TEXT_DIM,
        );
    }
    if high <= low {
        return;
    }
    for hz in [100.0, 1_000.0, 10_000.0] {
        let z = -1.0 + 2.0 * (hz / low).ln() / (high / low).ln();
        let left = camera.screen([-1.0, 0.0, z]);
        let right = camera.screen([1.0, 0.0, z]);
        painter.line_segment([left, right], stroke(1.0, alpha(grid_strong(painter), 0.6)));
        painter.text(
            left + vec2(-6.0, 0.0),
            Align2::RIGHT_CENTER,
            format_hz(hz),
            mono(9.0),
            TEXT_FAINT,
        );
    }
}
