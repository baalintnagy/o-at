# O-AUD-TX, O-AUD-RX and the audio bus

O-AUD-TX sends a track's audio to a named bus. O-AUD-RX plays a bus: as a plugin in any host,
or as the standalone O-AUD-RX application on any output device. Buses need no server or
driver; they live in shared memory.

## Crates

| Crate | Role |
| --- | --- |
| `audbus` | Transport: bus directory, name leases, the shared ring, real-time handoff. |
| `aud-ui` | Editor theme and controls shared by O-NEC, both plugins and the application. |
| `o-aud-tx` | The O-AUD-TX plugin: link thread, audio port and editor. |
| `o-aud-rx` | The receiver core (link, connection, engine, clock sync), the editor body and, with the default `plugin` feature, the O-AUD-RX plugin. |
| `o-aud-rx-app` | The standalone O-AUD-RX application: the `o-aud-rx` core and editor body with cpal output. |

Editors depend on the `Control` and `Switch` abstractions in `aud-ui`, not on NIH-plug
parameters, so the plugin and the application share the O-AUD-RX editor body. The plugin
binds them to parameters with automation gestures; the application binds them to
atomic settings.

## Threads

Each plugin instance and the application run a control thread, the *link*, next to the
audio thread. The link opens shared memory, takes or follows bus names and allocates.
The audio thread only reads and writes the ring.

Resources move between them through `audbus::handoff`: the link sends a prepared bus
writer (O-AUD-TX) or connection (O-AUD-RX) through a lock-free queue, the audio thread swaps
it in and returns the previous one, and the link drops it. The audio thread never
allocates, frees, locks or maps memory. The editors read levels and synchronization
state from atomics.

## Directory and names

The directory is a shared region with 64 slots. A transmitter takes a *lease* on a
slot for its bus name and renews it every 200 ms; leases that are not renewed for
1.5 s expire, so the buses of crashed hosts disappear. Names are 1–32 bytes of UTF-8
without control characters and match case-insensitively.

A transmitter whose name is taken reports it with a free variant (`Bus 1` → `Bus 2`)
that the editor offers as a button. An empty name becomes the first free `Bus n` once
the host activates the plugin. When a name changes, the old lease is kept until the
audio thread has stopped writing, so two writers never share a bus.

## The ring

Each bus is its own shared region: a header and 8 channels × 65 536 frames of planar
`f32` samples (1.37 s at 48 kHz). The format is fixed per protocol version, and a zero-
filled region is valid, so no initialization race exists.

- One writer appends blocks and publishes the total frame count with release ordering.
  Samples are `AtomicU32` bits with relaxed ordering, which compile to plain loads and
  stores.
- Readers keep their own position and never write the ring, so any number of receivers
  can play one bus. After copying, a reader rechecks the writer's position; data that
  may have been overwritten is rejected.
- An *epoch* sequence lock covers the stream format (rate, channels). Each stream start
  or format change moves it, and receivers resynchronize.
- Receivers register in reader slots with heartbeats, so O-AUD-TX shows its listeners.

Windows uses pagefile-backed named mappings in the session namespace; they vanish with
the last handle. Unix uses files in `/dev/shm` (or the temporary directory). Region
names contain the protocol version, so incompatible builds do not meet.

## Clock synchronization

The transmitter's and the receiver's sample clocks run from different crystals. O-AUD-RX
resamples with rubato's asynchronous sinc resampler (128 taps, Blackman-Harris²) from
the bus rate to the output rate, and steers the ratio to hold the queued audio at its
target:

- A PI controller on the queue length (filtered with a 1 s time constant to remove the
  sawtooth of block-wise writing and reading) adjusts the ratio by at most ±0.5 %
  (8.6 cents). The loop is critically damped; its integral converges to the clock
  offset, which the editor shows in ppm.
- Playback starts once the queue reaches the target and then reads exactly the target
  behind the writer. Dropouts fade out over one 64-frame chunk; restarts fade in over
  256 frames. Queues far above the target are skipped back to it.
- The target is the requested latency, raised automatically when the blocks need more:
  half a transmitter burst plus half a device block, or the deepest dip of the queue
  below its smoothed length, plus one resampler input, times 1.5. Bursts are remembered
  for 4 s; dips decay with a 17 s half-life and are ignored during the first second of
  playback.

Equal rates use the same path, so drift is corrected without a rate change. A clock
more than 0.5 % off (for example a sleep-paced dummy audio backend) cannot be followed
and causes periodic dropouts.

## Channel mapping

A transmitter streams its input channels. A receiver maps the stream to its outputs:
mono fills every output, a stereo stream folds to a mono output, and outputs beyond the
stream stay silent.

## Testing

```bash
cargo test -p audbus -p o-aud-tx -p o-aud-rx
```

The tests cover leases and name conflicts, separately mapped directories, ring round
trips, overwritten-data detection, format changes, listener counting, automatic names
and free variants, clock locking to a +300 ppm 44.1 kHz source at 48 kHz (queue, drift
estimate, pitch and level), recovery after a stalled transmitter, automatic latency for
bursty transmitters, and allocation-free audio paths of the bus, O-AUD-TX and O-AUD-RX.
Run them in debug builds for the allocation checks.

To test against a real device clock without a host, send a tone clocked by any output
device (it plays silence), for example a virtual cable:

```bash
cargo run -p o-aud-tx --example tone -- "Tone" "CABLE Input" 440
```
