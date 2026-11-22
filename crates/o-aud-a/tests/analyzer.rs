//! The capture path and the analysis worker, end to end.
//!
//! Debug builds install NIH-plug's allocation-checking global allocator
//! (`assert_process_allocs`), which aborts on any allocation inside `assert_no_alloc`.
//! Run these tests without `--release` for the allocation check.

use std::f32::consts::TAU;
use std::process;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use audbus::{BusName, BusWriter, Directory};
use o_aud_a::analysis::Analyzer;
use o_aud_a::analysis::bus::SourceState;
use o_aud_a::capture::capture;
use o_aud_a::layout::Module;
use o_aud_a::settings::{AnalyzerSettings, Averaging, ChannelMode, Choice, Lanes, Signal, Window};
use o_aud_a::sources::SourceList;

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
        settings.sonogram.channels = Lanes::LeftRight;
    }
    let sources = Arc::new(RwLock::new(SourceList::default()));
    let analyzer = Analyzer::start(consumer, stream, Arc::clone(&settings), sources);
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
        while pushed_s - shared.with_published(|published| published.input.loudness.duration_s)
            > 1.0
        {
            assert!(Instant::now() < deadline, "the worker keeps up");
            shared.mark_viewed();
            thread::sleep(Duration::from_millis(2));
        }
    }
    // Wait for every block, so no audio is measured after the reset below.
    loop {
        shared.mark_viewed();
        let done = shared.with_published(|published| {
            published.input.loudness.duration_s >= 4.999
                && !published.input.spectrum.primary_db.is_empty()
        });
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "the worker analyzes the stream");
        thread::sleep(Duration::from_millis(10));
    }
    shared.with_published(|published| {
        assert_eq!(
            (published.input.sample_rate, published.input.channels),
            (RATE, 2)
        );
        let loudness = published.input.loudness;
        assert!(
            (loudness.integrated_lufs + 23.0).abs() < 0.1,
            "integrated {:.2} LUFS",
            loudness.integrated_lufs
        );
        assert!((published.input.stereo.correlation - 1.0).abs() < 1e-3);
        let spectrum = &published.input.spectrum;
        let bin = (1_000.0 / spectrum.bin_hz).round() as usize;
        let level = spectrum.primary_db[bin];
        assert!((level + 23.0).abs() < 0.2, "spectrum peak {level:.2} dB");
        assert!(!published.input.loudness_history.is_empty());
        assert!(
            !published.input.surface.rows.is_empty(),
            "3D history is recorded"
        );
        // Both lanes reach the editor in the sonogram feed.
        assert_eq!(
            published.input.sonogram.lanes, 2,
            "the sonogram reports two lanes"
        );
        assert_eq!(
            published.input.surface.lanes, 2,
            "the 3D history reports two lanes"
        );
        // Millisecond envelope buckets for long oscilloscope spans.
        let bucket = published.input.envelope.back().expect("envelope buckets");
        assert!((bucket[Signal::Left.index()][1] - amplitude).abs() < 0.01);
        assert!(published.input.envelope.len() > 1_000);
        assert_eq!(published.dropped_frames, 0);
    });

    shared.reset_loudness();
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.with_published(|published| published.input.loudness.integrated_lufs.is_finite()) {
        assert!(Instant::now() < deadline, "loudness resets");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn buses_are_measured_at_their_own_rate() {
    const BUS_RATE: u32 = 44_100;
    const BUS_BLOCK: usize = 441;
    let namespace = format!("o-aud-a-test-bus-{}", process::id());
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let name = BusName::new("Bass").expect("valid");
    let lease = directory.lease(&name).expect("free name");
    lease.set_format(BUS_RATE, 2);
    let mut writer = BusWriter::create(&lease).expect("bus region");
    writer.start_stream(BUS_RATE, 2);

    // The input is never activated: buses are measured anyway.
    let (_capture, consumer, stream) = capture();
    let settings = Arc::new(RwLock::new(AnalyzerSettings::default()));
    if let Ok(mut settings) = settings.write() {
        settings.spectrum.averaging = Averaging::Off;
        settings.spectrum.window = Window::FlatTop;
    }
    let mut list = SourceList::default();
    assert!(list.add("bass"), "names match case-insensitively");
    list.add("Missing");
    // Every view shows the bus; the image views and the oscilloscope only the bus.
    for module in Module::ALL {
        list.show_bus(module, 0, true);
    }
    for module in [
        Module::Sonogram,
        Module::Spectral,
        Module::Surface,
        Module::Oscilloscope,
    ] {
        list.show_input(module, false);
    }
    let sources = Arc::new(RwLock::new(list));
    let analyzer = Analyzer::start_in(&namespace, consumer, stream, settings, sources);
    let shared = Arc::clone(analyzer.shared());

    // Three seconds of a -20 dBFS 1 kHz sine, written in real time: a debug build of the
    // worker cannot run every analysis much faster.
    let amplitude = 0.1_f32;
    let blocks = 3 * BUS_RATE as usize / BUS_BLOCK;
    for block in 0..blocks {
        shared.mark_viewed();
        let samples: Vec<f32> = (0..BUS_BLOCK)
            .map(|n| {
                let t = (block * BUS_BLOCK + n) as f32 / BUS_RATE as f32;
                amplitude * (TAU * 1_000.0 * t).sin()
            })
            .collect();
        writer.write(&[&samples[..], &samples[..]], 1.0);
        if block % 20 == 0 {
            assert!(lease.renew(), "the lease stays ours");
        }
        thread::sleep(Duration::from_millis(10));
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        shared.mark_viewed();
        let done = shared.with_published(|published| {
            published.sources.first().is_some_and(|source| {
                let signal = &source.signal;
                signal.loudness.duration_s >= 2.9
                    && !signal.spectrum.primary_db.is_empty()
                    && !signal.surface.rows.is_empty()
            })
        });
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the worker measures the bus: {:?}",
            shared.with_published(|published| published.sources.first().map(|source| (
                source.signal.loudness.duration_s,
                source.gaps,
                source.state
            )))
        );
        thread::sleep(Duration::from_millis(10));
    }
    shared.with_published(|published| {
        assert_eq!(published.input.sample_rate, 0, "the input stays inactive");
        assert!(published.live_buses.iter().any(|bus| bus == "Bass"));
        let [bass, missing] = &published.sources[..] else {
            panic!("two sources, got {}", published.sources.len());
        };
        assert_eq!(bass.name, "bass");
        assert_eq!(bass.state, SourceState::Live);
        assert_eq!(bass.gaps, 0, "nothing was skipped");
        let signal = &bass.signal;
        assert_eq!((signal.sample_rate, signal.channels), (BUS_RATE, 2));
        let spectrum = &signal.spectrum;
        assert!((spectrum.bin_hz - BUS_RATE as f32 / 8_192.0).abs() < 1e-3);
        let bin = (1_000.0 / spectrum.bin_hz).round() as usize;
        let peak = spectrum.primary_db[bin - 2..=bin + 2]
            .iter()
            .fold(f32::MIN, |peak, level| peak.max(*level));
        assert!((peak + 20.0).abs() < 0.2, "spectrum peak {peak:.2} dB");
        let loudness = signal.loudness.integrated_lufs;
        assert!(
            (loudness + 20.0).abs() < 0.2,
            "integrated {loudness:.2} LUFS"
        );
        assert!(!signal.loudness_history.is_empty());
        assert!((signal.stereo.correlation - 1.0).abs() < 1e-3);
        assert_eq!(missing.state, SourceState::NoTransmitter);

        // The bus's sonograms reach 20 kHz at its rate and peak at the tone.
        for feed in [&signal.sonogram, &signal.spectral] {
            assert_ne!(feed.generation, 0, "a bus image");
            assert_eq!(feed.range_hz.1, 20_000.0);
            let column = feed.columns.back().expect("sonogram columns");
            let loudest = column
                .levels
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map_or(0, |(row, _)| row);
            let (low, high) = feed.range_hz;
            let position = (1_000.0_f32 / low).ln() / (high / low).ln();
            let expected = (position * column.levels.len() as f32) as usize;
            assert!(
                loudest.abs_diff(expected) <= 2,
                "row {loudest}, expected {expected}"
            );
            let wave = column.wave[0];
            assert!(
                wave.max > 0.09 && wave.max <= amplitude,
                "the bus's waveform"
            );
        }
        assert_eq!(signal.surface.range_hz.1, 20_000.0);
        // The oscilloscope's frames and envelope.
        assert!(signal.scope.len() >= 4_096);
        let peak = signal
            .scope
            .iter()
            .map(|frame| frame[0].abs())
            .fold(0.0, f32::max);
        assert!((peak - amplitude).abs() < 0.01, "scope peak {peak}");
        let bucket = signal.envelope.back().expect("envelope buckets");
        assert!((bucket[Signal::Left.index()][1] - amplitude).abs() < 0.01);
    });

    // Removing a source keeps the other one's measurements.
    shared.set_sources({
        let mut list = SourceList::default();
        list.add("bass");
        list
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.with_published(|published| published.sources.len()) != 1 {
        assert!(Instant::now() < deadline, "the list follows the settings");
        thread::sleep(Duration::from_millis(10));
    }
    shared.with_published(|published| {
        assert!(published.sources[0].signal.loudness.duration_s >= 2.9);
    });
}
