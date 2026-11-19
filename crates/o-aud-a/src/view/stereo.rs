//! The stereo imager: a goniometer, correlation and balance meters and the stereo image
//! per frequency band.

use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_4, PI};

use aud_ui::theme::{BORDER, METER_WARN, TEXT, TEXT_DIM, TEXT_FAINT, alpha, font, mono, stroke};
use aud_ui::widgets::{bar_meter, bipolar_meter, panel};
use egui::{
    Align2, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Shape, StrokeKind, Ui, pos2, vec2,
};

use crate::analysis::Published;
use crate::analysis::stereo::{STEREO_BANDS, stereo_band_hz};
use crate::settings::{ScopeGain, ScopeMode, StereoSettings};
use crate::style::StereoStyle;
use crate::view::colormap::{Colors, rgb};
use crate::view::plot::{format_hz, grid, grid_strong, plot_background};

/// Scope frames kept as a fading trail.
const TRAIL: usize = 4;
/// Highest automatic scope gain.
const MAX_GAIN: f32 = 1_000.0;
/// Automatic gain rise per frame, as a factor.
const GAIN_RISE: f32 = 1.03;

/// Scope state kept between frames.
pub struct StereoView {
    gain: f32,
    /// Recent frames' points in scope coordinates, oldest first.
    trail: VecDeque<Vec<[f32; 2]>>,
    last_sequence: u64,
}

impl Default for StereoView {
    fn default() -> Self {
        Self {
            gain: 1.0,
            trail: VecDeque::new(),
            last_sequence: u64::MAX,
        }
    }
}

/// Draws the stereo panel.
pub fn draw(
    ui: &mut Ui,
    rect: Rect,
    view: &mut StereoView,
    data: &Published,
    settings: &mut StereoSettings,
    (colors, style): (&Colors, &StereoStyle),
    compact: bool,
    frozen: bool,
) {
    panel(ui, rect, "STEREO", colors.accent);
    let content = Rect::from_min_max(rect.min + vec2(12.0, 34.0), rect.max - vec2(12.0, 12.0));
    let painter = ui.painter_at(rect);
    if compact {
        let meters_height = 76.0;
        let scope_side = (content.height() - meters_height - 8.0).min(content.width());
        let scope = Rect::from_center_size(
            pos2(content.center().x, content.top() + scope_side * 0.5),
            vec2(scope_side, scope_side),
        );
        draw_scope(&painter, scope, view, data, (settings, style), frozen);
        let meters = Rect::from_min_max(
            pos2(content.left(), content.bottom() - meters_height),
            content.max,
        );
        draw_meters(ui, meters, data, style, true);
        return;
    }
    let scope_side = content.height().min(content.width() * 0.48);
    let scope = Rect::from_min_size(content.min, vec2(scope_side, scope_side));
    draw_scope(&painter, scope, view, data, (settings, style), frozen);
    let side = Rect::from_min_max(pos2(scope.right() + 20.0, content.top()), content.max);
    let meters = Rect::from_min_size(side.min, vec2(side.width(), 150.0));
    draw_meters(ui, meters, data, style, false);
    let image = Rect::from_min_max(
        pos2(side.left() + 36.0, meters.bottom() + 34.0),
        pos2(side.right(), side.bottom() - 18.0),
    );
    painter.text(
        pos2(side.left(), meters.bottom() + 16.0),
        Align2::LEFT_CENTER,
        "IMAGE BY FREQUENCY",
        font(10.0),
        TEXT_DIM,
    );
    painter.text(
        pos2(side.right(), meters.bottom() + 16.0),
        Align2::RIGHT_CENTER,
        "position · width · opposite phase",
        font(10.0),
        TEXT_FAINT,
    );
    draw_band_image(&painter, image, data, style);
}

/// Scope coordinates of a frame: side across, mid up, full scale on the diamond's edge.
fn mid_side([left, right]: [f32; 2]) -> [f32; 2] {
    [0.5 * (right - left), 0.5 * (left + right)]
}

fn draw_scope(
    painter: &Painter,
    rect: Rect,
    view: &mut StereoView,
    data: &Published,
    (settings, style): (&StereoSettings, &StereoStyle),
    frozen: bool,
) {
    plot_background(painter, rect);
    let count = settings.samples.count().min(data.scope.len());
    let frames = &data.scope[data.scope.len() - count..];
    if !frozen && data.sequence != view.last_sequence {
        view.last_sequence = data.sequence;
        let points: Vec<[f32; 2]> = frames
            .iter()
            .map(|frame| match settings.mode {
                ScopeMode::Xy => *frame,
                _ => mid_side(*frame),
            })
            .collect();
        let peak = points
            .iter()
            .map(|[x, y]| match settings.mode {
                ScopeMode::Xy => x.abs().max(y.abs()),
                ScopeMode::Polar => x.hypot(*y),
                _ => x.abs() + y.abs(),
            })
            .fold(0.0, f32::max);
        let target = match settings.gain {
            ScopeGain::Unity => 1.0,
            ScopeGain::Auto if peak > 1e-6 => (0.85 / peak).clamp(1.0, MAX_GAIN),
            ScopeGain::Auto => view.gain,
        };
        view.gain = if target < view.gain || settings.gain == ScopeGain::Unity {
            target
        } else {
            (view.gain * GAIN_RISE).min(target)
        };
        if view.trail.len() >= TRAIL {
            view.trail.pop_front();
        }
        view.trail.push_back(points);
    }

    let centre = rect.center();
    let radius = rect.width() * 0.5 - 10.0;
    let gain = view.gain;
    let base = pos2(centre.x, rect.bottom() - 14.0);
    let polar_radius = (rect.width() * 0.5 - 10.0).min(rect.height() - 28.0);
    let to_screen = |[x, y]: [f32; 2]| -> Pos2 {
        match settings.mode {
            ScopeMode::Polar => {
                let (x, y) = if y < 0.0 { (-x, -y) } else { (x, y) };
                base + vec2(x, -y) * gain * polar_radius
            }
            _ => centre + vec2(x, -y) * gain * radius,
        }
    };

    // Guides.
    match settings.mode {
        ScopeMode::Polar => {
            let arc: Vec<Pos2> = (0..=48)
                .map(|step| {
                    let angle = PI * step as f32 / 48.0;
                    base + vec2(angle.cos(), -angle.sin()) * polar_radius
                })
                .collect();
            painter.add(Shape::line(arc, stroke(1.0, grid_strong(painter))));
            for (angle, label) in [(3.0 * FRAC_PI_4, "L"), (PI * 0.5, "M"), (FRAC_PI_4, "R")] {
                let end = base + vec2(angle.cos(), -angle.sin()) * polar_radius;
                painter.line_segment([base, end], stroke(1.0, grid(painter)));
                painter.text(
                    end + vec2(angle.cos(), -angle.sin()) * 8.0,
                    Align2::CENTER_CENTER,
                    label,
                    font(10.0),
                    TEXT_DIM,
                );
            }
            painter.hline(
                (base.x - polar_radius)..=(base.x + polar_radius),
                base.y,
                stroke(1.0, grid(painter)),
            );
        }
        ScopeMode::Xy => {
            let square = Rect::from_center_size(centre, vec2(2.0 * radius, 2.0 * radius));
            painter.rect_stroke(
                square,
                CornerRadius::ZERO,
                stroke(1.0, grid_strong(painter)),
                StrokeKind::Middle,
            );
            painter.hline(square.x_range(), centre.y, stroke(1.0, grid(painter)));
            painter.vline(centre.x, square.y_range(), stroke(1.0, grid(painter)));
            painter.line_segment(
                [square.left_bottom(), square.right_top()],
                stroke(1.0, grid(painter)),
            );
            painter.text(
                pos2(square.right() - 4.0, centre.y - 8.0),
                Align2::RIGHT_CENTER,
                "L",
                font(10.0),
                TEXT_DIM,
            );
            painter.text(
                pos2(centre.x + 8.0, square.top() + 6.0),
                Align2::LEFT_CENTER,
                "R",
                font(10.0),
                TEXT_DIM,
            );
        }
        _ => {
            let diamond = vec![
                centre + vec2(0.0, -radius),
                centre + vec2(radius, 0.0),
                centre + vec2(0.0, radius),
                centre + vec2(-radius, 0.0),
                centre + vec2(0.0, -radius),
            ];
            painter.add(Shape::line(diamond, stroke(1.0, grid_strong(painter))));
            painter.vline(
                centre.x,
                (centre.y - radius)..=(centre.y + radius),
                stroke(1.0, grid(painter)),
            );
            painter.hline(
                (centre.x - radius)..=(centre.x + radius),
                centre.y,
                stroke(1.0, grid(painter)),
            );
            let diagonal = radius * 0.5;
            painter.line_segment(
                [
                    centre + vec2(-diagonal, -diagonal),
                    centre + vec2(diagonal, diagonal),
                ],
                stroke(1.0, grid(painter)),
            );
            painter.line_segment(
                [
                    centre + vec2(diagonal, -diagonal),
                    centre + vec2(-diagonal, diagonal),
                ],
                stroke(1.0, grid(painter)),
            );
            for (offset, label) in [
                (vec2(0.0, -radius - 2.0), "M"),
                (vec2(-diagonal - 8.0, -diagonal - 8.0), "L"),
                (vec2(diagonal + 8.0, -diagonal - 8.0), "R"),
                (vec2(radius + 2.0, 0.0), "+S"),
                (vec2(-radius - 2.0, 0.0), "−S"),
            ] {
                painter.text(
                    centre + offset,
                    Align2::CENTER_BOTTOM,
                    label,
                    font(10.0),
                    TEXT_DIM,
                );
            }
        }
    }

    let clipped = painter.with_clip_rect(rect.shrink(1.0));
    let count = view.trail.len();
    let scope_color = rgb(style.scope.color);
    let scope_opacity = style.scope.opacity.clamp(0.0, 1.0);
    for (age, points) in view.trail.iter().enumerate() {
        let fade = (age + 1) as f32 / count.max(1) as f32;
        if settings.lines {
            let line: Vec<Pos2> = points.iter().map(|point| to_screen(*point)).collect();
            clipped.add(Shape::line(
                line,
                stroke(1.0, alpha(scope_color, 0.75 * scope_opacity * fade * fade)),
            ));
        } else {
            let color = alpha(scope_color, scope_opacity * fade * fade);
            let mut mesh = Mesh::default();
            for point in points {
                let position = to_screen(*point);
                mesh.add_colored_rect(Rect::from_center_size(position, vec2(1.6, 1.6)), color);
            }
            clipped.add(Shape::mesh(mesh));
        }
    }
    let gain_db = 20.0 * gain.log10();
    painter.text(
        rect.right_bottom() + vec2(-6.0, -4.0),
        Align2::RIGHT_BOTTOM,
        format!("{gain_db:+.0} dB"),
        mono(9.0),
        TEXT_FAINT,
    );
}

fn draw_meters(ui: &Ui, rect: Rect, data: &Published, style: &StereoStyle, compact: bool) {
    let stereo = &data.stereo;
    let correlation_color = if stereo.correlation < 0.0 {
        rgb(style.opposite)
    } else if stereo.correlation < 0.3 {
        METER_WARN
    } else {
        rgb(style.mid)
    };
    let row = 36.0;
    bipolar_meter(
        ui,
        Rect::from_min_size(rect.min, vec2(rect.width(), 22.0)),
        stereo.correlation,
        1.0,
        correlation_color,
        "Correlation",
        &format!("{:+.2}", stereo.correlation),
    );
    let balance = if stereo.balance_db.abs() < 0.05 {
        String::from("centre")
    } else if stereo.balance_db > 0.0 {
        format!("R {:.1} dB", stereo.balance_db)
    } else {
        format!("L {:.1} dB", -stereo.balance_db)
    };
    bipolar_meter(
        ui,
        Rect::from_min_size(rect.min + vec2(0.0, row), vec2(rect.width(), 22.0)),
        stereo.balance_db,
        12.0,
        rgb(style.balance),
        "Balance",
        &balance,
    );
    if compact {
        return;
    }
    let level = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let text = |db: f32| {
        if db < -150.0 {
            String::from("–∞ dB")
        } else {
            format!("{db:.1} dB")
        }
    };
    bar_meter(
        ui,
        Rect::from_min_size(rect.min + vec2(0.0, 2.0 * row), vec2(rect.width(), 22.0)),
        level(stereo.mid_db),
        None,
        rgb(style.mid),
        "Mid level (RMS)",
        &text(stereo.mid_db),
    );
    bar_meter(
        ui,
        Rect::from_min_size(rect.min + vec2(0.0, 3.0 * row), vec2(rect.width(), 22.0)),
        level(stereo.side_db),
        None,
        rgb(style.side),
        "Side level (RMS)",
        &text(stereo.side_db),
    );
    let ratio = stereo.side_db - stereo.mid_db;
    let width = if stereo.mid_db < -150.0 && stereo.side_db < -150.0 {
        String::from("–")
    } else if stereo.side_db < -150.0 {
        String::from("mono")
    } else {
        format!("{ratio:+.1} dB")
    };
    ui.painter().text(
        rect.left_bottom() + vec2(0.0, -2.0),
        Align2::LEFT_BOTTOM,
        format!("Side to mid  {width}"),
        mono(11.0),
        TEXT,
    );
}

fn draw_band_image(painter: &Painter, rect: Rect, data: &Published, style: &StereoStyle) {
    plot_background(painter, rect);
    let bands = &data.spectrum.bands;
    let row = rect.height() / STEREO_BANDS as f32;
    painter.vline(
        rect.center().x,
        rect.y_range(),
        stroke(1.0, grid_strong(painter)),
    );
    for fraction in [0.25, 0.75] {
        painter.vline(
            rect.left() + rect.width() * fraction,
            rect.y_range(),
            stroke(1.0, grid(painter)),
        );
    }
    for band in 0..STEREO_BANDS {
        let hz = stereo_band_hz(band);
        let y = rect.bottom() - (band as f32 + 0.5) * row;
        if band % 3 == 1 {
            painter.text(
                pos2(rect.left() - 4.0, y),
                Align2::RIGHT_CENTER,
                format_hz(hz),
                mono(9.0),
                TEXT_FAINT,
            );
        }
        let level = bands.level_db[band];
        if level < -100.0 {
            continue;
        }
        let presence = ((level + 80.0) / 70.0).clamp(0.1, 1.0);
        let correlation = bands.correlation[band];
        let spread = (1.0 - correlation).clamp(0.02, 2.0) * 0.5;
        let centre = rect.center().x + bands.pan[band] * rect.width() * 0.5;
        let half = spread * rect.width() * 0.5;
        let color = if correlation < 0.0 {
            rgb(style.opposite)
        } else {
            rgb(style.bands.color)
        };
        painter.rect_filled(
            Rect::from_min_max(
                pos2((centre - half).max(rect.left()), y - row * 0.36),
                pos2((centre + half).min(rect.right()), y + row * 0.36),
            ),
            CornerRadius::same(2),
            alpha(color, style.bands.opacity * (0.3 + 0.7 * presence)),
        );
        painter.circle_filled(pos2(centre, y), 2.0, alpha(Color32::WHITE, presence));
    }
    for (fraction, label) in [(0.0, "L"), (0.5, "C"), (1.0, "R")] {
        painter.text(
            pos2(rect.left() + rect.width() * fraction, rect.bottom() + 3.0),
            Align2::CENTER_TOP,
            label,
            font(10.0),
            TEXT_DIM,
        );
    }
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        stroke(1.0, BORDER),
        StrokeKind::Inside,
    );
}
