//! Shared editor look and controls of the O-AT editors, and the plugins' CLAP and VST3
//! entry points (`export_plugin!`, with the `nih-plug` feature).

pub mod control;
#[cfg(feature = "nih-plug")]
mod export;
pub mod name_window;
#[cfg(feature = "nih-plug")]
pub mod param;
pub mod theme;
pub mod widgets;
pub mod window_icon;
