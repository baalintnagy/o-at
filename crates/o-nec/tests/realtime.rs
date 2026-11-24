//! Real-time safety: the audio path must not allocate.
//!
//! Debug builds install NIH-plug's allocation-checking global allocator
//! (`assert_process_allocs`), which aborts on any allocation inside
//! `realtime`. Release builds do not check, so run this test without
//! `--release`.

use std::f32::consts::TAU;

use o_nec::dsp::deep_filter::Pacing;
use o_nec::dsp::denoise::NoiseModel;
use o_nec::dsp::engine::{Engine, EngineSettings};
use o_nec::dsp::gate::GateTrigger;
use o_nec::dsp::stream::StreamProcessor;
use o_nec::telemetry::Telemetry;
use rt_check::realtime;
use triple_buffer::triple_buffer;

#[test]
fn processing_and_reset_do_not_allocate() {
    let cases = [
        (NoiseModel::RnNoise, 48_000_u32),
        (NoiseModel::RnNoise, 44_100),
        (NoiseModel::DeepFilterNet, 48_000),
        (NoiseModel::DeepFilterNet, 44_100),
    ];
    for (noise_model, rate) in cases {
        let (publisher, mut reader) = triple_buffer(&Telemetry::default());
        let mut engine = Engine::new(Some(publisher));
        let mut settings = EngineSettings {
            analyze: true,
            echo_tail_ms: 500,
            noise_model,
            ..EngineSettings::default()
        };
        settings.gate.trigger = GateTrigger::Voice;
        settings.leveler.enabled = true;
        engine.set_settings(settings);
        let mut processor = StreamProcessor::new(engine);
        processor
            .configure(rate, 333, Pacing::Realtime)
            .expect("supported rate");

        let length = rate as usize * 2;
        let reference: Vec<f32> = (0..length)
            .map(|n| 0.3 * (TAU * 310.0 * n as f32 / rate as f32).sin())
            .collect();
        let mic: Vec<f32> = reference
            .iter()
            .enumerate()
            .map(|(n, far)| 0.5 * far + 0.1 * (TAU * 1_200.0 * n as f32 / rate as f32).sin())
            .collect();
        let mut output = vec![0.0; length];

        // The first frame on a thread fills nnnoiseless's thread-local FFT plan cache, the
        // one allocation the audio path permits (see `Denoiser::warm_up_thread`).
        // RealtimeSanitizer reports it all the same, so it happens before the checked part.
        processor.process(&mic[..1000], &reference[..1000], &mut output[..1000]);
        processor.reset();

        realtime(|| {
            for ((mic, reference), output) in mic
                .chunks(333)
                .zip(reference.chunks(333))
                .zip(output.chunks_mut(333))
            {
                processor.process(mic, reference, output);
            }
            processor.reset();
            processor.process(&mic[..1000], &reference[..1000], &mut output[..1000]);
        });
        assert!(reader.read().sequence > 0, "telemetry was published");
    }
}
