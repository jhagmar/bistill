# Run the GitHub checks locally

From the repository root, with Docker running:

```bash
docker compose run --rm ci
docker compose run --rm codeql
```

`compose.yaml` builds `ci/Dockerfile` and mounts this tree at `/src`. The image installs the same tool versions GitHub uses, so the commands below match the pull request checks.

## The test job

`ci/entrypoint.sh` switches to the user id that owns the tree, then `ci/ci.sh` runs. GitHub Actions also runs the job as a normal user. That matters for tests that lock down a directory or a file and expect the write to fail.

The script lints the license metadata, then runs the `test` job:

1. REUSE
2. `cargo fmt --all -- --check`
3. Clippy, with warnings denied
4. `scripts/layering.sh`
5. `cargo deny check`
6. `cargo test --workspace --locked`
7. `cargo doc --workspace --locked --no-deps`, with rustdoc warnings denied
8. `scripts/coverage.sh`, which requires 100% line coverage and writes `lcov.info`

`RUSTUP_TOOLCHAIN` is `1.85.0`. `CARGO_INCREMENTAL` is `0`, matching GitHub Actions. The named volume `ci-target` holds `/src/target`. After a large move of source files, `docker volume rm bistill_ci-target` (or `docker compose down -v`) drops that cache so the next run compiles cleanly.

## CodeQL

`docker compose run --rm codeql` writes `ci/out/codeql.sarif` through `ci/codeql.sh`. Uploading coverage to Codecov stays on GitHub.

`ci/Dockerfile` pins Ubuntu 24.04, Rust 1.85.0, cargo-deny 0.20.2, reuse 6.2.0, and CodeQL bundle 2.27.0.
