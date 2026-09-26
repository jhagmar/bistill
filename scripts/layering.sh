#!/bin/sh
# Fail when a manifest lists a dependency that is not a path or workspace crate.
set -eu
cd "$(dirname "$0")/.."
fail=0
for f in $(find . -name Cargo.toml -not -path './target/*'); do
  if awk '
    /^\[(dependencies|dev-dependencies|build-dependencies)(\..*)?\]/ { dep = 1; next }
    /^\[/ { dep = 0 }
    dep && $0 ~ /^[[:space:]]*[^#[:space:]]/ && $0 !~ /path *=/ && $0 !~ /workspace *=/ {
      bad = 1
    }
    END { exit bad ? 0 : 1 }
  ' "$f"; then
    echo "crates.io dependency in $f" >&2
    fail=1
  fi
done
exit "$fail"
