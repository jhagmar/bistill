#!/bin/sh
# Fail when line coverage of crates/json/src, excluding tests, is below 100%.
set -eu
cd "$(dirname "$0")/.."
host=$(rustc -vV | awk '/^host:/{print $2}')
sysroot=$(rustc --print sysroot)
tools="$sysroot/lib/rustlib/$host/bin"
rm -rf target/cov-prof
mkdir -p target/cov-prof
RUSTFLAGS="-C instrument-coverage" cargo test --workspace --locked --no-run --message-format=json \
  > target/cov-prof/build.json
bin=$(python3 -c '
import json
found = ""
for line in open("target/cov-prof/build.json"):
    line = line.strip()
    if not line.startswith("{"):
        continue
    event = json.loads(line)
    if event.get("reason") != "compiler-artifact":
        continue
    target = event.get("target", {})
    if target.get("name") != "json" or not event.get("profile", {}).get("test"):
        continue
    if event.get("executable"):
        found = event["executable"]
print(found)
')
test -n "$bin"
export LLVM_PROFILE_FILE="$PWD/target/cov-prof/run-%p-%m.profraw"
"$bin" >/dev/null
"$tools/llvm-profdata" merge -sparse target/cov-prof/*.profraw -o target/cov-prof/all.profdata
"$tools/llvm-cov" report "$bin" \
  --instr-profile=target/cov-prof/all.profdata \
  --ignore-filename-regex='tests\.rs' \
  --sources crates/json/src > target/cov-prof/report.txt
awk '
  $1 == "TOTAL" {
    if ($9 != 0) {
      print "line coverage missed " $9 " lines" > "/dev/stderr"
      exit 1
    }
  }
' target/cov-prof/report.txt
echo "line coverage 100%"
