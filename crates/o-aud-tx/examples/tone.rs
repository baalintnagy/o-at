//! Sends a test tone to a bus, clocked by a real output device that plays silence. Use it
//! to check O-AUD-RX against a hardware or driver clock without a plugin host.
//!
//! `cargo run -p o-aud-tx --example tone -- [BUS] [DEVICE] [FREQUENCY_HZ]`, where `DEVICE`
//! is part of an output device name. Press Enter to stop.

use std::env;
use std::f32::consts::TAU;
use std::io::stdin;
use std::sync::{Arc, RwLock};

use cpal::OutputCallbackInfo;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use o_aud_tx::link::transmitter;
use o_aud_tx::telemetry::TxTelemetry;

/// Largest device block handled at once, in frames.
const MAX_FRAMES: usize = 8_192;

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let bus = arguments
        .first()
        .cloned()
        .unwrap_or_else(|| String::from("Tone"));
    let device_filter = arguments.get(1).cloned().unwrap_or_default();
    let frequency: f32 = arguments
        .get(2)
        .and_then(|text| text.parse().ok())
        .unwrap_or(440.0);

    let host = cpal::default_host();
    let device = host
        .output_devices()
        .expect("output devices")
        .find(|device| {
            device
                .name()
                .is_ok_and(|name| name.contains(device_filter.as_str()))
        })
        .expect("a matching output device");
    let config = device.default_output_config().expect("device config");
    let rate = config.sample_rate().0;
    let device_channels = usize::from(config.channels());
    println!(
        "Sending {frequency} Hz to bus “{bus}”, clocked by {} at {rate} Hz",
        device.name().unwrap_or_default()
    );

    let telemetry = Arc::new(TxTelemetry::default());
    let (link, mut port) = transmitter(Arc::new(RwLock::new(bus)), telemetry);
    link.set_format(rate, 2);
    port.set_format(rate, 2);

    let mut phase = 0.0_f32;
    let mut tone = vec![0.0_f32; MAX_FRAMES];
    let stream = device
        .build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &OutputCallbackInfo| {
                data.fill(0.0);
                for block in data.chunks(MAX_FRAMES * device_channels) {
                    let frames = block.len() / device_channels;
                    for sample in &mut tone[..frames] {
                        *sample = 0.25 * phase.sin();
                        phase = (phase + TAU * frequency / rate as f32) % TAU;
                    }
                    port.process(&[&tone[..frames], &tone[..frames]], 1.0);
                }
            },
            |error| eprintln!("Stream error: {error}"),
            None,
        )
        .expect("output stream");
    stream.play().expect("stream starts");

    let mut line = String::new();
    let _ = stdin().read_line(&mut line);
    println!("Final state: {:?}", link.status().state);
}
