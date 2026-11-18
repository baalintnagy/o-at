//! Colors and typography, shared with the O-NEC editor's look.

use egui::{Color32, Context, FontId, Stroke, Visuals};

pub const BACKGROUND: Color32 = Color32::from_rgb(0x12, 0x14, 0x19);
pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1d, 0x24);
pub const PANEL_RAISED: Color32 = Color32::from_rgb(0x22, 0x26, 0x2f);
pub const BORDER: Color32 = Color32::from_rgb(0x2b, 0x30, 0x3b);
pub const GRID: Color32 = Color32::from_rgb(0x25, 0x2a, 0x34);
pub const GRID_STRONG: Color32 = Color32::from_rgb(0x33, 0x39, 0x46);
pub const TRACK: Color32 = Color32::from_rgb(0x30, 0x35, 0x41);

pub const TEXT: Color32 = Color32::from_rgb(0xd6, 0xdb, 0xe4);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x80, 0x88, 0x97);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x55, 0x5c, 0x6a);

/// Transmitting: O-AUD-TX accents.
pub const SEND: Color32 = Color32::from_rgb(0xff, 0x8a, 0x3d);
/// Receiving and playing: O-AUD-RX accents.
pub const RECEIVE: Color32 = Color32::from_rgb(0x3c, 0xd6, 0xc0);
/// Buses and their names.
pub const BUS: Color32 = Color32::from_rgb(0xa4, 0x8b, 0xfa);
/// Clock synchronization.
pub const SYNC: Color32 = Color32::from_rgb(0x5c, 0xa8, 0xff);
/// Measuring: O-AUD-A accents.
pub const ANALYZE: Color32 = Color32::from_rgb(0xc6, 0xe0, 0x5a);
/// Problems: dropouts, name conflicts.
pub const ALERT: Color32 = Color32::from_rgb(0xff, 0x5c, 0x7a);
/// Neutral signal levels.
pub const LEVEL: Color32 = Color32::from_rgb(0x8a, 0x93, 0xa3);

/// The wave inside the O-AT logo's ring.
pub const LOGO_WAVE: Color32 = Color32::from_rgb(0x5c, 0xa8, 0xff);

pub const METER_WARN: Color32 = Color32::from_rgb(0xf5, 0xd0, 0x4c);
pub const METER_CLIP: Color32 = Color32::from_rgb(0xff, 0x55, 0x55);

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// `color` with its alpha replaced, for translucent fills.
pub fn alpha(color: Color32, alpha: f32) -> Color32 {
    let [red, green, blue, _] = color.to_array();
    Color32::from_rgba_unmultiplied(red, green, blue, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Stroke with an `f32` width, avoiding float literal fallback through `Into<f32>`.
pub fn stroke(width: f32, color: Color32) -> Stroke {
    Stroke::new(width, color)
}

/// Applies the dark theme to a context.
pub fn apply(context: &Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = PANEL;
    visuals.window_stroke = stroke(1.0, BORDER);
    visuals.extreme_bg_color = PANEL;
    visuals.selection.bg_fill = alpha(BUS, 0.35);
    visuals.selection.stroke = stroke(1.0, BUS);
    visuals.text_cursor.stroke = stroke(1.5, TEXT);
    context.set_visuals(visuals);
}
