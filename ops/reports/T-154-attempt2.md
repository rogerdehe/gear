# T-154 attempt 2: one open item per fact

Engine alerts reuse their open or deferred item when facts change, close on recovery, and raise a new item after the configured captain reminder interval (15 minutes by default) if a resolved condition persists. This applies to lane feed and imbalance, all-tail, stalled, busy, and blocked alerts. Missing-agent recovery closes its stall; outstanding failed deliveries remain eligible for a reminder. Blocked items refresh their screen evidence.

Coverage exercises five stall ticks, changing a fact without changing its item, recovery, and the 15-minute re-alert boundary. Existing feed tests now expect updates in place when a shortage worsens.

Validation: `cargo test -p gear-engine --lib` passed (234 tests before the new focused tests); `scripts/test.sh` passed on the final changes: 1,220 tests, 0 failures (42 s build, 68 s suite). The first full run found one outdated fresh-item expectation (1219/1220); that expectation was updated for this contract.
