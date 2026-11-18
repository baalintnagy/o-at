//! The capture path and the analysis worker, end to end.
//!
//! Debug builds install NIH-plug's allocation-checking global allocator
//! (`assert_process_allocs`), which aborts on any allocation inside `assert_no_alloc`.
//! Run these tests without `--release` for the allocation check.

use std::f32::consts::TAU;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use o_aud_a::analysis::Analyzer;
use o_aud_a::capture::capture;
use o_aud_a::settings::{AnalyzerSettings, Averaging, ChannelMode, Window};

const RATE: u32 = 48_000;
const BLOCK: usize = 480;

fn sine_block(start: usize, amplitude: f32) -> Vec<f32> {
    (0..BLOCK)
        .map(|n| amplitude * (TAU * 1_000.0 * (start + n) as f32 / RATE as f32).sin())
        .collect()
}

#[test]
fn capturing_does_not_allocate() {
    let (mut capture, mut consumer, _stream) = capture();
    capture.set_format(RATE, 2);
    let left = sine_block(0, 0.5);
    let right = sine_block(0, 0.25);
    assert_no_alloc(|| {
        // Far more than the ring holds, so the overflow path runs too.
        for _ in 0..1_000 {
            capture.push(&[&left[..], &right[..]]);
        }
        capture.push(&[&left[..]]);
    });
    assert_eq!(consumer.pop(), Ok([left[0], right[0]]));
}

#[test]
fn worker_measures_loudness_spectrum_and_stereo() {
    let (mut capture, consumer, stream) = capture();
    let settings = Arc::new(RwLock::new(AnalyzerSettings::default()));
    if let Ok(mut settings) = settings.write() {
        settings.spectrum.channels = ChannelMode::LeftRight;
        settings.spectrum.averaging = Averaging::Off;
        settings.spectrum.window = Window::FlatTop;
    }
    let analyzer = Analyzer::start(consumer, stream, Arc::clone(&settings));
    let shared = Arc::clone(analyzer.shared());
    capture.set_format(RATE, 2);

    // Five seconds of a stereo -23 dBFS sine, kept within a second of the worker so the
    // ring never overflows.
    let amplitude = 10.0_f32.powf(-23.0 / 20.0);
    let blocks = 5 * RATE as usize / BLOCK;
    let deadline = Instant::now() + Duration::from_secs(60);
    for block in 0..blocks {
        shared.mark_viewed();
        let samples = sine_block(block * BLOCK, amplitude);
        capture.push(&[&samples[..], &samples[..]]);
        let pushed_s = ((block + 1) * BLOCK) as f32 / RATE as f32;
        while pushed_s - shared.with_published(|published| published.loudness.duration_s) > 1.0 {
            assert!(Instant::now() < deadline, "the worker keeps up");
            shared.mark_viewed();
            thread::sleep(Duration::from_millis(2));
        }
    }
    loop {
        shared.mark_viewed();
        let done = shared.with_published(|published| {
            published.loudness.duration_s >= 4.9 && !published.spectrum.primary_db.is_empty()
        });
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "the worker analyzes the stream");
        thread::sleep(Duration::from_millis(10));
    }
    shared.with_published(|published| {
        assert_eq!((published.sample_rate, published.channels), (RATE, 2));
        let loudness = published.loudness;
        assert!(
            (loudness.integrated_lufs + 23.0).abs() < 0.1,
            "integrated {:.2} LUFS",
            loudness.integrated_lufs
        );
        assert!((published.stereo.correlation - 1.0).abs() < 1e-3);
        let spectrum = &published.spectrum;
        let bin = (1_000.0 / spectrum.bin_hz).round() as usize;
        let level = spectrum.primary_db[bin];
        assert!((level + 23.0).abs() < 0.2, "spectrum peak {level:.2} dB");
        assert!(!published.loudness_history.is_empty());
        assert!(!published.surface.rows.is_empty(), "3D history is recorded");
        assert_eq!(published.dropped_frames, 0);
    });

    shared.reset_loudness();
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.with_published(|published| published.loudness.integrated_lufs.is_finite()) {
        assert!(Instant::now() < deadline, "loudness resets");
        thread::sleep(Duration::from_millis(10));
    }
}
