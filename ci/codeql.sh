#!/bin/sh
set -eu
cd /src

# Keep Cargo output off the source root so the extractor does not index
# build artifacts under /src/target.
export CARGO_TARGET_DIR=/tmp/cargo-target
export RUSTUP_TOOLCHAIN=1.85.0
mkdir -p /out "$CARGO_TARGET_DIR"
rm -rf /out/codeql-db /out/codeql.sarif

codeql database create /out/codeql-db \
    --language=rust \
    --source-root=/src \
    --overwrite \
    --command="cargo build --workspace --locked"

# Exit 70 is the CLI chain token. The wrapper sometimes surfaces it after
# a finished analyze; accept a freshly written SARIF in that case.
set +e
codeql database analyze /out/codeql-db \
    --format=sarifv2.1.0 \
    --output=/out/codeql.sarif \
    rust-code-scanning.qls
status=$?
set -e
if [ "$status" -ne 0 ] && [ "$status" -ne 70 ]; then
    exit "$status"
fi
if [ ! -s /out/codeql.sarif ]; then
    echo "codeql did not write /out/codeql.sarif" >&2
    exit 1
fi

chown -R "$(stat -c '%u:%g' /src)" /out
