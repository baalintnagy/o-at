//! End-to-end reception: a transmitter with its own clock, received through the link at
//! another rate. Simulated time runs faster than real time; the lease is renewed in
//! real time.

use std::f32::consts::TAU;
use std::process;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(all(debug_assertions, not(feature = "plugin")))]
use assert_no_alloc::AllocDisabler;
use assert_no_alloc::assert_no_alloc;
use audbus::{BusName, BusWriter, Directory, Lease};
use o_aud_rx::engine::Receiver;
use o_aud_rx::link::{LinkState, RxLink, receiver_in};
use o_aud_rx::settings::RxSettings;
use o_aud_rx::telemetry::{PlayState, RxTelemetry};

// With the `plugin` feature, NIH-plug's `assert_process_allocs` installs this allocator.
#[cfg(all(debug_assertions, not(feature = "plugin")))]
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const TONE_HZ: f32 = 1_000.0;
const SOURCE_RATE: u32 = 44_100;
const DEVICE_RATE: u32 = 48_000;
const SOURCE_BLOCK: usize = 512;
const DEVICE_BLOCK: usize = 480;

struct Transmitter {
    lease: Lease,
    writer: BusWriter,
    phase: f32,
    last_renew: Instant,
    /// Simulated time of the next block, in seconds.
    next_block_s: f64,
}

impl Transmitter {
    fn new(directory: &Arc<Directory>, name: &str) -> Self {
        let lease = directory
            .lease(&BusName::new(name).expect("valid name"))
            .expect("free name");
        let mut writer = BusWriter::create(&lease).expect("bus created");
        writer.start_stream(SOURCE_RATE, 2);
        lease.set_format(SOURCE_RATE, 2);
        Self {
            lease,
            writer,
            phase: 0.0,
            last_renew: Instant::now(),
            next_block_s: 0.0,
        }
    }

    fn write_block(&mut self) {
        let mut block = [0.0_f32; SOURCE_BLOCK];
        for sample in &mut block {
            *sample = 0.5 * self.phase.sin();
            self.phase = (self.phase + TAU * TONE_HZ / SOURCE_RATE as f32) % TAU;
        }
        self.writer.write(&[&block, &block], 1.0);
        if self.last_renew.elapsed() > Duration::from_millis(200) {
            assert!(self.lease.renew(), "lease kept");
            self.last_renew = Instant::now();
        }
    }
}

fn connect(namespace: &str, name: &str) -> (RxLink, Receiver, Arc<RxTelemetry>) {
    let telemetry = Arc::new(RxTelemetry::default());
    let (link, mut receiver) = receiver_in(
        namespace,
        Arc::new(RwLock::new(name.to_owned())),
        Arc::clone(&telemetry),
    );
    link.set_device_rate(DEVICE_RATE);
    receiver.set_device_rate(DEVICE_RATE);
    let deadline = Instant::now() + Duration::from_secs(5);
    while link.status().state != LinkState::Connected {
        assert!(
            Instant::now() < deadline,
            "link connects: {:?}",
            link.status()
        );
        thread::sleep(Duration::from_millis(10));
    }
    (link, receiver, telemetry)
}

/// Simulated output device clock.
#[derive(Default)]
struct Device {
    /// Simulated time of the next block, in seconds.
    next_block_s: f64,
}

/// Runs `seconds` more of simulated time with the source clock `drift` faster than
/// nominal, returning the left output channel.
fn run(
    transmitter: &mut Transmitter,
    (receiver, device): (&mut Receiver, &mut Device),
    seconds: f64,
    drift: f64,
    settings: &RxSettings,
) -> Vec<f32> {
    let source_period = SOURCE_BLOCK as f64 / (f64::from(SOURCE_RATE) * (1.0 + drift));
    let device_period = DEVICE_BLOCK as f64 / f64::from(DEVICE_RATE);
    let end = device.next_block_s + seconds;
    let mut left = vec![0.0_f32; DEVICE_BLOCK];
    let mut right = vec![0.0_f32; DEVICE_BLOCK];
    let mut output = Vec::with_capacity((seconds * f64::from(DEVICE_RATE)) as usize + DEVICE_BLOCK);
    while device.next_block_s < end {
        if transmitter.next_block_s <= device.next_block_s {
            transmitter.write_block();
            transmitter.next_block_s += source_period;
        } else {
            receiver.process(&mut [&mut left[..], &mut right[..]], settings);
            output.extend_from_slice(&left);
            device.next_block_s += device_period;
        }
    }
    output
}

/// Estimated frequency in Hz from upward zero crossings.
fn frequency(samples: &[f32]) -> f32 {
    let crossings = samples
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count();
    crossings as f32 * DEVICE_RATE as f32 / samples.len() as f32
}

#[test]
fn locks_to_a_drifting_transmitter_at_another_rate() {
    let namespace = format!("o-aud-rx-test-drift-{}", process::id());
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let mut transmitter = Transmitter::new(&directory, "Drift");
    let (_link, mut receiver, telemetry) = connect(&namespace, "drift");
    let mut device = Device::default();
    let settings = RxSettings {
        latency_ms: 30.0,
        ..RxSettings::default()
    };
    let drift = 300e-6;

    run(
        &mut transmitter,
        (&mut receiver, &mut device),
        30.0,
        drift,
        &settings,
    );
    assert_eq!(telemetry.state(), PlayState::Playing);
    let underruns = telemetry.underruns();
    let overruns = telemetry.overruns();

    let output = run(
        &mut transmitter,
        (&mut receiver, &mut device),
        10.0,
        drift,
        &settings,
    );
    assert_eq!(telemetry.underruns(), underruns, "no dropouts once locked");
    assert_eq!(telemetry.overruns(), overruns, "no skips once locked");
    let fill = telemetry.fill_ms.load();
    assert!((fill - 30.0).abs() < 8.0, "queue at {fill} ms");
    let ppm = telemetry.drift_ppm.load();
    let target = telemetry.target_ms.load();
    assert!(
        (ppm - 300.0).abs() < 60.0,
        "estimated drift {ppm} ppm, queue {fill} ms for {target} ms"
    );

    // The tone plays at its pitch, corrected for the source clock.
    let measured = frequency(&output);
    let expected = TONE_HZ * (1.0 + drift as f32);
    assert!((measured - expected).abs() < 1.0, "tone at {measured} Hz");
    let peak = output
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!((peak - 0.5).abs() < 0.02, "level preserved, peak {peak}");
}

#[test]
fn raises_the_target_for_bursty_transmitters() {
    let namespace = format!("o-aud-rx-test-burst-{}", process::id());
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let mut transmitter = Transmitter::new(&directory, "Burst");
    let (_link, mut receiver, telemetry) = connect(&namespace, "Burst");
    let mut device = Device::default();
    let settings = RxSettings {
        latency_ms: 5.0,
        ..RxSettings::default()
    };
    // The host delivers four blocks at once, as with deep driver buffering.
    let burst = 4;
    let source_period = SOURCE_BLOCK as f64 / f64::from(SOURCE_RATE);
    let device_period = DEVICE_BLOCK as f64 / f64::from(DEVICE_RATE);
    let mut left = vec![0.0_f32; DEVICE_BLOCK];
    let mut right = vec![0.0_f32; DEVICE_BLOCK];
    let mut run_for = |seconds: f64, receiver: &mut Receiver| {
        let end = device.next_block_s + seconds;
        while device.next_block_s < end {
            if transmitter.next_block_s <= device.next_block_s {
                (0..burst).for_each(|_| transmitter.write_block());
                transmitter.next_block_s += burst as f64 * source_period;
            } else {
                receiver.process(&mut [&mut left[..], &mut right[..]], &settings);
                device.next_block_s += device_period;
            }
        }
    };
    run_for(5.0, &mut receiver);
    let underruns = telemetry.underruns();
    run_for(10.0, &mut receiver);
    assert_eq!(telemetry.underruns(), underruns, "no dropouts once adapted");
    let target = telemetry.target_ms.load();
    let burst_ms = 1_000.0 * burst as f32 * SOURCE_BLOCK as f32 / SOURCE_RATE as f32;
    assert!(
        target > 0.5 * burst_ms && target < 2.0 * burst_ms,
        "target {target} ms for {burst_ms} ms bursts"
    );
}

#[test]
fn recovers_after_the_transmitter_pauses() {
    let namespace = format!("o-aud-rx-test-pause-{}", process::id());
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let mut transmitter = Transmitter::new(&directory, "Pause");
    let (_link, mut receiver, telemetry) = connect(&namespace, "Pause");
    let mut device = Device::default();
    let settings = RxSettings::default();
    run(
        &mut transmitter,
        (&mut receiver, &mut device),
        2.0,
        0.0,
        &settings,
    );
    assert_eq!(telemetry.state(), PlayState::Playing);

    let mut left = vec![1.0_f32; DEVICE_BLOCK];
    let mut right = vec![1.0_f32; DEVICE_BLOCK];
    for _ in 0..20 {
        receiver.process(&mut [&mut left[..], &mut right[..]], &settings);
    }
    assert_eq!(telemetry.state(), PlayState::Buffering);
    assert!(
        left.iter().all(|&sample| sample == 0.0),
        "silence while starved"
    );
    transmitter.next_block_s = device.next_block_s;

    run(
        &mut transmitter,
        (&mut receiver, &mut device),
        1.0,
        0.0,
        &settings,
    );
    assert_eq!(telemetry.state(), PlayState::Playing);
}

#[test]
fn processing_does_not_allocate() {
    let namespace = format!("o-aud-rx-test-alloc-{}", process::id());
    let directory = Directory::open_namespace(&namespace).expect("directory opens");
    let mut transmitter = Transmitter::new(&directory, "Alloc");
    let (_link, mut receiver, telemetry) = connect(&namespace, "Alloc");
    let settings = RxSettings {
        latency_ms: 5.0,
        mute: false,
        gain: 0.7,
    };
    let mut left = vec![0.0_f32; DEVICE_BLOCK];
    let mut right = vec![0.0_f32; DEVICE_BLOCK];
    assert_no_alloc(|| {
        for block in 0..400 {
            // Starve the receiver now and then to exercise buffering and draining.
            if block % 100 < 90 {
                transmitter.write_block();
            }
            receiver.process(&mut [&mut left[..], &mut right[..]], &settings);
        }
    });
    assert!(telemetry.underruns() > 0, "starvation was exercised");
}
