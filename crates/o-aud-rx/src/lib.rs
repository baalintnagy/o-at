//! O-AUD-RX: receives a named audio bus from O-AUD-TX and plays it.
//!
//! The receiver core (this crate without features) is shared by the O-AUD-RX plugin and
//! the standalone O-AUD-RX application. A [`link`] control thread finds the bus and
//! prepares [`connection`]s; the real-time [`engine`] plays them, resampling to the
//! output rate and locking to the transmitter's clock (see [`sync`]).

pub mod connection;
pub mod engine;
pub mod link;
#[cfg(feature = "plugin")]
pub mod plugin;
pub mod settings;
pub mod sync;
pub mod telemetry;
#[cfg(feature = "view")]
pub mod view;
