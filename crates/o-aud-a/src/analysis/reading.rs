//! What the worker publishes and the editor draws: the measurements of each signal, and
//! the hand-over that moves new data to the editor without copying what did not change.

use std::collections::VecDeque;
use std::mem;

use crate::analysis::bus::SourceReading;
use crate::analysis::loudness::LoudnessReading;
use crate::analysis::spectrum::{SonogramFeed, SpectrumFrame, SurfaceFrame};
use crate::analysis::stereo::StereoReading;
use crate::settings::HistorySpan;

/// Loudness history entries per second.
pub const LOUDNESS_HISTORY_RATE_HZ: f32 = 10.0;
/// Loudness history length in entries: the longest span.
pub const LOUDNESS_HISTORY_LEN: usize = (600.0 * LOUDNESS_HISTORY_RATE_HZ) as usize;

/// One loudness history entry: momentary and short-term loudness in LUFS.
pub type LoudnessPoint = [f32; 2];

/// Duration of an envelope bucket in seconds.
pub const ENVELOPE_BUCKET_S: f32 = 0.001;
/// Envelope buckets kept, a little over a minute.
pub const ENVELOPE_LEN: usize = 65_536;

/// The minimum and maximum of each [`Signal`](crate::settings::Signal) over one envelope
/// bucket, indexed by [`Choice::index`](crate::settings::Choice::index).
pub type EnvelopeBucket = [[f32; 2]; 4];

/// Longest loudness history span in entries for `span`.
pub fn history_entries(span: HistorySpan) -> usize {
    (span.seconds() * LOUDNESS_HISTORY_RATE_HZ) as usize
}

/// The large parts of a [`SignalReading`] the worker rewrote since the editor last took
/// them; the editor takes only those.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fresh {
    pub spectrum: bool,
    pub scope: bool,
    pub surface: bool,
    pub loudness_history: bool,
}

impl Fresh {
    pub const ALL: Self = Self {
        spectrum: true,
        scope: true,
        surface: true,
        loudness_history: true,
    };
}

/// The measurements of one signal, the input or a bus, for the editor.
#[derive(Clone, Debug, Default)]
pub struct SignalReading {
    /// Stream format; zero before the stream starts.
    pub sample_rate: u32,
    pub channels: u32,
    pub spectrum: SpectrumFrame,
    pub sonogram: SonogramFeed,
    /// The spectral view's sonogram layer, analyzed with its own settings.
    pub spectral: SonogramFeed,
    pub surface: SurfaceFrame,
    /// The latest frames for the stereo scope and the oscilloscope, oldest first: at
    /// least [`ScopeSamples::MAX`](crate::settings::ScopeSamples::MAX), more for long oscilloscope spans.
    pub scope: Vec<[f32; 2]>,
    pub stereo: StereoReading,
    pub loudness: LoudnessReading,
    /// Loudness every 100 ms, oldest first.
    pub loudness_history: VecDeque<LoudnessPoint>,
    /// New oscilloscope envelope buckets, oldest first; the editor takes them.
    pub envelope: VecDeque<EnvelopeBucket>,
    /// Changes when earlier envelope buckets no longer belong to this signal.
    pub envelope_generation: u64,
    /// What the worker rewrote since the editor last took it.
    pub fresh: Fresh,
}

impl SignalReading {
    /// An empty reading that replaces whatever the editor has.
    pub fn cleared() -> Self {
        Self {
            fresh: Fresh::ALL,
            ..Self::default()
        }
    }

    /// Takes the new results out of `published`: the parts it rewrote are swapped in, so
    /// nothing is copied and the worker reuses the editor's previous buffers; sonogram
    /// columns and envelope buckets are moved.
    pub fn take_from(&mut self, published: &mut Self) {
        self.sample_rate = published.sample_rate;
        self.channels = published.channels;
        let fresh = mem::take(&mut published.fresh);
        if fresh.spectrum {
            mem::swap(&mut self.spectrum, &mut published.spectrum);
        }
        if fresh.scope {
            mem::swap(&mut self.scope, &mut published.scope);
        }
        if fresh.surface {
            mem::swap(&mut self.surface, &mut published.surface);
        }
        if fresh.loudness_history {
            mem::swap(&mut self.loudness_history, &mut published.loudness_history);
        }
        take_feed(&mut self.sonogram, &mut published.sonogram);
        take_feed(&mut self.spectral, &mut published.spectral);
        self.stereo = published.stereo;
        self.loudness = published.loudness;
        if self.envelope_generation != published.envelope_generation {
            self.envelope.clear();
            self.envelope_generation = published.envelope_generation;
        }
        self.envelope.extend(published.envelope.drain(..));
    }
}

/// Moves a sonogram feed's new columns, with its mapping.
fn take_feed(data: &mut SonogramFeed, published: &mut SonogramFeed) {
    data.generation = published.generation;
    data.range_hz = published.range_hz;
    data.scale = published.scale;
    data.lanes = published.lanes;
    data.columns.extend(published.columns.drain(..));
}

/// Everything the editor draws.
#[derive(Clone, Debug, Default)]
pub struct Published {
    pub input: SignalReading,
    /// Input frames the worker could not take in time.
    pub dropped_frames: u64,
    /// The listed buses, in the order of the
    /// [`SourceList`](crate::sources::SourceList).
    pub sources: Vec<SourceReading>,
    /// Names of the buses that have a transmitter, for adding sources.
    pub live_buses: Vec<String>,
    /// Changes with every update.
    pub sequence: u64,
}

impl Published {
    /// Takes the new results out of `published`, unless nothing changed since the last
    /// time.
    pub fn take_from(&mut self, published: &mut Self) {
        if self.sequence == published.sequence {
            return;
        }
        self.input.take_from(&mut published.input);
        self.dropped_frames = published.dropped_frames;
        self.sources
            .resize_with(published.sources.len(), SourceReading::default);
        for (data, published) in self.sources.iter_mut().zip(&mut published.sources) {
            data.name.clone_from(&published.name);
            data.state = published.state;
            data.gaps = published.gaps;
            data.signal.take_from(&mut published.signal);
        }
        self.live_buses.clone_from(&published.live_buses);
        self.sequence = published.sequence;
    }
}

#[cfg(test)]
mod tests {
    use super::{Fresh, SignalReading};

    #[test]
    fn only_fresh_parts_are_taken() {
        let mut published = SignalReading {
            scope: vec![[1.0, 1.0]; 4],
            fresh: Fresh {
                scope: true,
                ..Fresh::default()
            },
            ..SignalReading::default()
        };
        published.loudness_history.push_back([-20.0, -21.0]);
        let mut data = SignalReading::default();
        data.take_from(&mut published);
        assert_eq!(data.scope.len(), 4, "the fresh scope frames are taken");
        assert!(
            data.loudness_history.is_empty(),
            "the history was not rewritten"
        );
        assert_eq!(
            published.fresh,
            Fresh::default(),
            "taken parts are no longer fresh"
        );
        // Taking again keeps what the editor has.
        data.take_from(&mut published);
        assert_eq!(data.scope.len(), 4);
    }
}
