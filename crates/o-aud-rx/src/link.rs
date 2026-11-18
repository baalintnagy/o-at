//! The control-thread side of a receiver: finds the selected bus, builds connections for
//! it and hands them to the audio thread.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use audbus::handoff::{HandoffSender, handoff};
use audbus::{
    BusError, BusInfo, BusName, BusReader, DEFAULT_NAMESPACE, Directory, NameError, Presence,
    RENEW_INTERVAL_MS, StreamFormat,
};

use crate::connection::Connection;
use crate::engine::Receiver;
use crate::telemetry::RxTelemetry;

/// How often the link checks for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How often the bus list for the editor is refreshed.
const LIST_INTERVAL: Duration = Duration::from_millis(250);

/// What the link is doing, for the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// No bus is selected.
    NoBus,
    InvalidName(NameError),
    /// The selected bus has no transmitter yet.
    Waiting,
    /// Connected to the transmitter of the selected bus.
    Connected,
    Error(BusError),
}

/// The link's state and the buses available to choose from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStatus {
    pub state: LinkState,
    pub buses: Vec<BusInfo>,
}

struct Shared {
    namespace: String,
    name: Arc<RwLock<String>>,
    device_rate: AtomicU32,
    stop: AtomicBool,
    status: Mutex<LinkStatus>,
    /// Connection channel of a replacement audio side, taken over by the worker.
    replacement: Mutex<Option<HandoffSender<Connection>>>,
    telemetry: Arc<RxTelemetry>,
}

/// Owns the control thread; dropping it stops the thread.
pub struct RxLink {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

/// Creates a receiver for the bus named in `name`, which the link rereads continuously
/// (a plugin passes its persisted name field). Returns the control side, which must
/// outlive playback, and the audio side.
pub fn receiver(name: Arc<RwLock<String>>, telemetry: Arc<RxTelemetry>) -> (RxLink, Receiver) {
    receiver_in(DEFAULT_NAMESPACE, name, telemetry)
}

/// Like [`receiver`], for buses in a bus directory `namespace`.
pub fn receiver_in(
    namespace: &str,
    name: Arc<RwLock<String>>,
    telemetry: Arc<RxTelemetry>,
) -> (RxLink, Receiver) {
    let (sender, connections) = handoff();
    let shared = Arc::new(Shared {
        namespace: namespace.to_owned(),
        name,
        device_rate: AtomicU32::new(0),
        stop: AtomicBool::new(false),
        status: Mutex::new(LinkStatus {
            state: LinkState::NoBus,
            buses: Vec::new(),
        }),
        replacement: Mutex::new(None),
        telemetry: Arc::clone(&telemetry),
    });
    let worker_shared = Arc::clone(&shared);
    let worker = thread::Builder::new()
        .name(String::from("o-aud-rx link"))
        .spawn(move || Worker::new(worker_shared, sender).run())
        .ok();
    (
        RxLink { shared, worker },
        Receiver::new(connections, telemetry),
    )
}

impl RxLink {
    /// Sets the output rate in Hz; zero pauses connecting.
    pub fn set_device_rate(&self, device_rate: u32) {
        self.shared
            .device_rate
            .store(device_rate, Ordering::Relaxed);
        self.wake();
    }

    /// The selected bus name as entered.
    pub fn name(&self) -> String {
        self.shared
            .name
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Creates a new audio side and connects it instead of the previous one, for an
    /// application that rebuilds its audio stream on another device. Drop the previous
    /// [`Receiver`] first; it no longer receives connections.
    pub fn replace_receiver(&self) -> Receiver {
        let (sender, connections) = handoff();
        *self
            .shared
            .replacement
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(sender);
        self.wake();
        Receiver::new(connections, Arc::clone(&self.shared.telemetry))
    }

    /// Selects a bus; an empty name disconnects.
    pub fn set_name(&self, name: &str) {
        *self
            .shared
            .name
            .write()
            .unwrap_or_else(PoisonError::into_inner) = name.trim().to_owned();
        self.wake();
    }

    pub fn status(&self) -> LinkStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn wake(&self) {
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
}

impl Drop for RxLink {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

/// The connection currently owned by the audio thread.
struct Active {
    name: BusName,
    format: StreamFormat,
    device_rate: u32,
    /// Second handle on the same bus, for watching its format.
    probe: BusReader,
    presence: Presence,
}

struct Worker {
    shared: Arc<Shared>,
    sender: HandoffSender<Connection>,
    directory: Option<Arc<Directory>>,
    active: Option<Active>,
    last_list: Option<Instant>,
    last_renew: Instant,
    buses: Vec<BusInfo>,
}

impl Worker {
    fn new(shared: Arc<Shared>, sender: HandoffSender<Connection>) -> Self {
        Self {
            shared,
            sender,
            directory: None,
            active: None,
            last_list: None,
            last_renew: Instant::now(),
            buses: Vec::new(),
        }
    }

    fn run(mut self) {
        while !self.shared.stop.load(Ordering::Relaxed) {
            let state = self.step();
            self.publish(state);
            thread::park_timeout(POLL_INTERVAL);
        }
    }

    fn step(&mut self) -> LinkState {
        let replacement = self
            .shared
            .replacement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(sender) = replacement {
            // The previous receiver owned the active connection.
            self.sender = sender;
            self.active = None;
        }
        self.sender.collect();
        let directory = match &self.directory {
            Some(directory) => Arc::clone(directory),
            None => match Directory::open_namespace(&self.shared.namespace) {
                Ok(directory) => {
                    self.directory = Some(Arc::clone(&directory));
                    directory
                }
                Err(error) => return LinkState::Error(error),
            },
        };
        if self
            .last_list
            .is_none_or(|time| time.elapsed() >= LIST_INTERVAL)
        {
            self.buses = directory.buses();
            self.last_list = Some(Instant::now());
        }

        let text = self
            .shared
            .name
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let desired = if text.trim().is_empty() {
            None
        } else {
            match BusName::new(&text) {
                Ok(name) => Some(name),
                Err(error) => {
                    self.disconnect();
                    return LinkState::InvalidName(error);
                }
            }
        };
        let device_rate = self.shared.device_rate.load(Ordering::Relaxed);

        if let Some(active) = &self.active {
            let current = desired.as_ref() == Some(&active.name)
                && active.device_rate == device_rate
                && active.probe.format() == Some(active.format)
                && directory.find(&active.name).is_some();
            if !current {
                self.disconnect();
            }
        }
        let Some(name) = desired else {
            self.disconnect();
            return LinkState::NoBus;
        };
        if self.active.is_none() && device_rate > 0 && directory.find(&name).is_some() {
            if let Err(error) = self.connect(&directory, name, device_rate) {
                return LinkState::Error(error);
            }
        }
        match &mut self.active {
            Some(active) => {
                if self.last_renew.elapsed() >= Duration::from_millis(RENEW_INTERVAL_MS) {
                    active.presence.renew();
                    self.last_renew = Instant::now();
                }
                LinkState::Connected
            }
            None => LinkState::Waiting,
        }
    }

    fn connect(
        &mut self,
        directory: &Directory,
        name: BusName,
        device_rate: u32,
    ) -> Result<(), BusError> {
        let probe = match BusReader::open(directory, &name) {
            Ok(reader) => reader,
            // The transmitter may not have created its region yet.
            Err(BusError::NotFound) => return Ok(()),
            Err(error) => return Err(error),
        };
        let Some(format) = probe.format() else {
            return Ok(());
        };
        let Ok(connection) = Connection::new(probe.clone(), format, device_rate) else {
            return Err(BusError::Incompatible);
        };
        if self.sender.send(Some(connection)).is_ok() {
            let mut presence = probe.presence();
            presence.renew();
            self.last_renew = Instant::now();
            self.active = Some(Active {
                name,
                format,
                device_rate,
                probe,
                presence,
            });
        }
        Ok(())
    }

    fn disconnect(&mut self) {
        if self.active.is_some() && self.sender.send(None).is_ok() {
            self.active = None;
        }
    }

    fn publish(&self, state: LinkState) {
        let mut status = self
            .shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        status.state = state;
        if status.buses != self.buses {
            status.buses.clone_from(&self.buses);
        }
    }
}
