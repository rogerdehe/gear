# T-194 attempt 2: monitor resumed attempts

Removed the attempt-generation filter from the quiet reminder and loop guard. A restarted lane continues to monitor its existing Started attempt, while the current seat, observed generation, and binding checks before Escape remain in place.

A regression uses a generation-1 attempt with a generation-2 seat and binding. It checks the reminder and captain item, the stale-session Escape and its messages, and throttling on the next tick. `scripts/test.sh` passed all 1,299 tests (build 99 seconds, suite 206 seconds).
