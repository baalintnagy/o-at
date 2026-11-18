# Rust source-style tooling

Run these commands from the repository root:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features o-nec/standalone,o-aud-tx/standalone,o-aud-a/standalone -- -D warnings
cargo test --release
cargo test --test realtime
cargo test --workspace --release
cargo test -p audbus -p o-aud-tx -p o-aud-rx
cargo test -p o-aud-a
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
  `Receiver::process` and `Capture::push` beyond allocations, which debug builds detect at run time.
- Units in names or doc comments.
