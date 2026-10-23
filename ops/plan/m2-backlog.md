# M2 backlog (ideas, not yet tasks)

- A lane's `result` should not be a captain action item once the engine starts the reviewer (it rings the captain for nothing); make it an information line, and ring the captain only for the review verdict / failed merge.
- T-5 review R2-1 (accepted start repeated after crash + agent exit while down), R2-3 (quota lowered mid-tick).
- T-12 review: failed-merge retry/duplicate items, remote cleanup errors, Unknown merge ops of cancelled tasks, Executing before the gate.
- T-17 review: R2 slot freed one tick late, R3 exit keys repeated after a restart.
- Deletion pass after M1 (old manual scripts and ledger, Python-era design docs, duplicate notes, dead code).
- T-13 review R2-1: a review item the captain deferred (Waiting) survives settlement.
- T-7 review R1 (lane runs dry again: alert waits for the 10-minute window), R2 (deferring an alert creates another item).
- Hand-in check: `gear send --kind result` refuses a candidate that does not contain current origin/dev (lanes skip the rule under rework: T-13, T-15).
- T-17 review R5: cancel during a reviewer's start keeps its slot (Starting seats skipped by release).
