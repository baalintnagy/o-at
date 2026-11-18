//! Standalone O-AUD-TX, built with `--features standalone`: sends an audio input device to
//! a bus without a plugin host. `--backend dummy` sends silence, for testing.

use aud_ui::window_icon;
use nih_plug::nih_export_standalone;
use o_aud_tx::OAudTx;

fn main() {
    window_icon::apply_when_open("O-AUD-TX");
    nih_export_standalone::<OAudTx>();
}
