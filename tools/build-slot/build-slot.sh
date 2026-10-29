#!/bin/sh
# build-slot.sh: run a heavy command (a build, a test run, a merge gate) with a
# share of the machine sized to what is running on it now, at low priority.
#
#   build-slot.sh <command> [args...]
#
# Any language: it reads the machine, exports the job counts that Rust, Go and
# a project's own Makefile read, and runs the command. No install, no locks,
# no queue, no waiting; macOS and Linux.
#
# N, the jobs for this run:
#   cores    hw.ncpu / nproc
#   load     the 1-minute load average, shown in the line only: it counts
#            every busy thread (agents, editors, other work), lags a burst by
#            a minute, and already includes the builds counted in running, so
#            dividing by it starved every build on a busy machine to 1 job
#   running  the build-slot.sh runs alive on the machine, this one included,
#            the larger of two samples 400 ms apart, so runs that start in the
#            same second all see each other. Every project on the machine must
#            run its heavy commands through a file named build-slot.sh for them
#            to count one another.
#   free     free memory in percent (kern.memorystatus_level / MemAvailable)
#   N = max(1, cores / running); at most 2 when free memory
#   is under 25 %, 1 when under 15 %.
#
# Exported to the command:
#   BUILD_JOBS, CARGO_BUILD_JOBS, NEXTEST_TEST_THREADS = N
#   GOMAXPROCS = max(1, cores / N): Go runs -p packages times -parallel cases,
#   and -parallel follows GOMAXPROCS, so the product stays at the core count.
#   BUILD_SLOT = 1: a build-slot.sh started under it runs its command at once,
#   without counting itself or changing these, so the outermost run owns the
#   slot.
#
# Priority: taskpolicy -c utility on macOS (lower priority, still on the
# performance cores; -b keeps to the efficiency cores even on an idle machine,
# measured at 2.2 times the CPU time), nice -n 10 elsewhere.
#
# One line goes to stderr:
#   build-slot: N (cores C, load L, running R, free memory M %)
# BUILD_SLOT_CORES, BUILD_SLOT_LOAD, BUILD_SLOT_RUNNING and BUILD_SLOT_FREE
# replace the readings (for tests).

[ $# -gt 0 ] || { echo "usage: build-slot.sh <command> [args...]" >&2; exit 2; }

if [ "${BUILD_SLOT:-}" = 1 ]; then
    exec "$@"
fi

cores=${BUILD_SLOT_CORES:-$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 1)}

if [ -n "${BUILD_SLOT_LOAD:-}" ]; then
    load=$BUILD_SLOT_LOAD
elif [ -r /proc/loadavg ]; then
    read -r load _ < /proc/loadavg
else
    load=$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}')
fi

if [ -n "${BUILD_SLOT_FREE:-}" ]; then
    free=$BUILD_SLOT_FREE
elif [ -r /proc/meminfo ]; then
    free=$(awk '/^MemTotal:/{t=$2} /^MemAvailable:/{a=$2} END{if (t) printf "%d", a * 100 / t; else print 100}' /proc/meminfo)
else
    free=$(sysctl -n kern.memorystatus_level 2>/dev/null || echo 100)
fi

# The runs alive now. pgrep writes to a file, not through a command
# substitution: a subshell of this script carries its command line and would
# count as one more run.
count_runs() {
    list=${TMPDIR:-/tmp}/build-slot.$$
    # A shell running this file ("/bin/sh .../build-slot.sh ..."), not every
    # command line that names it (a `sh -c "... build-slot.sh ..."` around it,
    # an editor, a grep). -a: BSD pgrep leaves out its own ancestors, this run
    # among them, unless asked; Linux pgrep counts it anyway and reads -a as
    # "print the command line", one line per process all the same.
    pgrep -a -f '^[^ ]*sh [^ ]*build-slot\.sh( |$)' > "$list" 2>/dev/null
    n=0
    while read -r _; do n=$((n + 1)); done < "$list"
    rm -f "$list"
    [ "$n" -ge 1 ] || n=1
}
if [ -n "${BUILD_SLOT_RUNNING:-}" ]; then
    running=$BUILD_SLOT_RUNNING
else
    count_runs; first=$n
    sleep 0.4
    count_runs
    running=$n
    [ "$first" -gt "$running" ] && running=$first
fi

jobs=$(awk -v c="$cores" -v l="${load:-0}" -v r="$running" -v f="${free:-100}" 'BEGIN {
    n = int(c / r)
    if (f < 25 && n > 2) n = 2
    if (f < 15) n = 1
    if (n < 1) n = 1
    if (n > c) n = c
    print n
}')
gomax=$((cores / jobs)); [ "$gomax" -ge 1 ] || gomax=1

echo "build-slot: $jobs (cores $cores, load $load, running $running, free memory $free %)" >&2

BUILD_SLOT=1 BUILD_JOBS=$jobs CARGO_BUILD_JOBS=$jobs NEXTEST_TEST_THREADS=$jobs GOMAXPROCS=$gomax
export BUILD_SLOT BUILD_JOBS CARGO_BUILD_JOBS NEXTEST_TEST_THREADS GOMAXPROCS

# A child, not exec: this process stays counted until the command ends.
if command -v taskpolicy >/dev/null 2>&1; then
    taskpolicy -c utility "$@"
else
    nice -n 10 "$@"
fi
