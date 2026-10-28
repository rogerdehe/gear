# T-154 attempt 3

F1: resolving a stall examines the latest engine incident even when its item is already resolved. A bounded check-in clears its active marker only if the incident is not newer than that check-in; older evidence cannot clear a later incident.

F2: each tick checks the task and attempt referenced by a failed delivery. A terminal task or attempt clears that record and resolves its stall, while a separately observed current stall remains open. Missing seats are checked as well as present ones.

Full-tick regressions cover a captain-resolved delivery followed by a late check-in, cancellation followed by the reminder boundary, and cancellation while the agent is missing. A bounded-evidence regression covers a newer resolved incident. `scripts/test.sh` passed: 1,264 tests, zero failures (121 s build, 149 s suite under shared build contention).
