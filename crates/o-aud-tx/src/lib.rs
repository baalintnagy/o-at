//! O-AUD-TX: sends audio to a named audio bus, where O-AUD-RX receivers in other hosts or
//! the standalone O-AUD-RX application play it.
//!
//! The transmitter core (this crate without features) is shared by the O-AUD-TX plugin
//! and the standalone O-AUD-TX application. A [`link`] control thread owns the bus name
//! and hands bus writers to the real-time [`port`]; [`interleaved`] feeds the port from
//! interleaved device buffers.

pub mod interleaved;
pub mod link;
#[cfg(feature = "plugin")]
pub mod plugin;
pub mod port;
pub mod settings;
pub mod telemetry;
#[cfg(feature = "view")]
pub mod view;
