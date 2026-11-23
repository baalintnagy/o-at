//! Real-time safety: the audio path must not allocate.
//!
//! Debug builds install NIH-plug's allocation-checking global allocator
//! (`assert_process_allocs`), which aborts on any allocation inside `assert_no_alloc`.
//! Release builds do not check, so run this test without `--release`.

use std::f32::consts::TAU;

use assert_no_alloc::assert_no_alloc;
use o_nr::dsp::engine::{Engine, NoiseMode, Resolution, Settings};
use o_nr::dsp::profile::{ProfileData, ProfileStore};
use o_nr::telemetry::Telemetry;
use triple_buffer::triple_buffer;

const BLOCK: usize = 333;

fn signal(rate: f32, length: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..length)
        .map(|n| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
            0.05 * noise + 0.2 * (TAU * 440.0 * n as f32 / rate).sin()
        })
        .collect()
}

fn process(engine: &mut Engine, left: &mut [f32], right: &mut [f32], settings: &Settings) {
    for (left, right) in left.chunks_mut(BLOCK).zip(right.chunks_mut(BLOCK)) {
        engine.process(&mut [left, right], settings);
    }
}

#[test]
fn processing_learning_and_switching_do_not_allocate() {
    for rate in [44_100.0_f32, 96_000.0] {
        let store = ProfileStore::default();
        let (publisher, mut reader) = triple_buffer(&Telemetry::default());
        let mut engine = Engine::new(store.clone(), Some(publisher));
        engine.configure(rate, 2, Resolution::Medium);
        let length = rate as usize / 2;
        let mut left = signal(rate, length, 1);
        let mut right = signal(rate, length, 2);

        // A profile restored by the host, at another bin spacing.
        store.store(&ProfileData {
            bin_hz: 10.0,
            level_db: vec![-90.0; 2_000],
            spread_db: vec![6.0; 2_000],
        });

        let mut cases = Vec::new();
        for resolution in Resolution::ALL {
            for mode in [NoiseMode::Profile, NoiseMode::Adaptive] {
                for link in [true, false] {
                    cases.push(Settings {
                        resolution,
                        mode,
                        link,
                        ..Settings::default()
                    });
                }
            }
        }
        let learn = Settings {
            learn: true,
            ..Settings::default()
        };
        let listen = Settings {
            listen_removed: true,
            ..Settings::default()
        };

        assert_no_alloc(|| {
            for settings in &cases {
                process(&mut engine, &mut left, &mut right, settings);
            }
            // Learning stores a profile, which the engine then loads.
            process(&mut engine, &mut left, &mut right, &learn);
            process(&mut engine, &mut left, &mut right, &Settings::default());
            process(&mut engine, &mut left, &mut right, &listen);
            // Switching resolution while learning keeps the learned profile.
            process(&mut engine, &mut left, &mut right, &learn);
            process(
                &mut engine,
                &mut left,
                &mut right,
                &Settings {
                    resolution: Resolution::Max,
                    ..learn
                },
            );
            engine.reset();
            process(&mut engine, &mut left, &mut right, &Settings::default());
        });

        assert!(store.has_profile());
        let telemetry = reader.read();
        assert!(telemetry.sequence > 0, "telemetry was published");
        assert!(telemetry.has_profile);
    }
}
