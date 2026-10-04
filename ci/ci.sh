#!/bin/sh
set -eu
cd /src

export CARGO_INCREMENTAL=0
export CARGO_TERM_COLOR=always
# rust-toolchain.toml pins 1.98.0. CI overrides that with the MSRV.
export RUSTUP_TOOLCHAIN=1.85.0

# GitHub's reuse-action lints a checkout. This bind mount may be a submodule
# whose gitdir sits outside /src, so reuse cannot honour .gitignore and
# would scan target/ and ci/out/.
reuse_tree=/tmp/reuse-tree
mkdir -p "$reuse_tree"
tar -C /src \
    --exclude=./target \
    --exclude=./ci/out \
    --exclude=./.git \
    -cf - . | tar -C "$reuse_tree" -xf -
(cd "$reuse_tree" && reuse lint)

cargo fmt --all -- --check
cargo clippy --workspace --locked --all-targets -- -D warnings
./scripts/layering.sh
cargo deny check
cargo test --workspace --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --locked --no-deps
./scripts/coverage.sh
