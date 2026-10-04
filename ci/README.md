# Local required checks

From the product root, with Docker running:

```bash
docker compose run --rm ci
docker compose run --rm codeql
```

`compose.yaml` builds `ci/Dockerfile` and bind-mounts this tree at `/src`.

## `ci`

`ci/ci.sh` is the `ci` service command. It matches the GitHub `test` job, plus `reuse lint`:

1. REUSE
2. `cargo fmt --all -- --check`
3. Clippy (`-D warnings`)
4. `scripts/layering.sh`
5. `cargo deny check`
6. `cargo test --workspace --locked`
7. `cargo doc --workspace --locked --no-deps` with `RUSTDOCFLAGS='-D warnings'`
8. `scripts/coverage.sh` (line coverage 100%, and `lcov.info`)

`RUSTUP_TOOLCHAIN` is `1.85.0`. `CARGO_INCREMENTAL` is `0`, matching GitHub Actions. `ci/entrypoint.sh` drops to the uid that owns the tree before the checks, matching the GitHub `runner` user. Named volume `ci-target` holds `/src/target`. After a large source move, `docker volume rm bistill_ci-target` (or `docker compose down -v`) rebuilds that cache.

## CodeQL

`docker compose run --rm codeql` writes `ci/out/codeql.sarif` via `ci/codeql.sh`. Codecov upload stays on GitHub.

`ci/Dockerfile` pins Ubuntu 24.04, Rust 1.85.0, cargo-deny 0.20.2, reuse 6.2.0, and CodeQL bundle 2.27.0.
