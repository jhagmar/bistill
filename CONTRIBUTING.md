# Contributing

Thank you for contributing to bistill.

## Bootstrap

Rust 1.85.0, with `rustfmt`, `clippy`, and `llvm-tools-preview`. The repository `rust-toolchain.toml` pins 1.98.0 for local builds. CI sets `RUSTUP_TOOLCHAIN=1.85.0`.

`cargo-deny` 0.20.2 and `reuse` 6.2.0 are required for the full local check. With Docker, follow [`ci/README.md`](ci/README.md).

Once per clone, enable the pre-commit hook so `cargo fmt` runs with the 1.85.0 toolchain before each commit:

```bash
git config core.hooksPath .githooks
```

CI still runs `cargo fmt --all -- --check`. The hook is a local convenience.

```bash
export RUSTUP_TOOLCHAIN=1.85.0
cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
./scripts/layering.sh
cargo deny check
cargo test --workspace --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps
./scripts/coverage.sh
reuse lint
```

The `test` job runs the cargo and script commands. The REUSE workflow runs `reuse lint`. `./scripts/coverage.sh` fails when a measured library line is missed. Line coverage is 100%. Registry dependencies are none. `deny.toml` rejects a crates.io or git dependency.

The [README](README.md) is the short entry. Linux setup is [docs/linux.md](docs/linux.md). Windows setup is [docs/windows.md](docs/windows.md).

## Pull requests

1. Keep the crate graph in `scripts/layering.sh`: `json` and `tui` stay free of `host` and `bistill-lib`; `host` stays free of `json`; `bistill-lib` stays free of `tui`; the `bistill` binary stays free of the `json` crate.
2. Fill in the pull request template.

## Code of conduct

See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
