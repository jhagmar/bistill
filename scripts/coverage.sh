#!/bin/sh
# Fail when line coverage of the workspace libraries, excluding tests, is below 100%.
set -eu
cd "$(dirname "$0")/.."
host=$(rustc -vV | awk '/^host:/{print $2}')
sysroot=$(rustc --print sysroot)
tools="$sysroot/lib/rustlib/$host/bin"
rm -rf target/cov-prof
mkdir -p target/cov-prof
RUSTFLAGS="-C instrument-coverage" cargo test --workspace --locked --no-run --message-format=json \
  > target/cov-prof/build.json
bins=$(python3 -c '
import json
found = []
for line in open("target/cov-prof/build.json"):
    line = line.strip()
    if not line.startswith("{"):
        continue
    event = json.loads(line)
    if event.get("reason") != "compiler-artifact":
        continue
    target = event.get("target", {})
    kind = target.get("kind", [])
    if "lib" not in kind or not event.get("profile", {}).get("test"):
        continue
    if event.get("executable"):
        found.append(event["executable"])
print("\n".join(found))
')
test -n "$bins"
export LLVM_PROFILE_FILE="$PWD/target/cov-prof/run-%p-%m.profraw"
first=""
objects=""
for bin in $bins; do
  "$bin" >/dev/null
  if [ -z "$first" ]; then
    first=$bin
  else
    objects="$objects --object $bin"
  fi
done
"$tools/llvm-profdata" merge -sparse target/cov-prof/*.profraw -o target/cov-prof/all.profdata
# shellcheck disable=SC2086
"$tools/llvm-cov" report "$first" $objects \
  --instr-profile=target/cov-prof/all.profdata \
  --ignore-filename-regex='tests\.rs' \
  --sources crates > target/cov-prof/report.txt
awk '
  $1 == "TOTAL" {
    if ($9 != 0) {
      print "line coverage missed " $9 " lines" > "/dev/stderr"
      exit 1
    }
  }
' target/cov-prof/report.txt
# shellcheck disable=SC2086
"$tools/llvm-cov" export "$first" $objects \
  --format=lcov \
  --instr-profile=target/cov-prof/all.profdata \
  --ignore-filename-regex='tests\.rs' \
  > lcov.info
echo "line coverage 100%"
