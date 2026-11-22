//! Transmitter link and audio port against a private bus directory.

use std::process;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(all(debug_assertions, not(feature = "plugin")))]
use assert_no_alloc::AllocDisabler;
use assert_no_alloc::assert_no_alloc;
use audbus::{BusName, BusReader, Directory};
use o_aud_tx::interleaved::{InterleavedSender, MAX_BLOCK_FRAMES};
use o_aud_tx::link::{TxLink, TxState, transmitter_in};
use o_aud_tx::port::TxPort;
use o_aud_tx::telemetry::TxTelemetry;

// With the `plugin` feature, NIH-plug's `assert_process_allocs` installs this allocator.
#[cfg(all(debug_assertions, not(feature = "plugin")))]
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

fn namespace(test: &str) -> String {
    format!("o-aud-tx-test-{test}-{}", process::id())
}

fn start(namespace: &str, name: &str) -> (TxLink, TxPort, Arc<TxTelemetry>) {
    let telemetry = Arc::new(TxTelemetry::default());
    let (link, mut port) = transmitter_in(
        namespace,
        Arc::new(RwLock::new(name.to_owned())),
        Arc::clone(&telemetry),
    );
    link.set_format(48_000, 2);
    port.set_format(48_000, 2);
    (link, port, telemetry)
}

/// Waits until the link reports a state matching `predicate`, processing silence so the
/// port takes over writers.
fn wait_for(link: &TxLink, port: &mut TxPort, predicate: impl Fn(&TxState) -> bool) -> TxState {
    let deadline = Instant::now() + Duration::from_secs(5);
    let block = [0.0_f32; 64];
    loop {
        port.process(&[&block, &block], 1.0);
        let state = link.status().state;
        if predicate(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "state stuck at {state:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn names_are_assigned_and_conflicts_suggest_a_free_one() {
    let namespace = namespace("names");
    let (first, mut first_port, _) = start(&namespace, "");
    wait_for(&first, &mut first_port, |state| *state == TxState::Live);
    assert_eq!(first.name(), "Bus 1", "empty names get a free default");

    let (second, mut second_port, _) = start(&namespace, "bus 1");
    let state = wait_for(&second, &mut second_port, |state| {
        matches!(state, TxState::Taken { .. })
    });
    assert_eq!(
        state,
        TxState::Taken {
            suggestion: String::from("bus 2")
        }
    );

    drop(first);
    wait_for(&second, &mut second_port, |state| *state == TxState::Live);

    second.set_name("Renamed");
    wait_for(&second, &mut second_port, |state| *state == TxState::Live);
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let deadline = Instant::now() + Duration::from_secs(2);
    while directory
        .find(&BusName::new("Renamed").expect("valid"))
        .is_none()
    {
        assert!(Instant::now() < deadline, "renamed bus listed");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        directory
            .find(&BusName::new("bus 1").expect("valid"))
            .is_none()
    );
}

#[test]
fn invalid_names_are_reported() {
    let namespace = namespace("invalid");
    let (link, mut port, _) = start(&namespace, &"x".repeat(40));
    let state = wait_for(&link, &mut port, |state| {
        matches!(state, TxState::InvalidName(_))
    });
    assert!(matches!(state, TxState::InvalidName(_)));
}

#[test]
fn audio_reaches_the_bus_with_gain_and_meters() {
    let namespace = namespace("audio");
    let (link, mut port, telemetry) = start(&namespace, "Audio");
    wait_for(&link, &mut port, |state| *state == TxState::Live);
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let reader =
        BusReader::open(&directory, &BusName::new("Audio").expect("valid")).expect("bus exists");

    // Let the gain ramp settle at 0.5.
    let block = [0.8_f32; 4_800];
    port.process(&[&block, &block], 0.5);
    let position = reader.write_position();
    port.process(&[&block, &block], 0.5);
    let format = reader.format().expect("stream started");
    assert_eq!((format.sample_rate, format.channels), (48_000, 2));
    let mut output = vec![0.0; 2 * 4_800];
    assert!(reader.read(position, 4_800, 2, format.epoch, &mut output));
    assert!(output.iter().all(|&sample| (sample - 0.4).abs() < 1e-6));
    assert!((telemetry.peaks[0].take() - 0.4).abs() < 1e-3);
}

#[test]
fn sending_does_not_allocate() {
    let namespace = namespace("alloc");
    let (link, mut port, _) = start(&namespace, "Alloc");
    wait_for(&link, &mut port, |state| *state == TxState::Live);
    // A rename hands a new writer over while the port is processing.
    link.set_name("Alloc 2");
    let block = vec![0.25_f32; 512];
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        assert_no_alloc(|| {
            for _ in 0..16 {
                port.process(&[&block, &block], 0.9);
            }
        });
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(link.status().state, TxState::Live);
    assert_eq!(link.name(), "Alloc 2");
}

/// Waits until the link is live, sending silence through `sender`.
fn wait_live(link: &TxLink, sender: &mut InterleavedSender) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let block = [0_i16; 128];
    while link.status().state != TxState::Live {
        sender.send(&block, 1.0, f32::from);
        assert!(
            Instant::now() < deadline,
            "state stuck at {:?}",
            link.status().state
        );
        thread::sleep(Duration::from_millis(5));
    }
    // Let the port take over the writer.
    sender.send(&block, 1.0, f32::from);
}

#[test]
fn interleaved_buffers_reach_the_bus_planar() {
    let namespace = namespace("interleaved");
    let (link, port, _) = start(&namespace, "Interleaved");
    // A four-channel device: all four channels reach the bus.
    let mut sender = InterleavedSender::new(port, 48_000, 4);
    link.set_format(48_000, sender.channels());
    wait_live(&link, &mut sender);
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let reader = BusReader::open(&directory, &BusName::new("Interleaved").expect("valid"))
        .expect("bus exists");

    // Longer than one block, so the buffer is split.
    let frames = MAX_BLOCK_FRAMES + 100;
    let data: Vec<f32> = (0..frames).flat_map(|_| [0.1_f32, 0.2, 0.3, 0.4]).collect();
    // Settle the gain ramp at unity first.
    sender.send(&data, 1.0, |sample| sample);
    let position = reader.write_position();
    sender.send(&data, 1.0, |sample| sample);
    let format = reader.format().expect("stream started");
    assert_eq!((format.sample_rate, format.channels), (48_000, 4));
    let mut output = vec![0.0; 4 * frames];
    assert!(reader.read(position, frames, 4, format.epoch, &mut output));
    for (channel, expected) in output.chunks(frames).zip([0.1, 0.2, 0.3, 0.4]) {
        assert!(
            channel
                .iter()
                .all(|&sample| (sample - expected).abs() < 1e-6)
        );
    }
}

#[test]
fn replaced_ports_reclaim_the_bus_without_allocating() {
    let namespace = namespace("replace");
    let (link, port, _) = start(&namespace, "Replace");
    let mut sender = InterleavedSender::new(port, 48_000, 2);
    wait_live(&link, &mut sender);

    // A new device stream: drop the old port, then send through a replacement.
    drop(sender);
    let mut sender = InterleavedSender::new(link.replace_port(), 44_100, 1);
    link.set_format(44_100, sender.channels());
    let block = vec![1_000_i16; 2 * 441];
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        assert_no_alloc(|| {
            for _ in 0..16 {
                sender.send(&block, 0.5, |sample| f32::from(sample) / 32_768.0);
            }
        });
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(link.status().state, TxState::Live);
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let reader =
        BusReader::open(&directory, &BusName::new("Replace").expect("valid")).expect("bus exists");
    let format = reader.format().expect("stream started");
    assert_eq!((format.sample_rate, format.channels), (44_100, 1));
}
