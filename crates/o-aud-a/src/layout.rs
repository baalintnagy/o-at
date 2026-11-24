//! The overview's arrangement: which modules it shows, in which rows and order, and how
//! wide and tall they are.
//!
//! Stored with the settings by field name, so new modules can be added without losing
//! saved layouts: never rename the fields.

use serde::{Deserialize, Serialize};

/// Rows the overview can have.
pub const MAX_ROWS: usize = 4;
/// Smallest width or height weight, relative to the others.
pub const MIN_WEIGHT: f32 = 0.2;

/// A view the overview can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    Spectrum,
    Sonogram,
    Spectral,
    Stereo,
    Oscilloscope,
    Loudness,
    Surface,
}

impl Module {
    /// In the order of the pages.
    pub const ALL: [Self; 7] = [
        Self::Spectral,
        Self::Spectrum,
        Self::Sonogram,
        Self::Oscilloscope,
        Self::Stereo,
        Self::Loudness,
        Self::Surface,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "SPECTRUM",
            Self::Sonogram => "SONOGRAM",
            Self::Spectral => "SPECTRAL",
            Self::Stereo => "STEREO",
            Self::Oscilloscope => "SCOPE",
            Self::Loudness => "LOUDNESS",
            Self::Surface => "3D",
        }
    }
}

/// Where a dragged module goes, relative to the module it is dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Before,
    After,
    /// A new row above the target's row.
    Above,
    /// A new row below the target's row.
    Below,
}

/// One module's place.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModulePlacement {
    pub visible: bool,
    pub row: u8,
    /// Position within the row, from the left.
    pub order: u8,
    /// Width weight relative to the other modules in its row.
    pub width: f32,
}

impl Default for ModulePlacement {
    fn default() -> Self {
        Self {
            visible: false,
            row: (MAX_ROWS - 1) as u8,
            order: 0,
            width: 1.0,
        }
    }
}

const fn placed(row: u8, order: u8, width: f32) -> ModulePlacement {
    ModulePlacement {
        visible: true,
        row,
        order,
        width,
    }
}

/// The overview's arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverviewLayout {
    pub spectrum: ModulePlacement,
    pub sonogram: ModulePlacement,
    pub spectral: ModulePlacement,
    pub stereo: ModulePlacement,
    pub oscilloscope: ModulePlacement,
    pub loudness: ModulePlacement,
    pub surface: ModulePlacement,
    /// Height weight of each row.
    pub row_heights: [f32; MAX_ROWS],
}

impl Default for OverviewLayout {
    fn default() -> Self {
        Self {
            spectral: placed(0, 0, 1.0),
            stereo: placed(0, 1, 1.15),
            // As wide as the spectral view; the scope and loudness share the stereo
            // view's width.
            spectrum: placed(1, 0, 1.0),
            oscilloscope: placed(1, 1, 0.72),
            loudness: placed(1, 2, 0.43),
            sonogram: ModulePlacement::default(),
            surface: ModulePlacement::default(),
            row_heights: [1.1, 1.0, 1.0, 1.0],
        }
    }
}

/// A row while rearranging: its height and modules from left to right.
struct Row {
    height: f32,
    modules: Vec<Module>,
}

impl OverviewLayout {
    pub fn placement(&self, module: Module) -> &ModulePlacement {
        match module {
            Module::Spectrum => &self.spectrum,
            Module::Sonogram => &self.sonogram,
            Module::Spectral => &self.spectral,
            Module::Stereo => &self.stereo,
            Module::Oscilloscope => &self.oscilloscope,
            Module::Loudness => &self.loudness,
            Module::Surface => &self.surface,
        }
    }

    pub fn placement_mut(&mut self, module: Module) -> &mut ModulePlacement {
        match module {
            Module::Spectrum => &mut self.spectrum,
            Module::Sonogram => &mut self.sonogram,
            Module::Spectral => &mut self.spectral,
            Module::Stereo => &mut self.stereo,
            Module::Oscilloscope => &mut self.oscilloscope,
            Module::Loudness => &mut self.loudness,
            Module::Surface => &mut self.surface,
        }
    }

    /// The visible modules by row, top to bottom and left to right, without empty rows.
    pub fn rows(&self) -> Vec<Vec<Module>> {
        self.working_rows()
            .into_iter()
            .map(|row| row.modules)
            .collect()
    }

    /// Row heights matching [`rows`](Self::rows).
    pub fn heights(&self) -> Vec<f32> {
        self.working_rows()
            .into_iter()
            .map(|row| row.height)
            .collect()
    }

    fn working_rows(&self) -> Vec<Row> {
        (0..MAX_ROWS)
            .filter_map(|row| {
                let mut modules: Vec<Module> = Module::ALL
                    .into_iter()
                    .filter(|module| {
                        let placement = self.placement(*module);
                        placement.visible && usize::from(placement.row).min(MAX_ROWS - 1) == row
                    })
                    .collect();
                modules.sort_by_key(|module| self.placement(*module).order);
                (!modules.is_empty()).then(|| Row {
                    height: self.row_heights[row].max(MIN_WEIGHT),
                    modules,
                })
            })
            .collect()
    }

    fn apply(&mut self, mut rows: Vec<Row>) {
        rows.retain(|row| !row.modules.is_empty());
        while rows.len() > MAX_ROWS {
            if let Some(extra) = rows.pop()
                && let Some(last) = rows.last_mut()
            {
                last.modules.extend(extra.modules);
            }
        }
        let mut heights = [1.0; MAX_ROWS];
        for (index, row) in rows.iter().enumerate() {
            heights[index] = row.height;
            for (order, module) in row.modules.iter().enumerate() {
                let placement = self.placement_mut(*module);
                placement.visible = true;
                placement.row = index as u8;
                placement.order = order as u8;
            }
        }
        self.row_heights = heights;
    }

    /// Renumbers rows and positions without gaps, keeping each row's height.
    pub fn normalize(&mut self) {
        let rows = self.working_rows();
        self.apply(rows);
    }

    /// Shows a hidden module at the end of the last row, or hides a shown one.
    pub fn set_visible(&mut self, module: Module, visible: bool) {
        let mut rows = self.working_rows();
        rows.iter_mut()
            .for_each(|row| row.modules.retain(|other| *other != module));
        if visible {
            let full = rows.len() >= MAX_ROWS;
            match rows.last_mut() {
                Some(last) if last.modules.len() < 3 || full => {
                    last.modules.push(module);
                }
                _ => rows.push(Row {
                    height: 1.0,
                    modules: vec![module],
                }),
            }
        } else {
            self.placement_mut(module).visible = false;
        }
        self.apply(rows);
    }

    /// Moves `module` next to `target`, or into a new row beside the target's row.
    pub fn move_module(&mut self, module: Module, target: Module, drop: DropZone) {
        if module == target || !self.placement(target).visible {
            return;
        }
        let mut rows = self.working_rows();
        rows.iter_mut()
            .for_each(|row| row.modules.retain(|other| *other != module));
        let Some(row_index) = rows.iter().position(|row| row.modules.contains(&target)) else {
            return;
        };
        let room = rows.iter().filter(|row| !row.modules.is_empty()).count() < MAX_ROWS;
        let drop = match drop {
            DropZone::Above | DropZone::Below if !room => DropZone::After,
            drop => drop,
        };
        match drop {
            DropZone::Before | DropZone::After => {
                let row = &mut rows[row_index];
                let position = row
                    .modules
                    .iter()
                    .position(|other| *other == target)
                    .unwrap_or(0);
                let position = if drop == DropZone::After {
                    position + 1
                } else {
                    position
                };
                row.modules.insert(position, module);
            }
            DropZone::Above | DropZone::Below => {
                let index = if drop == DropZone::Below {
                    row_index + 1
                } else {
                    row_index
                };
                rows.insert(
                    index,
                    Row {
                        height: 1.0,
                        modules: vec![module],
                    },
                );
            }
        }
        self.apply(rows);
    }
}

#[cfg(test)]
mod tests {
    use super::{DropZone, MAX_ROWS, Module, OverviewLayout};

    #[test]
    fn default_layout_has_two_rows() {
        let layout = OverviewLayout::default();
        assert_eq!(
            layout.rows(),
            vec![
                vec![Module::Spectral, Module::Stereo],
                vec![Module::Spectrum, Module::Oscilloscope, Module::Loudness],
            ]
        );
    }

    #[test]
    fn modules_move_within_and_between_rows() {
        let mut layout = OverviewLayout::default();
        layout.move_module(Module::Loudness, Module::Spectral, DropZone::Before);
        assert_eq!(
            layout.rows()[0],
            vec![Module::Loudness, Module::Spectral, Module::Stereo]
        );
        layout.move_module(Module::Stereo, Module::Spectrum, DropZone::Below);
        assert_eq!(layout.rows().len(), 3);
        assert_eq!(layout.rows()[2], vec![Module::Stereo]);
        // Moving the only module out of a row removes the row.
        layout.move_module(Module::Stereo, Module::Spectral, DropZone::After);
        assert_eq!(layout.rows().len(), 2);
        assert_eq!(
            layout.rows()[0],
            vec![Module::Loudness, Module::Spectral, Module::Stereo]
        );
    }

    #[test]
    fn rows_are_limited() {
        let mut layout = OverviewLayout::default();
        layout.set_visible(Module::Sonogram, true);
        layout.set_visible(Module::Surface, true);
        for module in [
            Module::Stereo,
            Module::Loudness,
            Module::Sonogram,
            Module::Surface,
        ] {
            layout.move_module(module, Module::Spectrum, DropZone::Above);
        }
        assert_eq!(layout.rows().len(), MAX_ROWS);
        let shown: usize = layout.rows().iter().map(Vec::len).sum();
        assert_eq!(shown, 7);
    }

    #[test]
    fn hiding_and_showing_keeps_the_rest() {
        let mut layout = OverviewLayout::default();
        layout.set_visible(Module::Spectral, false);
        layout.set_visible(Module::Stereo, false);
        assert_eq!(layout.rows().len(), 1, "the empty row goes away");
        layout.set_visible(Module::Surface, true);
        assert_eq!(layout.rows().len(), 2, "a full row gets a new row");
        assert!(layout.placement(Module::Surface).visible);
        assert!(!layout.placement(Module::Spectral).visible);
    }
}
