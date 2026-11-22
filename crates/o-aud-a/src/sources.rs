//! The audio buses analyzed next to the input, chosen in the SOURCES panel, and which
//! signals each view shows.
//!
//! Persisted by field name under `#[persist = "sources"]`, separately from the
//! [`AnalyzerSettings`](crate::settings::AnalyzerSettings), which stay `Copy`. Never
//! rename fields. Fields missing from older sessions take their defaults.

use serde::{Deserialize, Serialize};

use crate::layout::Module;

/// Most buses analyzed at once.
pub const MAX_BUSES: usize = 8;

/// The buses to analyze, how the spectrum compares them and what each view shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceList {
    pub buses: Vec<BusSource>,
    /// Shifts each bus curve by the bus's loudness difference to the first shown signal.
    pub match_level: bool,
    /// Shades the frequencies where two shown sources are about equally loud.
    pub masking: bool,
    /// The signals each view shows.
    pub views: ViewSources,
}

/// One bus, found by name; it may not have a transmitter yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BusSource {
    pub name: String,
    /// Whether the spectrum draws its curve, as sessions from before per-view sources
    /// stored it; read only while [`ViewSources::spectrum`] is unset.
    pub shown: bool,
}

impl Default for BusSource {
    fn default() -> Self {
        Self {
            name: String::new(),
            shown: true,
        }
    }
}

/// The signals one view shows: the input, if chosen, and every listed bus that is not
/// hidden there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub input: bool,
    /// Names of the buses the view leaves out, matched case-insensitively.
    pub hidden: Vec<String>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            input: true,
            hidden: Vec::new(),
        }
    }
}

/// The signals of every view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSources {
    /// Unset in sessions from before per-view sources: the input and the buses marked
    /// [`BusSource::shown`]. Every other view shows every bus until it is hidden there.
    pub spectrum: Option<Selection>,
    pub sonogram: Selection,
    pub spectral: Selection,
    pub surface: Selection,
    pub oscilloscope: Selection,
    pub stereo: Selection,
    pub loudness: Selection,
}

/// What a view shows, resolved against the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    pub input: bool,
    /// Positions in the list, in list order.
    pub buses: Vec<usize>,
}

impl Shown {
    /// Signals shown: the input, if shown, and the buses.
    pub fn count(&self) -> usize {
        usize::from(self.input) + self.buses.len()
    }

    pub fn has_bus(&self, index: usize) -> bool {
        self.buses.contains(&index)
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl SourceList {
    /// Whether a bus of this name (case-insensitive, like bus names) is in the list.
    pub fn contains(&self, name: &str) -> bool {
        self.position(name).is_some()
    }

    /// Position in the list of the bus called `name`; `None` for an empty name or one
    /// not listed.
    pub fn position(&self, name: &str) -> Option<usize> {
        if name.trim().is_empty() {
            return None;
        }
        self.buses.iter().position(|bus| same_name(&bus.name, name))
    }

    /// Adds a bus unless it is listed already or the list is full; returns whether it
    /// was added. Every view shows it at once.
    pub fn add(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.contains(name) || self.buses.len() >= MAX_BUSES {
            return false;
        }
        self.buses.push(BusSource {
            name: name.to_owned(),
            shown: true,
        });
        true
    }

    /// The signals `module` shows, as chosen.
    pub fn selection(&self, module: Module) -> Selection {
        let views = &self.views;
        match module {
            Module::Spectrum => views
                .spectrum
                .clone()
                .unwrap_or_else(|| self.legacy_spectrum()),
            Module::Sonogram => views.sonogram.clone(),
            Module::Spectral => views.spectral.clone(),
            Module::Surface => views.surface.clone(),
            Module::Oscilloscope => views.oscilloscope.clone(),
            Module::Stereo => views.stereo.clone(),
            Module::Loudness => views.loudness.clone(),
        }
    }

    /// The spectrum's selection in sessions from before per-view sources.
    fn legacy_spectrum(&self) -> Selection {
        Selection {
            input: true,
            hidden: self
                .buses
                .iter()
                .filter(|bus| !bus.shown)
                .map(|bus| bus.name.clone())
                .collect(),
        }
    }

    fn selection_mut(&mut self, module: Module) -> &mut Selection {
        if module == Module::Spectrum && self.views.spectrum.is_none() {
            self.views.spectrum = Some(self.legacy_spectrum());
        }
        let views = &mut self.views;
        match module {
            Module::Spectrum => views.spectrum.get_or_insert_with(Selection::default),
            Module::Sonogram => &mut views.sonogram,
            Module::Spectral => &mut views.spectral,
            Module::Surface => &mut views.surface,
            Module::Oscilloscope => &mut views.oscilloscope,
            Module::Stereo => &mut views.stereo,
            Module::Loudness => &mut views.loudness,
        }
    }

    /// What `module` shows: the input if chosen and the listed buses not hidden there.
    pub fn shown(&self, module: Module) -> Shown {
        let selection = self.selection(module);
        Shown {
            input: selection.input,
            buses: self
                .buses
                .iter()
                .enumerate()
                .filter(|(_, bus)| {
                    !selection
                        .hidden
                        .iter()
                        .any(|name| same_name(name, &bus.name))
                })
                .map(|(index, _)| index)
                .collect(),
        }
    }

    /// Shows or hides the input in `module`.
    pub fn show_input(&mut self, module: Module, shown: bool) {
        self.selection_mut(module).input = shown;
    }

    /// Shows or hides the bus at `index` in `module`.
    pub fn show_bus(&mut self, module: Module, index: usize, shown: bool) {
        let Some(name) = self.buses.get(index).map(|bus| bus.name.clone()) else {
            return;
        };
        let selection = self.selection_mut(module);
        selection.hidden.retain(|listed| !same_name(listed, &name));
        if !shown {
            selection.hidden.push(name);
        }
    }

    /// Removes the bus at `index` from the list, forgetting where it was hidden.
    pub fn remove(&mut self, index: usize) {
        if index >= self.buses.len() {
            return;
        }
        for module in Module::ALL {
            self.show_bus(module, index, true);
        }
        self.buses.remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_BUSES, SourceList};
    use crate::layout::Module;

    #[test]
    fn buses_are_unique_and_limited() {
        let mut list = SourceList::default();
        assert!(list.add("Kick"));
        assert!(!list.add(" kick "), "names match case-insensitively");
        assert!(!list.add(""));
        (1..MAX_BUSES).for_each(|index| {
            list.add(&format!("Bus {index}"));
        });
        assert_eq!(list.buses.len(), MAX_BUSES);
        assert!(!list.add("One more"));
    }

    #[test]
    fn older_sessions_load_with_defaults() {
        let list: SourceList =
            serde_json::from_str(r#"{"buses":[{"name":"Ref"},{"name":"Off","shown":false}]}"#)
                .expect("valid JSON");
        assert!(list.buses[0].shown);
        assert!(!list.match_level && !list.masking);
        let spectrum = list.shown(Module::Spectrum);
        assert!(spectrum.input);
        assert_eq!(spectrum.buses, [0], "the spectrum keeps its curves");
        let sonogram = list.shown(Module::Sonogram);
        assert!(sonogram.input);
        assert_eq!(sonogram.buses, [0, 1], "other views show every bus");
    }

    #[test]
    fn each_view_has_its_own_signals() {
        let mut list = SourceList::default();
        list.add("Kick");
        list.add("Bass");
        for module in Module::ALL {
            assert_eq!(
                list.shown(module).buses,
                [0, 1],
                "every view shows new buses"
            );
        }
        list.show_bus(Module::Oscilloscope, 0, false);
        list.show_input(Module::Oscilloscope, false);
        let scope = list.shown(Module::Oscilloscope);
        assert!(!scope.input);
        assert_eq!(scope.buses, [1]);
        assert_eq!(scope.count(), 1);
        list.show_bus(Module::Spectrum, 1, false);
        assert_eq!(list.shown(Module::Spectrum).buses, [0]);
        list.remove(0);
        assert_eq!(
            list.shown(Module::Oscilloscope).buses,
            [0],
            "follows its bus"
        );
        assert!(
            list.views.oscilloscope.hidden.is_empty(),
            "a removed bus is forgotten"
        );
        list.add("Kick");
        assert_eq!(
            list.shown(Module::Oscilloscope).buses,
            [0, 1],
            "added again, it shows again"
        );
    }
}
