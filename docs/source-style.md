# Rust source-style tooling

Run these commands from the repository root:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features o-nec/standalone,o-aud-a/standalone,o-nr/standalone -- -D warnings
cargo test -p o-nec --release
cargo test -p o-nec --test realtime
cargo test --workspace --release
cargo test -p audbus -p o-aud-tx -p o-aud-rx
cargo test -p o-aud-a
cargo test -p o-nr
```

On Linux, the real-time tests also run under RealtimeSanitizer:

```bash
RTSAN_ENABLE=1 cargo test -p o-nec --test realtime --features rt-check/rtsan
RTSAN_ENABLE=1 cargo test -p audbus -p o-aud-tx -p o-aud-rx -p o-aud-a -p o-nr --features rt-check/rtsan
```

The CI workflow runs the check commands and fails on any difference or warning.

## Enforced by tools

`rustfmt` with [rustfmt.toml](../rustfmt.toml) enforces:

- Four-space indentation and a 100-character maximum width.
- Unix line endings.
- Import ordering and grouping.
- Wrapping of long expressions, argument lists and boolean conditions. Long call
  chains are broken with one call per line and a leading dot; chains up to rustfmt's
  default `chain_width` stay on one line.

`cargo clippy -- -D warnings` and the compiler enforce:

- No unused imports, variables, functions or `mut` bindings.
- Clippy's default lint set, including redundant clones, needless borrows and
  initialization patterns.

`.editorconfig` configures editors for indentation, final newlines and line endings,
including two-space YAML.

## Review requirements

These conventions are not enforced by the configured tools:

- Importing items instead of writing fully qualified paths in expressions.
- No glob imports, including in test modules.
- Iterator adapters versus loops, following the guidance in [AGENTS.md](../AGENTS.md).
- Real-time safety of code reachable from `Plugin::process`, `TxPort::process`,
  `InterleavedSender::send`, `Receiver::process`, `Capture::push` and O-NR's
  `Engine::process` outside what the tests exercise. Debug builds detect allocations
  at run time inside `rt_check::realtime`; the RealtimeSanitizer workflow
  (`.github/workflows/rtsan.yml`) also detects locks, blocking and system calls there,
  on Linux only.
- Units in names or doc comments.
