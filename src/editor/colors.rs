//! O-NEC's signal colors. The base palette, typography and drawing helpers come from
//! `aud-ui`, shared with O-AUD-TX and O-AUD-RX.

use aud_ui::theme::{ALERT, BUS, LEVEL, RECEIVE, SYNC};
use nih_plug_egui::egui::Color32;

/// Kept voice and output.
pub const VOICE: Color32 = RECEIVE;
/// Echo and the echo reference.
pub const ECHO: Color32 = Color32::from_rgb(0xf5, 0xa5, 0x24);
/// Removed noise.
pub const REMOVED: Color32 = ALERT;
/// Gate and voice activity.
pub const GATE: Color32 = BUS;
/// Speech leveler gain.
pub const LEVELER: Color32 = SYNC;
/// Unprocessed input.
pub const INPUT: Color32 = LEVEL;
