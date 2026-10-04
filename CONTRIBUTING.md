# Contributing

Thank you for contributing to bistill.

The [README](README.md) is the guide for someone using the program. This page is for someone changing it. Linux setup is [docs/linux.md](docs/linux.md). Windows setup is [docs/windows.md](docs/windows.md).

## Bootstrap

Install Rust 1.85.0, including `rustfmt`, `clippy`, and `llvm-tools-preview`. The committed `rust-toolchain.toml` selects 1.98.0 for day-to-day builds. Continuous integration sets `RUSTUP_TOOLCHAIN=1.85.0`, which is the oldest compiler this repository supports.

The full local check also needs `cargo-deny` 0.20.2 and `reuse` 6.2.0. If you have Docker, [ci/README.md](ci/README.md) runs the GitHub checks in a container so you do not install those tools yourself.

Once per clone, point Git at the hooks in this repository. The hook formats Rust with the 1.85.0 toolchain before each commit:

```bash
git config core.hooksPath .githooks
```

Continuous integration still rejects a commit that `cargo fmt` would change. The hook saves you that round trip.

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

The GitHub `test` job runs the cargo commands and the scripts. The REUSE workflow runs `reuse lint`. `./scripts/coverage.sh` fails when any measured library line was missed. The floor is 100%. The workspace has no crates.io dependencies. `deny.toml` rejects a registry dependency or a git dependency.

## Pull requests

Keep the crate boundaries that `scripts/layering.sh` checks. `json` and `tui` do not depend on `host` or `bistill-lib`. `host` does not depend on `json`. `bistill-lib` does not depend on `tui`. The `bistill` binary does not depend on the `json` crate.

Fill in the pull request template, including how you ran the checks.

## Code of conduct

See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
