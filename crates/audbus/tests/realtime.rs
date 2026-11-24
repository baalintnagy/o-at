//! Writing and reading a bus must not allocate.

use std::process;

#[cfg(debug_assertions)]
use assert_no_alloc::AllocDisabler;
use audbus::handoff::handoff;
use audbus::{BusName, BusReader, BusWriter, Directory};
use rt_check::realtime;

// The allocation guard only exists in debug builds.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[test]
fn audio_thread_operations_do_not_allocate() {
    let namespace = format!("audbus-test-realtime-{}", process::id());
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let name = BusName::new("Realtime").expect("valid name");
    let lease = directory.lease(&name).expect("free name");
    let mut writer = BusWriter::create(&lease).expect("bus created");
    let reader = BusReader::open(&directory, &name).expect("bus exists");
    let (mut sender, mut receiver) = handoff::<BusReader>();
    assert!(sender.send(Some(reader)).is_ok(), "room for the reader");
    let block = vec![0.5_f32; 512];
    let mut output = vec![0.0_f32; 1024];

    realtime(|| {
        writer.start_stream(48_000, 2);
        receiver.poll();
        let reader = receiver.current().expect("reader handed over");
        let format = reader.format().expect("stream started");
        for _ in 0..8 {
            let position = reader.write_position();
            writer.write(&[&block, &block], 0.8);
            assert!(reader.read(position, 512, 2, format.epoch, &mut output));
        }
    });
}
