//! The overview: the chosen views in rows. Each view can be shown or hidden, moved by
//! dragging its title and resized by dragging the gaps around it.

use aud_ui::theme::{BORDER, PANEL_RAISED, TEXT, TEXT_DIM, alpha, font, stroke};
use egui::{Align2, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, StrokeKind, Ui, pos2, vec2};

use crate::layout::{DropZone, MIN_WEIGHT, Module, OverviewLayout};
use crate::settings::AnalyzerSettings;
use crate::view::colormap::Colors;
use crate::view::{Actions, GAP, Toolbar, ViewState, draw_module};

/// Smallest module size in logical pixels while resizing.
const MIN_MODULE_WIDTH: f32 = 200.0;
const MIN_MODULE_HEIGHT: f32 = 140.0;
/// Height of the draggable title strip.
const TITLE_HEIGHT: f32 = 30.0;
/// Panels narrower or lower than this draw their compact version.
const COMPACT_WIDTH: f32 = 560.0;
const COMPACT_HEIGHT: f32 = 300.0;

/// Overview interaction kept between frames.
#[derive(Default)]
pub struct OverviewState {
    /// The module whose title is being dragged.
    dragging: Option<Module>,
}

/// A module's place on screen.
struct Cell {
    module: Module,
    rect: Rect,
}

/// Gaps closer than this to a gap in the row above line up with it, in logical pixels.
const SNAP_DISTANCE: f32 = 8.0;

/// Places the visible modules in `body`, lining up nearly aligned gaps between rows.
fn arrange(layout: &OverviewLayout, body: Rect) -> Vec<Vec<Cell>> {
    let mut rows = place(layout, body);
    for index in 1..rows.len() {
        let above: Vec<f32> = rows[index - 1]
            .windows(2)
            .map(|pair| pair[0].rect.right())
            .collect();
        for cell in 0..rows[index].len().saturating_sub(1) {
            let edge = rows[index][cell].rect.right();
            let Some(target) = above
                .iter()
                .copied()
                .filter(|target| (target - edge).abs() < SNAP_DISTANCE)
                .min_by(|a, b| (a - edge).abs().total_cmp(&(b - edge).abs()))
            else {
                continue;
            };
            rows[index][cell].rect.max.x = target;
            rows[index][cell + 1].rect.min.x = target + GAP;
        }
    }
    rows
}

/// Places the visible modules in `body` by their weights.
fn place(layout: &OverviewLayout, body: Rect) -> Vec<Vec<Cell>> {
    let rows = layout.rows();
    let heights = layout.heights();
    let total_height = body.height() - GAP * (rows.len().saturating_sub(1)) as f32;
    let height_sum: f32 = heights.iter().sum();
    let mut top = body.top();
    rows.iter()
        .zip(&heights)
        .map(|(modules, weight)| {
            let height = total_height * weight / height_sum;
            let total_width = body.width() - GAP * (modules.len().saturating_sub(1)) as f32;
            let width_sum: f32 = modules
                .iter()
                .map(|module| layout.placement(*module).width.max(MIN_WEIGHT))
                .sum();
            let mut left = body.left();
            let cells = modules
                .iter()
                .map(|module| {
                    let width =
                        total_width * layout.placement(*module).width.max(MIN_WEIGHT) / width_sum;
                    let rect = Rect::from_min_size(pos2(left, top), vec2(width, height));
                    left += width + GAP;
                    Cell {
                        module: *module,
                        rect,
                    }
                })
                .collect();
            top += height + GAP;
            cells
        })
        .collect()
}

/// Where a module dropped at `pointer` on `target` goes.
fn drop_zone(target: Rect, pointer: Pos2) -> DropZone {
    let v = (pointer.y - target.top()) / target.height();
    let u = (pointer.x - target.left()) / target.width();
    if v < 0.25 {
        DropZone::Above
    } else if v > 0.75 {
        DropZone::Below
    } else if u < 0.5 {
        DropZone::Before
    } else {
        DropZone::After
    }
}

/// Moves weight from one neighbour to the other by `delta_px`, keeping both at least
/// `min_px` and the sum unchanged.
fn shift_weights(
    (first, second): (f32, f32),
    delta_px: f32,
    px_per_weight: f32,
    min_px: f32,
) -> (f32, f32) {
    let sum = first + second;
    let min = (min_px / px_per_weight).min(sum * 0.5);
    let first = (first + delta_px / px_per_weight).clamp(min, sum - min);
    (first, sum - first)
}

/// Draws the overview page.
pub fn draw(
    ui: &mut Ui,
    body: Rect,
    state: &mut ViewState,
    settings: &mut AnalyzerSettings,
    colors: &Colors,
) -> Actions {
    settings.overview.normalize();
    let mut toolbar = Toolbar::new(body, colors.accent);
    for module in Module::ALL {
        let mut visible = settings.overview.placement(module).visible;
        let before = visible;
        toolbar.toggle_sized(ui, module.label(), &mut visible, 78.0);
        if visible != before {
            settings.overview.set_visible(module, visible);
        }
    }
    if toolbar.action(ui, "RESET LAYOUT") {
        settings.overview = OverviewLayout::default();
    }
    toolbar.note(ui, "drag a title to move · drag a gap to resize");
    let area = toolbar.rest(body);

    let rows = arrange(&settings.overview, area);
    if rows.is_empty() {
        ui.painter().text(
            area.center(),
            Align2::CENTER_CENTER,
            "Every view is hidden: switch one on above",
            font(12.0),
            TEXT_DIM,
        );
        return Actions::default();
    }
    for cell in rows.iter().flatten() {
        let compact = cell.rect.width() < COMPACT_WIDTH || cell.rect.height() < COMPACT_HEIGHT;
        draw_module(ui, cell.rect, cell.module, state, settings, colors, compact);
    }
    resize_handles(ui, &rows, area, &mut settings.overview, colors);
    move_handles(
        ui,
        &rows,
        &mut state.overview,
        &mut settings.overview,
        colors,
    );
    Actions::default()
}

/// Drag handles in the gaps between modules and between rows.
fn resize_handles(
    ui: &mut Ui,
    rows: &[Vec<Cell>],
    area: Rect,
    layout: &mut OverviewLayout,
    colors: &Colors,
) {
    for (row_index, cells) in rows.iter().enumerate() {
        let row_rect = cells
            .iter()
            .fold(Rect::NOTHING, |bounds, cell| bounds.union(cell.rect));
        let weight_sum: f32 = cells
            .iter()
            .map(|cell| layout.placement(cell.module).width.max(MIN_WEIGHT))
            .sum();
        let px_per_weight =
            (row_rect.width() - GAP * (cells.len() - 1) as f32) / weight_sum.max(1e-3);
        for pair in cells.windows(2) {
            let gap = Rect::from_min_max(
                pos2(pair[0].rect.right(), row_rect.top()),
                pos2(pair[1].rect.left(), row_rect.bottom()),
            );
            let response = ui
                .interact(
                    gap,
                    Id::new(("overview-column", pair[0].module)),
                    Sense::drag(),
                )
                .on_hover_cursor(CursorIcon::ResizeHorizontal);
            if response.hovered() || response.dragged() {
                ui.painter().vline(
                    gap.center().x,
                    gap.y_range().shrink(8.0),
                    stroke(2.0, alpha(colors.accent, 0.8)),
                );
            }
            if response.dragged() {
                let first = layout.placement(pair[0].module).width.max(MIN_WEIGHT);
                let second = layout.placement(pair[1].module).width.max(MIN_WEIGHT);
                let (first, second) = shift_weights(
                    (first, second),
                    response.drag_delta().x,
                    px_per_weight,
                    MIN_MODULE_WIDTH,
                );
                layout.placement_mut(pair[0].module).width = first;
                layout.placement_mut(pair[1].module).width = second;
            }
        }
        if row_index + 1 < rows.len() {
            let below = rows[row_index + 1][0].rect.top();
            let gap = Rect::from_min_max(
                pos2(area.left(), row_rect.bottom()),
                pos2(area.right(), below),
            );
            let response = ui
                .interact(gap, Id::new(("overview-row", row_index)), Sense::drag())
                .on_hover_cursor(CursorIcon::ResizeVertical);
            if response.hovered() || response.dragged() {
                ui.painter().hline(
                    gap.x_range().shrink(8.0),
                    gap.center().y,
                    stroke(2.0, alpha(colors.accent, 0.8)),
                );
            }
            if response.dragged() {
                let heights: f32 = layout.row_heights[..rows.len()].iter().sum();
                let px_per_weight = (area.height() - GAP * (rows.len() - 1) as f32) / heights;
                let (first, second) = shift_weights(
                    (
                        layout.row_heights[row_index],
                        layout.row_heights[row_index + 1],
                    ),
                    response.drag_delta().y,
                    px_per_weight,
                    MIN_MODULE_HEIGHT,
                );
                layout.row_heights[row_index] = first;
                layout.row_heights[row_index + 1] = second;
            }
        }
    }
}

/// Title strips that move modules by drag and drop.
fn move_handles(
    ui: &mut Ui,
    rows: &[Vec<Cell>],
    state: &mut OverviewState,
    layout: &mut OverviewLayout,
    colors: &Colors,
) {
    let mut released = false;
    for cell in rows.iter().flatten() {
        let strip = Rect::from_min_size(cell.rect.min, vec2(cell.rect.width(), TITLE_HEIGHT));
        let response = ui
            .interact(
                strip,
                Id::new(("overview-move", cell.module)),
                Sense::drag(),
            )
            .on_hover_cursor(CursorIcon::Grab)
            .on_hover_text("Drag to move");
        // Grip dots centred on the top edge.
        let lit = response.hovered() || response.dragged();
        let grip = pos2(strip.center().x, strip.top() + 5.0);
        for offset in [-8.0, -4.0, 0.0, 4.0, 8.0] {
            ui.painter().circle_filled(
                grip + vec2(offset, 0.0),
                1.2,
                if lit {
                    colors.accent
                } else {
                    alpha(TEXT_DIM, 0.6)
                },
            );
        }
        if response.drag_started() {
            state.dragging = Some(cell.module);
        }
        if response.drag_stopped() {
            released = true;
        }
    }
    let Some(dragged) = state.dragging else {
        return;
    };
    let Some(pointer) = ui.input(|input| input.pointer.latest_pos()) else {
        return;
    };
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    let target = rows
        .iter()
        .flatten()
        .find(|cell| cell.module != dragged && cell.rect.contains(pointer));
    if let Some(target) = target {
        let zone = drop_zone(target.rect, pointer);
        let rect = target.rect;
        let marker = match zone {
            DropZone::Above => Rect::from_min_size(rect.min, vec2(rect.width(), 4.0)),
            DropZone::Below => Rect::from_min_size(
                pos2(rect.left(), rect.bottom() - 4.0),
                vec2(rect.width(), 4.0),
            ),
            DropZone::Before => Rect::from_min_size(rect.min, vec2(4.0, rect.height())),
            DropZone::After => Rect::from_min_size(
                pos2(rect.right() - 4.0, rect.top()),
                vec2(4.0, rect.height()),
            ),
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(6), alpha(colors.accent, 0.08));
        painter.rect_filled(marker, CornerRadius::same(2), colors.accent);
        if released {
            layout.move_module(dragged, target.module, zone);
        }
    }
    // The dragged module's name follows the pointer.
    let painter = ui.painter();
    let label = painter.layout_no_wrap(dragged.label().to_owned(), font(11.0), TEXT);
    let tag = Rect::from_min_size(pointer + vec2(12.0, 8.0), label.size() + vec2(16.0, 8.0));
    painter.rect(
        tag,
        CornerRadius::same(4),
        PANEL_RAISED,
        stroke(
            1.0,
            if target.is_some() {
                colors.accent
            } else {
                BORDER
            },
        ),
        StrokeKind::Inside,
    );
    painter.galley(tag.min + vec2(8.0, 4.0), label, TEXT);
    if released {
        state.dragging = None;
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2};

    use super::{DropZone, arrange, drop_zone, shift_weights};
    use crate::layout::OverviewLayout;

    #[test]
    fn modules_fill_the_area_by_weight() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(1_000.0, 612.0));
        let rows = arrange(&OverviewLayout::default(), area);
        assert_eq!(rows.len(), 2);
        let last = rows[1].last().expect("a module").rect;
        assert!((last.right() - 1_000.0).abs() < 0.01);
        assert!((last.bottom() - 612.0).abs() < 0.01);
        assert!(rows[1][0].rect.width() > rows[1][1].rect.width());
        assert_eq!(
            rows[0][0].rect.right(),
            rows[1][0].rect.right(),
            "spectral and spectrum line up"
        );
    }

    #[test]
    fn drop_zones_and_resizing() {
        let target = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        assert_eq!(drop_zone(target, pos2(50.0, 10.0)), DropZone::Above);
        assert_eq!(drop_zone(target, pos2(50.0, 90.0)), DropZone::Below);
        assert_eq!(drop_zone(target, pos2(20.0, 50.0)), DropZone::Before);
        assert_eq!(drop_zone(target, pos2(80.0, 50.0)), DropZone::After);
        let (first, second) = shift_weights((1.0, 1.0), 100.0, 200.0, 50.0);
        assert!((first - 1.5).abs() < 1e-6 && (second - 0.5).abs() < 1e-6);
        let (first, second) = shift_weights((1.0, 1.0), 1_000.0, 200.0, 50.0);
        assert!((second - 0.25).abs() < 1e-6 && (first + second - 2.0).abs() < 1e-6);
    }
}
