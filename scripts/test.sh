#!/usr/bin/env bash
# Run every test of the workspace, print each test's time, and warn loudly
# about slow ones. The user's red line (ops/CONVENTIONS.md "Test time"): the
# whole suite under 300 s; 60 s passes, 30 s is good, 10 s is excellent.
#
#   scripts/test.sh            all tests
#   scripts/test.sh -p gear-app  extra arguments go to `cargo nextest run`
set -uo pipefail

SLOW_S=10
RED_LINE_S=300
ALLOW=scripts/slow-tests.allow   # tests allowed over 10 s, each with its own max and reason

cd "$(dirname "$0")/.."
command -v cargo-nextest >/dev/null 2>&1 || {
  echo "error: cargo-nextest is missing: cargo install cargo-nextest --locked" >&2
  exit 2
}

# Build first, so the times below are test time only.
cargo nextest run --workspace --no-run "$@" || exit $?

start=$(date +%s)
cargo nextest run --workspace --no-fail-fast --status-level all --final-status-level none \
  --color never "$@" 2>&1 | awk -v slow="$SLOW_S" -v allow="$ALLOW" '
  BEGIN {
    while ((getline line < allow) > 0) {
      if (line ~ /^[ \t]*(#|$)/) continue
      split(line, f, " ")
      max[f[1] " " f[2]] = f[3] + 0   # key: "<crate> <test>"
    }
  }
  {
    print
    if (match($0, /\[ *[0-9]+\.[0-9]+s\]/)) {
      t = substr($0, RSTART + 1, RLENGTH - 3) + 0
      name = $0
      sub(/^.*\] */, "", name)
      sub(/^\( *[0-9]+\/[0-9]+\) */, "", name)
      limit = (name in max) ? max[name] : slow
      if (t > slow && t <= limit) {
        printf("note: %s took %.1f s, allowed up to %d s by %s\n", name, t, limit, allow) > "/dev/stderr"
      } else if (t > slow) {
        n++
        msg = sprintf("warning: STUPID TEST, MUST FIX: %s took %.1f s (limit %d s). Every lane, reviewer and merge gate pays this on every run. Inject the time instead of sleeping, build the fixture once, wait on a result instead of polling.", name, t, limit)
        print msg > "/dev/stderr"
        if (ENVIRON["GITHUB_ACTIONS"] == "true") print "::warning::" msg
        slow_list = slow_list sprintf("  %6.1f s  %s\n", t, name)
      }
    }
  }
  END {
    if (n > 0) printf "\nwarning: %d stupid test(s) over %d s, fix them:\n%s", n, slow, slow_list > "/dev/stderr"
  }'
status=${PIPESTATUS[0]}
elapsed=$(( $(date +%s) - start ))

if   [ "$elapsed" -gt "$RED_LINE_S" ]; then verdict="OVER THE RED LINE (${RED_LINE_S} s): the suite must be cut now"
elif [ "$elapsed" -gt 60 ];  then verdict="below the pass mark (60 s)"
elif [ "$elapsed" -gt 30 ];  then verdict="pass (60 s)"
elif [ "$elapsed" -gt 10 ];  then verdict="good (30 s)"
else verdict="excellent (10 s)"; fi
if [ "$elapsed" -gt 60 ]; then
  echo "warning: the whole suite took ${elapsed} s: ${verdict}" >&2
else
  echo "suite: ${elapsed} s: ${verdict}"
fi
exit "$status"
