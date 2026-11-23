//! A route: one audio source sent to one bus.
//!
//! Each route has its own transmitter link, telemetry and capture controller, so routes
//! run independently, like several O-AUD-TX plugins in one host.

use std::sync::Arc;

use o_aud_tx::link::{TxLink, transmitter};
use o_aud_tx::telemetry::TxTelemetry;
use o_aud_tx::view::TxViewState;

use crate::capture::Capture;
use crate::settings::RouteSettings;

pub struct Route {
    // Declared first, so it drops first: its streams and ports go before the link.
    capture: Capture,
    pub settings: Arc<RouteSettings>,
    pub link: Arc<TxLink>,
    pub telemetry: Arc<TxTelemetry>,
    /// Editor state, kept while another route is shown so its meters keep up.
    pub view: TxViewState,
}

impl Route {
    /// Starts sending the source in `settings` to its bus.
    pub fn start(settings: Arc<RouteSettings>) -> Self {
        let telemetry = Arc::new(TxTelemetry::default());
        // The capture takes a port per stream, so this first one is not used.
        let (link, _unused) = transmitter(Arc::clone(&settings.bus_name), Arc::clone(&telemetry));
        let link = Arc::new(link);
        let capture = Capture::start(Arc::clone(&link), Arc::clone(&settings));
        Self {
            capture,
            settings,
            link,
            telemetry,
            view: TxViewState::default(),
        }
    }

    pub fn capture(&self) -> &Capture {
        &self.capture
    }
}
