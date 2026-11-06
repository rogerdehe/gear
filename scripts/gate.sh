#!/usr/bin/env bash
# The merge gate, timed. Every phase prints its seconds, and the whole gate
# fails when it takes more than GATE_LIMIT_S (80 s). Normal runs stay near half
# of that, so later work has room; over half prints a warning.
set -uo pipefail
cd "$(dirname "$0")/.."
LIMIT=${GATE_LIMIT_S:-80}
start=$(date +%s)
summary=""

phase() {
  local name=$1; shift
  local t=$(date +%s)
  "$@"
  local rc=$?
  local d=$(( $(date +%s) - t ))
  summary="$summary $name ${d}s"
  echo "gate phase: $name ${d} s"
  if [ $rc -ne 0 ]; then echo "gate: $name failed (phases:$summary)"; exit $rc; fi
}

phase fmt   cargo fmt --all --check
# The build is what the suite runs: libraries, binaries and tests. Examples
# (manual live probes) are built by `make full`, not on every merge.
phase build env BUILD_SLOT_MIN=4 BUILD_SLOT_PRIORITY=normal tools/build-slot/build-slot.sh cargo build --workspace --lib --bins --tests
# The suite runs with the machine's cores, as the plain `cargo nextest run
# --workspace` does (T-608). build-slot sizes a *build* to a fair share of the
# machine; a test run is mostly waiting on git subprocesses, and capping its
# threads at that share made the gate's suite 48 s under load (32 s with the
# cores it has), over half the 80 s limit. The build phase above keeps the
# build-slot cap.
phase test  env BUILD_SLOT_MIN=4 BUILD_SLOT_PRIORITY=normal tools/build-slot/build-slot.sh env -u NEXTEST_TEST_THREADS scripts/test.sh

total=$(( $(date +%s) - start ))
echo "gate: total ${total} s, limit ${LIMIT} s (phases:$summary)"
if [ "$total" -gt "$LIMIT" ]; then
  echo "gate: FAILED, the gate took ${total} s, over its ${LIMIT} s limit. Find the slow phase above and fix it; never raise the limit to pass." >&2
  exit 1
fi
if [ "$total" -gt $(( LIMIT / 2 )) ]; then
  echo "gate: warning, ${total} s is over half the ${LIMIT} s limit; the room left for later work is shrinking." >&2
fi
