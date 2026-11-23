//! Standalone application wrapper, built with `--features standalone`.

use aud_ui::window_icon;
use nih_plug::nih_export_standalone;
use o_nr::ONr;

fn main() {
    window_icon::apply_when_open("O-NR");
    nih_export_standalone::<ONr>();
}
