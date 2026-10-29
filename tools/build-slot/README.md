# build-slot

Run a heavy command (a build, a test run, a merge gate) with a share of the
machine sized to what is running on it now, at low priority. For several
projects and agents that build on one machine: each run takes less when others
are running and more when the machine is free, and memory is never run out.

```sh
tools/build-slot/build-slot.sh cargo nextest run -p my-crate
tools/build-slot/build-slot.sh go test -p "$BUILD_JOBS" ./...   # inside a Makefile: -p $$BUILD_JOBS
```

No install, no config, no locks, no queue, no waiting. macOS and Linux, POSIX
`sh`. Copy the file into any repository; it must keep the name
`build-slot.sh`, which is how runs of every project on the machine count each
other.

## The rule

| reading | from |
|---|---|
| cores | `sysctl -n hw.ncpu` / `nproc` |
| load | the 1-minute load average, shown in the line only (it counts every busy thread, lags a burst by a minute, and already includes the builds counted in running) |
| running | the `build-slot.sh` runs alive on the machine, this one included; the larger of two samples 400 ms apart, so runs started in the same second see each other |
| free memory | `kern.memorystatus_level` / `MemAvailable` over `MemTotal`, in percent |

`N = max(1, cores / running)`, then at most 2 when free memory is under 25 %
and 1 when under 15 %. CPU contention only slows every run (and the low
priority keeps the machine usable); memory is what can take a machine down,
so the memory readings are the hard limits.

## What the command gets

| variable | value | read by |
|---|---|---|
| `BUILD_JOBS` | N | your Makefile: map it to your own knob (`go test -p`, `--maxWorkers`, …) |
| `CARGO_BUILD_JOBS` | N | cargo |
| `NEXTEST_TEST_THREADS` | N | cargo-nextest |
| `GOMAXPROCS` | max(1, cores / N) alone, at most 2 while other runs are alive | Go: `-p` packages times `-parallel` cases (which follows GOMAXPROCS); a lone run stays at the core count, a crowded machine keeps a little case parallelism without each run taking the whole machine |
| `BUILD_SLOT` | 1 | `build-slot.sh` itself: a run started under it runs its command at once and changes nothing, so the outermost run owns the slot |

It prints one line to stderr:

```
build-slot: 3 (cores 10, load 4.20, running 2, free memory 61 %)
```

The command runs as a child, at `taskpolicy -c utility` on macOS and
`nice -n 10` elsewhere, and its exit status is the script's. (`taskpolicy -b`
is not used: it keeps a process on the efficiency cores even on an idle
machine, measured at 2.2 times the CPU time.)

## Test

```sh
sh tools/build-slot/test.sh
```

`BUILD_SLOT_CORES`, `BUILD_SLOT_LOAD`, `BUILD_SLOT_RUNNING` and
`BUILD_SLOT_FREE` replace the four readings.
