//! DeepFilterNet3 noise suppression (via libDF), run on a worker thread.
//!
//! libDF's inference allocates on every frame, so it cannot run on the audio thread.
//! The audio thread hands each 10 ms frame to a worker through a lock-free ring buffer
//! and picks up the result of the frame submitted [`DeepFilter::pipeline_frames`]
//! frames earlier. That pipeline delay is constant and part of the reported latency.
//! When a result is not ready in time (CPU overload), the frame is reported as missing
//! and the caller substitutes a time-aligned fallback.
//!
//! The worker loads the model when it receives its first frame, so instances that never
//! use DeepFilterNet do not pay for it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::dsp::FRAME;

/// DeepFilterNet3's own delay in samples: one frame of STFT overlap plus two frames of
/// lookahead.
pub const MODEL_DELAY: usize = 3 * FRAME;

/// Frames that can wait for the worker before new frames are dropped.
const QUEUE_FRAMES: usize = 64;

/// How long the worker sleeps between checks when no frame wakes it.
const IDLE_WAIT: Duration = Duration::from_millis(20);

/// Longest wait for a result when processing offline, which covers loading the model.
const OFFLINE_TIMEOUT: Duration = Duration::from_secs(10);

/// libDF skips frames with a mean square below 1e-7 without advancing its state, which
/// would replay stale audio afterwards. Near-silent frames get this much dither
/// (amplitude of uniform noise, about −60 dBFS RMS) so the model always advances.
const DITHER: f32 = 1.7e-3;
const SILENCE_MEAN_SQUARE: f32 = 1.0e-6;

/// How results are awaited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// The host calls in real time: never wait; late results are dropped.
    Realtime,
    /// The host renders offline: wait for every result.
    Offline,
}

struct Job {
    sequence: u64,
    samples: [f32; FRAME],
}

struct Shared {
    stop: AtomicBool,
    ready: AtomicBool,
    failed: AtomicBool,
}

pub struct DeepFilter {
    jobs: Producer<Job>,
    results: Consumer<Job>,
    shared: Arc<Shared>,
    worker_thread: Thread,
    worker: Option<JoinHandle<()>>,
    next_sequence: u64,
    pipeline_frames: usize,
    pacing: Pacing,
}

impl DeepFilter {
    /// Starts the worker thread. Allocates; call it outside the audio callback.
    pub fn new(pipeline_frames: usize, pacing: Pacing) -> Self {
        let (jobs, worker_jobs) = RingBuffer::new(QUEUE_FRAMES);
        let (worker_results, results) = RingBuffer::new(QUEUE_FRAMES);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name(String::from("O-NEC DeepFilterNet"))
            .spawn(move || run_worker(worker_jobs, worker_results, &worker_shared))
            .ok();
        if worker.is_none() {
            shared.failed.store(true, Ordering::Release);
        }
        let worker_thread = worker
            .as_ref()
            .map_or_else(thread::current, |handle| handle.thread().clone());
        Self {
            jobs,
            results,
            shared,
            worker_thread,
            worker,
            next_sequence: 0,
            pipeline_frames: pipeline_frames.max(1),
            pacing,
        }
    }

    /// Changes the pipeline depth and pacing. Restarts the model's stream.
    pub fn set_pipeline(&mut self, pipeline_frames: usize, pacing: Pacing) {
        self.pipeline_frames = pipeline_frames.max(1);
        self.pacing = pacing;
        self.reset();
    }

    /// Frames between submitting a frame and using its result.
    pub fn pipeline_frames(&self) -> usize {
        self.pipeline_frames
    }

    /// Total delay of [`DeepFilter::process`] in samples.
    pub fn delay(&self) -> usize {
        MODEL_DELAY + self.pipeline_frames * FRAME
    }

    /// Whether the model is loaded.
    pub fn is_ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire)
    }

    /// Whether the model could not be loaded or the worker could not start.
    pub fn has_failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }

    /// Starts a new stream: the worker restarts the model on the sequence gap, and
    /// pending results are discarded.
    pub fn reset(&mut self) {
        self.next_sequence += self.pipeline_frames as u64 + 1;
    }

    /// Submits `input` and writes the result of the frame submitted
    /// [`DeepFilter::pipeline_frames`] frames ago into `output`. Returns `false`, leaving
    /// `output` unchanged, when that result is not available.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> bool {
        debug_assert_eq!(input.len(), FRAME);
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let mut samples = [0.0; FRAME];
        samples.copy_from_slice(input);
        // A full queue means the worker stalled; the frame is dropped and the worker
        // restarts the model on the sequence gap.
        if self.jobs.push(Job { sequence, samples }).is_ok() {
            self.worker_thread.unpark();
        }

        let Some(wanted) = sequence.checked_sub(self.pipeline_frames as u64) else {
            return false;
        };
        let deadline = match self.pacing {
            Pacing::Realtime => None,
            Pacing::Offline => Some(Instant::now() + OFFLINE_TIMEOUT),
        };
        loop {
            while let Ok(job) = self.results.peek() {
                if job.sequence > wanted {
                    return false;
                }
                let found = job.sequence == wanted;
                if found {
                    output.copy_from_slice(&job.samples);
                }
                let _ = self.results.pop();
                if found {
                    return true;
                }
            }
            match deadline {
                Some(deadline) if Instant::now() < deadline && !self.has_failed() => {
                    thread::yield_now();
                }
                _ => return false,
            }
        }
    }
}

impl Drop for DeepFilter {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

struct Model {
    /// Freshly initialized model, cloned to restart the stream.
    pristine: DfTract,
    active: DfTract,
}

fn load_model() -> Option<Model> {
    let params = DfParams::default();
    let pristine = DfTract::new(params, &RuntimeParams::default_with_ch(1)).ok()?;
    let active = pristine.clone();
    Some(Model { pristine, active })
}

fn run_worker(mut jobs: Consumer<Job>, mut results: Producer<Job>, shared: &Shared) {
    let mut model: Option<Model> = None;
    let mut previous: Option<u64> = None;
    let mut noisy = Array2::<f32>::zeros((1, FRAME));
    let mut enhanced = Array2::<f32>::zeros((1, FRAME));
    let mut dither = Dither(0x2545_f491);

    while !shared.stop.load(Ordering::Acquire) {
        let Ok(mut job) = jobs.pop() else {
            thread::park_timeout(IDLE_WAIT);
            continue;
        };
        if model.is_none() && !shared.failed.load(Ordering::Acquire) {
            model = load_model();
            match model {
                Some(_) => shared.ready.store(true, Ordering::Release),
                None => shared.failed.store(true, Ordering::Release),
            }
        }
        let Some(model) = model.as_mut() else {
            continue;
        };
        if previous.is_none_or(|previous| job.sequence != previous + 1) {
            model.active = model.pristine.clone();
        }
        previous = Some(job.sequence);

        let mean_square = job
            .samples
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
            / FRAME as f32;
        let quiet = mean_square < SILENCE_MEAN_SQUARE;
        for (target, sample) in noisy.iter_mut().zip(&job.samples) {
            *target = if quiet {
                sample + DITHER * dither.next()
            } else {
                *sample
            };
        }
        match model.active.process(noisy.view(), enhanced.view_mut()) {
            Ok(_) => {
                for (target, sample) in job.samples.iter_mut().zip(enhanced.iter()) {
                    *target = *sample;
                }
            }
            Err(_) => job.samples.fill(0.0),
        }
        // A full queue means the audio thread stopped collecting; drop the result.
        let _ = results.push(job);
    }
}

/// Uniform noise in −1..1 (xorshift).
struct Dither(u32);

impl Dither {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}
