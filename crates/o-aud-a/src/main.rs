//! Standalone O-AUD-A, built with `--features standalone`: analyzes an audio input device
//! without a plugin host. Its output stays silent, so it never plays the input back.

use aud_ui::window_icon;
use nih_plug::nih_export_standalone;
use o_aud_a::{OAudA, set_silent_output};

fn main() {
    set_silent_output(true);
    window_icon::apply_when_open("O-AUD-A");
    nih_export_standalone::<OAudA>();
}
