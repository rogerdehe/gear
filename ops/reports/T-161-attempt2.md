# T-161 attempt 2: authoritative pause accounting

Removed the duplicate round-total update from the early wait reconciliation pass. Wait reconciliation still opens and closes wait rows and queues the merge wake; the later overrun pass now computes the pause union once, after merge collection and quota-pause reconciliation have established the end times.

The new fake-clock regressions cover a merge collected at T+230 with completion at T+200 (190 seconds paused, 40 active), and a full tick after a quota resume at T+2100 observed at T+2200 (1,400 seconds paused, 800 active). The integration merge retained both the wait and session-dispatch test modules and the combined doorbell slot rules.

The existing wake test now checks a missing agent in a separate fixture, so its fake session recovery does not prevent the independent wake-delivery assertion. All five wait contract tests passed. After merging origin/dev through 576713a0, `scripts/test.sh` passed all 1,170 tests (9 s build, 74 s suite); the suite emitted slow-test warnings under concurrent load. The preceding full run exposed the fixture issue (1,169 passed, 1 failed), which was corrected before the passing run.
