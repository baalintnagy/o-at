//! Buses analyzed next to the input: each is read from shared memory on the analysis
//! worker and measured at its own rate by an [`Engine`] of its own, so no resampling or
//! clock locking is needed and every view can show it.
//!
//! The sources are not sample-aligned with the input or each other: each bus adds its
//! transmitter's block size and up to one worker round of delay. Views that overlay
//! several signals compare them, they do not line them up.

use std::time::{Duration, Instant};

use audbus::{BusName, BusReader, Directory, Presence, SAFE_WINDOW_FRAMES};

use crate::analysis::{Engine, SignalReading, Wants};
use crate::settings::AnalyzerSettings;

/// Frames read from a bus at once.
const READ_FRAMES: usize = 4_096;
/// How often a bus without a region yet is looked for.
const OPEN_INTERVAL: Duration = Duration::from_millis(500);
/// How often the directory is asked whether the bus has a transmitter.
const LIVE_INTERVAL: Duration = Duration::from_millis(250);
/// How often the reader registration is renewed, so the transmitter counts the analyzer
/// as a listener.
const PRESENCE_INTERVAL: Duration = Duration::from_millis(200);
/// Unread frames beyond which a source skips ahead to the newest audio: half the safely
/// readable part of the ring.
const SKIP_FRAMES: u64 = SAFE_WINDOW_FRAMES / 2;

/// What a source is doing, for the editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceState {
    /// No transmitter uses the name.
    #[default]
    NoTransmitter,
    /// The transmitter exists but has not started its stream.
    Waiting,
    /// Receiving and measuring.
    Live,
    /// The name is not a valid bus name.
    InvalidName,
}

/// A source's results, for the editor.
#[derive(Clone, Debug, Default)]
pub struct SourceReading {
    /// The bus name as listed.
    pub name: String,
    pub state: SourceState,
    /// Times the source skipped audio because it could not be read in time.
    pub gaps: u64,
    /// The bus's measurements; its format is zero before its stream starts.
    pub signal: SignalReading,
}

/// Reads and measures one bus.
pub struct BusTap {
    /// The name as listed in the settings.
    listed: String,
    name: Option<BusName>,
    reader: Option<BusReader>,
    presence: Option<Presence>,
    last_open: Option<Instant>,
    last_live: Option<Instant>,
    last_presence: Instant,
    live: bool,
    /// The stream being read and the next frame to read.
    epoch: Option<u32>,
    position: u64,
    engine: Option<Engine>,
    state: SourceState,
    gaps: u64,
    /// Planar samples of one read.
    scratch: Vec<f32>,
    /// Whether the reading must be published whole, for example after a restart.
    changed: bool,
}

impl BusTap {
    pub fn new(listed: &str) -> Self {
        let name = BusName::new(listed.trim()).ok();
        Self {
            listed: listed.to_owned(),
            state: if name.is_some() {
                SourceState::NoTransmitter
            } else {
                SourceState::InvalidName
            },
            name,
            reader: None,
            presence: None,
            last_open: None,
            last_live: None,
            last_presence: Instant::now(),
            live: false,
            epoch: None,
            position: 0,
            engine: None,
            gaps: 0,
            scratch: Vec::new(),
            changed: true,
        }
    }

    /// The name as listed in the settings.
    pub fn listed(&self) -> &str {
        &self.listed
    }

    pub fn reset_loudness(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset_loudness();
        }
    }

    pub fn reset_peaks(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset_peaks();
        }
    }

    /// Reads everything the transmitter wrote since the last call. The FFT analyses in
    /// `wants` only run while `view` is set.
    pub fn poll(
        &mut self,
        directory: &Directory,
        (settings, wants): (&AnalyzerSettings, Wants),
        view: bool,
    ) {
        let Some(name) = &self.name else {
            return;
        };
        if self
            .last_live
            .is_none_or(|time| time.elapsed() >= LIVE_INTERVAL)
        {
            self.live = directory.find(name).is_some();
            self.last_live = Some(Instant::now());
        }
        if self.reader.is_none() {
            if !self.live
                || self
                    .last_open
                    .is_some_and(|time| time.elapsed() < OPEN_INTERVAL)
            {
                self.set_state(SourceState::NoTransmitter);
                return;
            }
            self.last_open = Some(Instant::now());
            // The transmitter may not have created its region yet.
            let Ok(reader) = BusReader::open(directory, name) else {
                self.set_state(SourceState::Waiting);
                return;
            };
            let mut presence = reader.presence();
            presence.renew();
            self.last_presence = Instant::now();
            self.presence = Some(presence);
            self.reader = Some(reader);
        }
        if self.last_presence.elapsed() >= PRESENCE_INTERVAL {
            if let Some(presence) = &mut self.presence {
                presence.renew();
            }
            self.last_presence = Instant::now();
        }
        self.read(settings, wants, view);
    }

    fn set_state(&mut self, state: SourceState) {
        if self.state != state {
            self.state = state;
            self.changed = true;
        }
    }

    fn read(&mut self, settings: &AnalyzerSettings, wants: Wants, view: bool) {
        let Some(reader) = &self.reader else {
            return;
        };
        // Keeps the region open across transmitter restarts, which continue on the
        // same ring.
        let Some(format) = reader.format() else {
            self.set_state(if self.live {
                SourceState::Waiting
            } else {
                SourceState::NoTransmitter
            });
            return;
        };
        let written = reader.write_position();
        let engine = match &mut self.engine {
            Some(engine) if self.epoch == Some(format.epoch) => engine,
            slot => {
                // A new stream: start at its newest audio.
                self.epoch = Some(format.epoch);
                self.position = written;
                self.changed = true;
                let channels = format.channels.clamp(1, 2) as u32;
                slot.insert(Engine::new(format.sample_rate, channels, settings))
            }
        };
        engine.configure(settings);
        engine.set_wants(wants);
        if written < self.position || written - self.position > SKIP_FRAMES {
            self.position = written;
            self.gaps += 1;
        }
        let channels = format.channels;
        while self.position < written {
            let frames = ((written - self.position) as usize).min(READ_FRAMES);
            self.scratch.resize(frames * channels, 0.0);
            if !reader.read(
                self.position,
                frames,
                channels,
                format.epoch,
                &mut self.scratch,
            ) {
                // Overwritten while copying, or the stream changed: start again next
                // round from the newest audio.
                self.position = reader.write_position();
                self.gaps += 1;
                break;
            }
            let second = if channels > 1 { frames } else { 0 };
            for index in 0..frames {
                engine.process([self.scratch[index], self.scratch[second + index]], view);
            }
            self.position += frames as u64;
        }
        self.set_state(if self.live {
            SourceState::Live
        } else {
            SourceState::NoTransmitter
        });
    }

    /// Copies the results into `reading`; `whole` rewrites everything, for a reading that
    /// held another bus.
    pub fn publish(&mut self, reading: &mut SourceReading, whole: bool) {
        let whole = whole || self.changed;
        if whole {
            reading.name.clone_from(&self.listed);
        }
        reading.state = self.state;
        reading.gaps = self.gaps;
        match &mut self.engine {
            Some(engine) => engine.publish(&mut reading.signal, whole),
            None if whole => reading.signal = SignalReading::cleared(),
            None => {}
        }
        self.changed = false;
    }
}
