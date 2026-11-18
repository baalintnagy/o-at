//! The control-thread side of a transmitter: owns the bus name and hands bus writers to
//! the audio thread.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use audbus::handoff::{HandoffSender, handoff};
use audbus::{
    BusError, BusInfo, BusMonitor, BusName, BusWriter, DEFAULT_NAMESPACE, Directory, Lease,
    NameError, RENEW_INTERVAL_MS,
};

use crate::port::TxPort;
use crate::telemetry::TxTelemetry;

/// How often the link checks for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How often the bus list for the editor is refreshed.
const LIST_INTERVAL: Duration = Duration::from_millis(250);
/// After this long without the audio thread taking over a replacement, it is not
/// processing, so the retired writer can no longer be in use.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(100);
/// Prefix of automatically assigned names.
const DEFAULT_PREFIX: &str = "Bus";

/// What the link is doing, for the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxState {
    /// The plugin has not been activated yet.
    Inactive,
    InvalidName(NameError),
    /// Another transmitter owns the name; `suggestion` is free.
    Taken {
        suggestion: String,
    },
    /// Transmitting on the bus.
    Live,
    Error(BusError),
}

/// The link's state for the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxStatus {
    pub state: TxState,
    /// Receivers playing the bus.
    pub listeners: usize,
    /// All live buses, including this one.
    pub buses: Vec<BusInfo>,
}

struct Shared {
    namespace: String,
    name: Arc<RwLock<String>>,
    sample_rate: AtomicU32,
    channels: AtomicU32,
    stop: AtomicBool,
    status: Mutex<TxStatus>,
}

/// Owns the control thread; dropping it stops the thread and frees the name.
pub struct TxLink {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

/// Creates a transmitter for the bus named in `name`, which the link rereads
/// continuously (the plugin passes its persisted name field). An empty name is replaced
/// by a free `Bus n` name once the stream format is known.
pub fn transmitter(name: Arc<RwLock<String>>, telemetry: Arc<TxTelemetry>) -> (TxLink, TxPort) {
    transmitter_in(DEFAULT_NAMESPACE, name, telemetry)
}

/// Like [`transmitter`], for buses in a bus directory `namespace`.
pub fn transmitter_in(
    namespace: &str,
    name: Arc<RwLock<String>>,
    telemetry: Arc<TxTelemetry>,
) -> (TxLink, TxPort) {
    let (sender, writers) = handoff();
    let shared = Arc::new(Shared {
        namespace: namespace.to_owned(),
        name,
        sample_rate: AtomicU32::new(0),
        channels: AtomicU32::new(0),
        stop: AtomicBool::new(false),
        status: Mutex::new(TxStatus {
            state: TxState::Inactive,
            listeners: 0,
            buses: Vec::new(),
        }),
    });
    let worker_shared = Arc::clone(&shared);
    let worker = thread::Builder::new()
        .name(String::from("o-aud-tx link"))
        .spawn(move || Worker::new(worker_shared, sender).run())
        .ok();
    (TxLink { shared, worker }, TxPort::new(writers, telemetry))
}

impl TxLink {
    /// Announces the stream format: rate in Hz and channel count. Transmitting starts
    /// once the rate is known.
    pub fn set_format(&self, sample_rate: u32, channels: usize) {
        self.shared
            .sample_rate
            .store(sample_rate, Ordering::Relaxed);
        self.shared
            .channels
            .store(channels as u32, Ordering::Relaxed);
        self.wake();
    }

    /// The bus name as entered.
    pub fn name(&self) -> String {
        self.shared
            .name
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_name(&self, name: &str) {
        *self
            .shared
            .name
            .write()
            .unwrap_or_else(PoisonError::into_inner) = name.trim().to_owned();
        self.wake();
    }

    pub fn status(&self) -> TxStatus {
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

impl Drop for TxLink {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

/// The bus the audio thread writes to.
struct Owned {
    lease: Lease,
    monitor: BusMonitor,
    format: (u32, u32),
}

/// A lease whose writer was withdrawn but may still be in use by the audio thread.
struct Retiring {
    /// Held until the writer is out of use; dropping it frees the name.
    _lease: Lease,
    since: Instant,
}

struct Worker {
    shared: Arc<Shared>,
    sender: HandoffSender<BusWriter>,
    directory: Option<Arc<Directory>>,
    owned: Option<Owned>,
    retiring: Option<Retiring>,
    last_list: Option<Instant>,
    last_renew: Instant,
    buses: Vec<BusInfo>,
}

impl Worker {
    fn new(shared: Arc<Shared>, sender: HandoffSender<BusWriter>) -> Self {
        Self {
            shared,
            sender,
            directory: None,
            owned: None,
            retiring: None,
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
        self.release();
        // Give the audio thread a chance to retire the writer before the lease goes.
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        while !self.sender.is_settled() && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn step(&mut self) -> TxState {
        self.sender.collect();
        self.finish_retiring();
        let directory = match &self.directory {
            Some(directory) => Arc::clone(directory),
            None => match Directory::open_namespace(&self.shared.namespace) {
                Ok(directory) => {
                    self.directory = Some(Arc::clone(&directory));
                    directory
                }
                Err(error) => return TxState::Error(error),
            },
        };
        if self
            .last_list
            .is_none_or(|time| time.elapsed() >= LIST_INTERVAL)
        {
            self.buses = directory.buses();
            self.last_list = Some(Instant::now());
        }

        let sample_rate = self.shared.sample_rate.load(Ordering::Relaxed);
        let channels = self.shared.channels.load(Ordering::Relaxed);
        if sample_rate == 0 {
            return TxState::Inactive;
        }

        let mut text = self
            .shared
            .name
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if text.trim().is_empty() {
            text = directory.suggest_name(DEFAULT_PREFIX).to_string();
            *self
                .shared
                .name
                .write()
                .unwrap_or_else(PoisonError::into_inner) = text.clone();
        }
        let name = match BusName::new(&text) {
            Ok(name) => name,
            Err(error) => {
                self.release();
                return TxState::InvalidName(error);
            }
        };

        if let Some(owned) = &self.owned {
            // A different spelling of the same name also reclaims it, to show the new one.
            if owned.lease.name().as_str() != name.as_str() {
                self.release();
            }
        }
        if self.owned.is_none() && self.retiring.is_none() {
            if let Err(error) = self.claim(&directory, &name) {
                return match error {
                    BusError::NameTaken => TxState::Taken {
                        suggestion: directory.free_variant(&name).to_string(),
                    },
                    error => TxState::Error(error),
                };
            }
        }
        let Some(owned) = &mut self.owned else {
            return TxState::Inactive;
        };
        if owned.format != (sample_rate, channels) {
            owned.lease.set_format(sample_rate, channels);
            owned.format = (sample_rate, channels);
        }
        if self.last_renew.elapsed() >= Duration::from_millis(RENEW_INTERVAL_MS) {
            self.last_renew = Instant::now();
            if !owned.lease.renew() {
                // The lease expired, for example while the computer slept, and another
                // transmitter took the name.
                self.release();
                return TxState::Taken {
                    suggestion: directory.free_variant(&name).to_string(),
                };
            }
        }
        TxState::Live
    }

    fn claim(&mut self, directory: &Arc<Directory>, name: &BusName) -> Result<(), BusError> {
        let lease = directory.lease(name)?;
        let writer = BusWriter::create(&lease)?;
        let monitor = writer.monitor();
        if self.sender.send(Some(writer)).is_ok() {
            self.last_renew = Instant::now();
            self.owned = Some(Owned {
                lease,
                monitor,
                format: (0, 0),
            });
        }
        Ok(())
    }

    /// Withdraws the writer from the audio thread; the lease is kept until it is no
    /// longer in use, so no other transmitter writes to the same bus meanwhile.
    fn release(&mut self) {
        if self.owned.is_some() && self.sender.send(None).is_ok() {
            if let Some(owned) = self.owned.take() {
                self.retiring = Some(Retiring {
                    _lease: owned.lease,
                    since: Instant::now(),
                });
            }
        }
    }

    fn finish_retiring(&mut self) {
        let done = self.retiring.as_ref().is_some_and(|retiring| {
            self.sender.is_settled() || retiring.since.elapsed() >= SETTLE_TIMEOUT
        });
        if done {
            // Dropping the lease frees the name.
            self.retiring = None;
        }
    }

    fn publish(&self, state: TxState) {
        let mut status = self
            .shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        status.listeners = self
            .owned
            .as_ref()
            .map_or(0, |owned| owned.monitor.listeners());
        status.state = state;
        if status.buses != self.buses {
            status.buses.clone_from(&self.buses);
        }
    }
}
