//! The SOURCES panel: the buses analyzed next to the input, with their loudness, and how
//! the spectrum compares them; and the traces, the signals a view draws.

use aud_ui::theme::{ALERT, TEXT, TEXT_DIM, TEXT_FAINT};
use egui::{
    Area, Button, Color32, ComboBox, Context, Grid, Id, Key, Order, Rect, Response, RichText,
    Sense, TextEdit, Ui, vec2,
};

use crate::analysis::bus::{SourceReading, SourceState};
use crate::analysis::loudness::{ABSOLUTE_GATE_LUFS, LoudnessReading};
use crate::analysis::{Published, SignalReading};
use crate::layout::Module;
use crate::sources::{MAX_BUSES, SourceList};
use crate::style::SOURCE_COLORS;
use crate::view::colormap::{Colors, rgb};
use crate::view::menu::{frame, should_close};
use crate::view::plot::Header;

/// Width of the panel's name field in logical pixels.
const NAME_WIDTH: f32 = 150.0;

/// The spectrum curve color of the source at `index` in the list.
pub fn source_color(index: usize) -> Color32 {
    rgb(SOURCE_COLORS[index % SOURCE_COLORS.len()])
}

/// One signal a view draws: the input or a bus.
pub struct Trace<'a> {
    pub name: &'a str,
    pub color: Color32,
    pub signal: &'a SignalReading,
    /// The bus's position in the list and its state; `None` for the input.
    pub bus: Option<(usize, SourceState)>,
}

impl Trace<'_> {
    /// The trace's name, with the bus's state unless it is live, as a header legend.
    pub fn legend(&self, header: &mut Header<'_>) {
        let label = match self.bus {
            Some((_, state)) if state != SourceState::Live => {
                format!("{} · {}", self.name, state_word(state))
            }
            _ => self.name.to_owned(),
        };
        if !header.legend(&label, self.color) {
            header.legend(self.name, self.color);
        }
    }

    /// What a view says while this signal has nothing to draw.
    pub fn waiting_text(&self) -> String {
        match self.bus {
            None => String::from("Waiting for audio"),
            Some((_, SourceState::Live)) => format!("Waiting for {}", self.name),
            Some((_, state)) => format!("{}: {}", self.name, state_word(state)),
        }
    }
}

/// The signals `module` shows, the input first, then the buses in list order. The input
/// is drawn in `input_color`, each bus in its own color.
pub fn traces<'a>(
    list: &'a SourceList,
    data: &'a Published,
    module: Module,
    input_color: Color32,
) -> Vec<Trace<'a>> {
    let shown = list.shown(module);
    let input = shown.input.then_some(Trace {
        name: "Input",
        color: input_color,
        signal: &data.input,
        bus: None,
    });
    input
        .into_iter()
        .chain(shown.buses.iter().filter_map(|index| {
            let reading = data.sources.get(*index)?;
            Some(Trace {
                name: list.buses[*index].name.as_str(),
                color: source_color(*index),
                signal: &reading.signal,
                bus: Some((*index, reading.state)),
            })
        }))
        .collect()
}

/// Names the traces in a view's header when there are several, or when the only one is
/// a bus.
pub fn name_traces(header: &mut Header<'_>, traces: &[Trace<'_>]) {
    if traces.len() > 1 || traces.first().is_some_and(|trace| trace.bus.is_some()) {
        traces.iter().for_each(|trace| trace.legend(header));
    }
}

/// What a view without signals says.
pub const NO_SIGNALS: &str = "No signals: choose them in the view's settings";

fn state_word(state: SourceState) -> &'static str {
    match state {
        SourceState::Live => "live",
        SourceState::Waiting => "starting",
        SourceState::NoTransmitter => "no transmitter",
        SourceState::InvalidName => "invalid name",
    }
}

/// The level shift in dB that matches a source's loudness to the input's: integrated
/// loudness once both have it, short-term loudness before that, else none.
pub fn match_offset_db(input: &LoudnessReading, source: &LoudnessReading) -> f32 {
    let audible = |lufs: f32| lufs.is_finite() && lufs > ABSOLUTE_GATE_LUFS;
    if audible(input.integrated_lufs) && audible(source.integrated_lufs) {
        input.integrated_lufs - source.integrated_lufs
    } else if audible(input.short_term_lufs) && audible(source.short_term_lufs) {
        input.short_term_lufs - source.short_term_lufs
    } else {
        0.0
    }
}

/// Draws the panel below `button`; returns `false` when it closed.
pub fn sources_panel(
    context: &Context,
    button: Rect,
    (list, draft): (&mut SourceList, &mut String),
    data: &Published,
    (colors, input_color): (&Colors, Color32),
) -> bool {
    let id = Id::new("sources-panel");
    let response = Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(button.left_bottom() + vec2(-420.0, 8.0))
        .constrain(true)
        .show(context, |ui| {
            frame(colors.accent).show(ui, |ui| {
                ui.set_max_width(640.0);
                ui.label(RichText::new("SOURCES").size(12.0).color(colors.accent));
                ui.label(
                    RichText::new(
                        "Buses from O-AUD-TX, measured next to the input and shown in every \
                         view, overlaid in their colors. Hide a signal in a view under SIGNALS \
                         in its settings; a dot here shows or hides a bus on the spectrum. \
                         Buses are not time-aligned with the input.",
                    )
                    .size(10.0)
                    .color(TEXT_DIM),
                );
                ui.add_space(8.0);
                source_grid(ui, id, list, data, input_color);
                ui.add_space(8.0);
                add_row(ui, id, (list, draft), &data.live_buses, colors);
                ui.add_space(8.0);
                ui.checkbox(
                    &mut list.match_level,
                    RichText::new("Match each spectrum curve's level to the first (by loudness)")
                        .color(TEXT),
                );
                ui.checkbox(
                    &mut list.masking,
                    RichText::new("Shade frequencies where two curves are within 6 dB").color(TEXT),
                );
            });
        })
        .response;
    !should_close(context, response.rect, Some(button))
}

fn source_grid(ui: &mut Ui, id: Id, list: &mut SourceList, data: &Published, input: Color32) {
    let mut remove = None;
    Grid::new((id, "grid"))
        .num_columns(9)
        .spacing(vec2(10.0, 6.0))
        .show(ui, |ui| {
            for caption in ["", "SOURCE", "STATE", "FORMAT", "M", "S", "I", "SHIFT", ""] {
                ui.label(RichText::new(caption).size(10.0).color(TEXT_FAINT));
            }
            ui.end_row();

            dot(ui, input);
            ui.label(RichText::new("Input").color(TEXT));
            ui.label(RichText::new("this track").color(TEXT_DIM));
            ui.label(format_text(data.input.sample_rate, data.input.channels));
            loudness_cells(ui, &data.input.loudness);
            ui.label("");
            ui.label("");
            ui.end_row();

            let empty = SourceReading::default();
            let spectrum = list.shown(Module::Spectrum);
            for index in 0..list.buses.len() {
                let reading = data.sources.get(index).unwrap_or(&empty);
                let color = source_color(index);
                let on_spectrum = spectrum.has_bus(index);
                let response = dot(ui, if on_spectrum { color } else { TEXT_FAINT });
                if response
                    .on_hover_text("Show or hide its spectrum curve; it is measured either way")
                    .clicked()
                {
                    list.show_bus(Module::Spectrum, index, !on_spectrum);
                }
                ui.label(
                    RichText::new(&list.buses[index].name).color(if on_spectrum {
                        TEXT
                    } else {
                        TEXT_DIM
                    }),
                );
                let (state, state_color) = state_text(reading);
                ui.label(RichText::new(state).color(state_color));
                let signal = &reading.signal;
                ui.label(format_text(signal.sample_rate, signal.channels));
                loudness_cells(ui, &signal.loudness);
                let shift = if list.match_level && reading.state == SourceState::Live {
                    format!(
                        "{:+.1} dB",
                        match_offset_db(&data.input.loudness, &signal.loudness)
                    )
                } else {
                    String::from("–")
                };
                ui.label(RichText::new(shift).monospace().color(TEXT_DIM));
                if ui
                    .small_button("✕")
                    .on_hover_text("Stop analyzing this bus")
                    .clicked()
                {
                    remove = Some(index);
                }
                ui.end_row();
            }
        });
    if let Some(index) = remove {
        list.remove(index);
    }
}

/// A clickable color dot.
fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::click());
    ui.painter().circle_filled(rect.center(), 5.0, color);
    response
}

fn state_text(reading: &SourceReading) -> (String, Color32) {
    match reading.state {
        SourceState::Live if reading.gaps > 0 => (format!("live · {} gaps", reading.gaps), TEXT),
        SourceState::Live => (String::from("live"), TEXT),
        SourceState::InvalidName => (String::from("invalid name"), ALERT),
        state => (state_word(state).to_owned(), TEXT_DIM),
    }
}

fn format_text(sample_rate: u32, channels: u32) -> RichText {
    let text = if sample_rate > 0 {
        format!("{:.1}k · {channels}ch", sample_rate as f32 / 1_000.0)
    } else {
        String::from("–")
    };
    RichText::new(text).monospace().color(TEXT_DIM)
}

/// Momentary, short-term and integrated loudness.
fn loudness_cells(ui: &mut Ui, loudness: &LoudnessReading) {
    for lufs in [
        loudness.momentary_lufs,
        loudness.short_term_lufs,
        loudness.integrated_lufs,
    ] {
        let text = if lufs.is_finite() && lufs > ABSOLUTE_GATE_LUFS {
            format!("{lufs:.1}")
        } else {
            String::from("–")
        };
        ui.label(RichText::new(text).monospace().color(TEXT));
    }
}

/// Adds a live bus from a list, or any name typed in, which waits for its transmitter.
fn add_row(
    ui: &mut Ui,
    id: Id,
    (list, draft): (&mut SourceList, &mut String),
    live: &[String],
    colors: &Colors,
) {
    if list.buses.len() >= MAX_BUSES {
        ui.label(
            RichText::new(format!("At most {MAX_BUSES} buses"))
                .size(10.0)
                .color(TEXT_DIM),
        );
        return;
    }
    ui.horizontal(|ui| {
        let available: Vec<&String> = live.iter().filter(|name| !list.contains(name)).collect();
        let mut chosen = None;
        ComboBox::from_id_salt((id, "live"))
            .selected_text(if available.is_empty() {
                "No other buses live"
            } else {
                "Add a live bus…"
            })
            .width(170.0)
            .show_ui(ui, |ui| {
                for name in &available {
                    if ui.selectable_label(false, name.as_str()).clicked() {
                        chosen = Some((*name).clone());
                    }
                }
            });
        if let Some(name) = chosen {
            list.add(&name);
        }
        ui.label(RichText::new("or").color(TEXT_DIM));
        let field = ui.add(
            TextEdit::singleline(draft)
                .hint_text("bus name")
                .desired_width(NAME_WIDTH),
        );
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        let clicked = ui
            .add_enabled(
                !draft.trim().is_empty(),
                Button::new(RichText::new("ADD").color(colors.accent)),
            )
            .clicked();
        if (entered || clicked) && list.add(draft) {
            draft.clear();
        }
    });
}
