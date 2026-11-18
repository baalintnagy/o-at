//! Bus behavior across independent mappings, as between two processes.

use std::process;
use std::sync::Arc;

use audbus::{
    BusError, BusName, BusReader, BusWriter, Directory, MAX_WRITE_FRAMES, SAFE_WINDOW_FRAMES,
};

/// A directory in a namespace private to one test, so tests and running applications
/// do not see each other's buses.
fn private_directory(test: &str) -> Arc<Directory> {
    let namespace = format!("audbus-test-{test}-{}", process::id());
    Directory::open_namespace(&namespace).expect("directory opens")
}

fn name(text: &str) -> BusName {
    BusName::new(text).expect("valid name")
}

#[test]
fn leases_are_exclusive_and_released_on_drop() {
    let directory = private_directory("lease");
    let lease = directory.lease(&name("Drums")).expect("free name");
    assert_eq!(
        directory.lease(&name("drums")).err(),
        Some(BusError::NameTaken)
    );
    lease.set_format(48_000, 2);
    let buses = directory.buses();
    assert_eq!(buses.len(), 1);
    assert_eq!(buses[0].name, "Drums");
    assert_eq!((buses[0].sample_rate, buses[0].channels), (48_000, 2));
    assert_eq!(directory.suggest_name("Drums").as_str(), "Drums 1");
    assert_eq!(directory.free_variant(&name("Drums")).as_str(), "Drums 2");
    let numbered = directory.lease(&name("Take 7")).expect("free name");
    assert_eq!(directory.free_variant(&name("Take 7")).as_str(), "Take 8");
    drop(numbered);

    drop(lease);
    assert!(directory.buses().is_empty());
    assert!(directory.lease(&name("drums")).is_ok());
}

#[test]
fn separately_opened_directories_share_state() {
    let first = private_directory("shared");
    let namespace = format!("audbus-test-shared-{}", process::id());
    let second = Directory::open_namespace(&namespace).expect("directory opens");
    let _lease = first.lease(&name("Vocals")).expect("free name");
    assert!(second.find(&name("VOCALS")).is_some());
    assert_eq!(
        second.lease(&name("Vocals")).err(),
        Some(BusError::NameTaken)
    );
}

#[test]
fn reader_needs_an_existing_bus() {
    let directory = private_directory("missing");
    assert_eq!(
        BusReader::open(&directory, &name("Nobody")).err(),
        Some(BusError::NotFound)
    );
}

#[test]
fn audio_round_trips_with_format_and_listeners() {
    let directory = private_directory("audio");
    let lease = directory.lease(&name("Mix")).expect("free name");
    let mut writer = BusWriter::create(&lease).expect("bus created");
    let reader = BusReader::open(&directory, &name("mix")).expect("bus exists");
    assert!(reader.format().is_none(), "no stream before start");

    writer.start_stream(44_100, 2);
    let format = reader.format().expect("stream started");
    assert_eq!((format.sample_rate, format.channels), (44_100, 2));

    let start = reader.write_position();
    let left: Vec<f32> = (0..256).map(|n| n as f32).collect();
    let right: Vec<f32> = (0..256).map(|n| -(n as f32)).collect();
    writer.write(&[&left, &right], 0.5);

    let mut output = vec![0.0; 2 * 256];
    assert!(reader.read(start, 256, 2, format.epoch, &mut output));
    assert_eq!(output[10], 5.0);
    assert_eq!(output[256 + 10], -5.0);
    assert!(
        !reader.read(start + 1, 256, 2, format.epoch, &mut output),
        "frames beyond the write position are unavailable"
    );

    let monitor = writer.monitor();
    assert_eq!(monitor.listeners(), 0);
    let mut presence = reader.presence();
    presence.renew();
    assert_eq!(monitor.listeners(), 1);
    drop(presence);
    assert_eq!(monitor.listeners(), 0);

    writer.start_stream(48_000, 1);
    assert!(
        !reader.read(start, 256, 2, format.epoch, &mut output),
        "a restarted stream invalidates the old epoch"
    );
}

#[test]
fn mono_input_fills_every_stream_channel() {
    let directory = private_directory("mono");
    let lease = directory.lease(&name("Mono")).expect("free name");
    let mut writer = BusWriter::create(&lease).expect("bus created");
    let reader = BusReader::open(&directory, &name("Mono")).expect("bus exists");
    writer.start_stream(48_000, 2);
    let start = reader.write_position();
    writer.write(&[[0.25_f32; 64]], 1.0);
    let epoch = reader.format().expect("stream").epoch;
    let mut output = [0.0; 128];
    assert!(reader.read(start, 64, 2, epoch, &mut output));
    assert!(output.iter().all(|&sample| sample == 0.25));
}

#[test]
fn readers_that_fall_too_far_behind_are_told_so() {
    let directory = private_directory("lag");
    let lease = directory.lease(&name("Lag")).expect("free name");
    let mut writer = BusWriter::create(&lease).expect("bus created");
    let reader = BusReader::open(&directory, &name("Lag")).expect("bus exists");
    writer.start_stream(48_000, 1);
    let start = reader.write_position();
    let block = vec![0.1_f32; MAX_WRITE_FRAMES];
    let blocks = SAFE_WINDOW_FRAMES as usize / MAX_WRITE_FRAMES + 1;
    (0..blocks).for_each(|_| writer.write(&[&block], 1.0));
    let epoch = reader.format().expect("stream").epoch;
    let mut output = vec![0.0; 64];
    assert!(!reader.read(start, 64, 1, epoch, &mut output));
    let recent = reader.write_position() - 64;
    assert!(reader.read(recent, 64, 1, epoch, &mut output));
}
