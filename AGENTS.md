# Source style

These conventions follow the sibling PermaFormula repositories, adapted to Rust.
[docs/source-style.md](docs/source-style.md) lists what the tools enforce and what
remains a review requirement.

- Use four spaces for indentation and a target source width of 100 characters.
  Keep indivisible URLs, literals and syntax that requires different indentation intact.
- Use two spaces for indentation in `.yml` and `.yaml` files.
- Format with `cargo fmt` using the repository's `rustfmt.toml`. Do not hand-format
  against it.
- Avoid fully qualified paths in source; import types, traits, functions and constants
  with `use`. Do not use glob imports, including `use super::*` in tests; name what a
  test uses.
- Remove unused imports, variables and dependencies.
- Eliminate compiler and Clippy warnings. Fix straightforward causes rather than hiding
  warnings. If resolving a warning is not easy, keep any `#[allow]` as narrow as
  possible, explain it in a comment, and report the remaining warning with practical
  solution ideas and tradeoffs. Do not disable lints crate-wide.
- Name unused bindings `_` when the value is not needed; keep descriptive names when
  the value is referenced.
- Prefer iterator adapters over loops when no extra mutable state is needed. Keep
  stateful, index-coupled, real-time and control-flow loops.
- Prefer `format!("...{value}...")` inline arguments to string concatenation.
- Write a doc comment for each public item whose purpose is not obvious from its name,
  and state units (dB, ms, Hz, samples) in names or docs.
- Apply these conventions to maintained sources, not vendored code, generated output
  or `Cargo.lock`.
- Maintain source-style rules and tool configuration together. When adding or changing
  a rule, update this guidance, `rustfmt.toml`, `clippy.toml` or the CI workflow, and
  the documented enforcement coverage. Prefer existing tool configuration; do not write
  custom formatters. If a rule cannot be enforced by the configured tools, document the
  limitation and keep it as a review requirement.

# Real-time safety

Code reachable from `Plugin::process` must not allocate, lock, block or perform I/O.
Allocate in constructors, `initialize` or `StreamProcessor::configure`. Debug builds
enable NIH-plug's `assert_process_allocs`, which aborts on allocations in `process`.
Run `cargo test --test realtime` (a debug build) after changing the audio path, and
extend that test when adding processing modes.

DeepFilterNet's inference allocates, so it runs on its own worker thread, fed through
lock-free `rtrb` ring buffers. The audio thread never waits for it in real-time mode; a
late result is replaced by the time-aligned RNNoise output. Only offline rendering
(`ProcessMode::Buffered` or `Offline`) waits for results.

The editor exchanges data with the audio thread only through the telemetry triple
buffer and parameters. Keep `Telemetry` fixed-size.

# Audio bus tools

O-AUD-TX, O-AUD-RX and the O-AUD-RX application live in `crates/`; see
[docs/audio-bus.md](docs/audio-bus.md). The same real-time rules apply to
`TxPort::process`, `Receiver::process` and the `BusWriter` and `BusReader` methods they
call. Their link threads open shared memory, lease names and allocate; resources reach
the audio thread only through `audbus::handoff`, which also returns them for dropping.
Run `cargo test -p audbus -p o-aud-tx -p o-aud-rx` (a debug build, with allocation checks)
after changing these paths, and extend the allocation tests for new processing modes.

The shared memory layouts in `audbus::layout` are a protocol between separately built
binaries. Never change them without raising `PROTOCOL_VERSION`.

All editors, O-NEC's and O-AUD-A's included, take their palette and controls from
`aud-ui`; O-NEC keeps only its signal colors in `src/editor/colors.rs`. Controls use the `Control` and
`Switch` abstractions, bound to parameters with `aud_ui::param`, so the O-AUD-RX editor
body serves the plugin and the application. Keep plugin-specific code out of
`o-aud-rx`'s core modules; the `plugin` and `view` features gate NIH-plug and egui.

# Audio analyzer

O-AUD-A lives in `crates/o-aud-a`; see [docs/o-aud-a.md](docs/o-aud-a.md). Its
`Plugin::process` only calls `Capture::push`, which copies the input into an `rtrb`
ring and must stay real-time safe. All measurements run on the analysis worker, which
may allocate; the editor reads its results through a mutex that the audio thread never
takes. Run `cargo test -p o-aud-a` (a debug build, with allocation checks) after
changing the capture path. Its settings are persisted by serde variant and field names
under `#[persist = "settings"]`: never rename them. O-AUD-A opens its own editor
window (`crates/o-aud-a/src/window.rs`) so it can resize itself; keep it in step with
`nih_plug_egui` when updating NIH-plug.

# Latency contract

The latency reported to the host must equal the measured impulse delay at every
supported rate. Every processing stage keeps a constant delay, also when it is
disabled. Only the noise model choice changes the delay; the plugin reports the new
latency when it changes. When changing a stage's delay, update its delay constant and
keep `reported_latency_matches_impulse_delay` passing for both noise models.
O-AUD-TX and O-AUD-A add no delay and O-AUD-RX reports none: its buffering belongs to
an external stream that a host cannot compensate.

# Parameter compatibility

Parameter IDs (`#[id = "..."]`) and `#[persist]` keys are stored in host sessions
and presets, for O-NEC, O-AUD-TX, O-AUD-RX and O-AUD-A alike. Never rename or reuse
them. Add new parameters with new IDs. The VST3 class IDs and CLAP IDs are equally permanent.

# Dependencies

Prefer well-tested audio and DSP crates over reimplementing them, but only use what
demonstrably works in this plugin. Keep NIH-plug pinned to a specific revision and
update it deliberately, rebuilding and retesting the bundles. Its `nih_plug` crate is
vendored in `vendor/nih_plug` with fixes to the standalone audio backend and replaces
the pinned revision through `[patch]`; when updating NIH-plug, follow
[vendor/README.md](vendor/README.md). Record license changes in NOTICE.md.

# Licensing

The repository's original code and documentation use the standard MIT License.
Built VST3 binaries include GPLv3 bindings; keep NOTICE.md accurate when dependencies
change. Preserve third-party licenses and notices.

# Releases and assets

Releases are published by `.github/workflows/release.yml` from tags `vX.Y.Z`, which must
match the root `Cargo.toml` version; it reuses `ci.yml` as its build. Keep
`packaging/INSTALL.txt` in step with what the package contains.

Executable icons are generated by `tools/icons/generate.py` into `assets/icons` and
embedded by the `build.rs` scripts (resource ID 1, which `aud_ui::window_icon` also
loads). Change icons in the generator, not by editing the images.
