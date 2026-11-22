//! The analysis: a worker thread drains the capture ring and runs every measurement off
//! the audio thread.
//!
//! Loudness and the stereo meters run all the time, so integrated loudness keeps counting
//! while the editor is closed. The FFT analyses only run while an editor draws (see
//! [`AnalysisShared::mark_viewed`]), and only for the views that show the signal. Results
//! go to the editor through a mutex that only the worker and the editor take; the audio
//! thread never touches it.
//!
//! - [`worker`]: the thread, the state it shares with the editor and the bus list.
//! - [`engine`]: the measurements of one signal, for the input and for each bus.
//! - [`reading`]: what is published, and how the editor takes it.
//! - [`bus`]: reading the buses listed in the [`SourceList`](crate::sources::SourceList).
//! - [`fft`], [`spectrum`], [`loudness`], [`stereo`]: the analyses themselves.

pub mod bus;
pub mod engine;
pub mod fft;
pub mod loudness;
pub mod reading;
pub mod spectrum;
pub mod stereo;
pub mod worker;

pub use engine::{Engine, Wants, scope_frames};
pub use reading::{
    ENVELOPE_BUCKET_S, ENVELOPE_LEN, EnvelopeBucket, Fresh, LOUDNESS_HISTORY_LEN,
    LOUDNESS_HISTORY_RATE_HZ, LoudnessPoint, Published, SignalReading, history_entries,
};
pub use worker::{AnalysisShared, Analyzer};
