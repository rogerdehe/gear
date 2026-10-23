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
- Captain can scope an automatic rework (today the engine hands the lane every finding, including ones the captain would log).
- T-18 review #4 (restart adoption types shell text into a running reviewer), #5 (re-review does not resume the reviewer's session).
- **Captain quota (user, 2026-10-23, must be in M2).** Today nothing handles it; lanes would keep developing with no captain to steer — risky. Shape to work out in M2:
  - Detect: never from conversation text (it may quote quota words). Read only the agent's status line, e.g. Claude Code's bottom bar `5h 1% ↻01:10 · 7d 64%` (window usage and reset time); per-kind in the profile.
  - While the captain is out: each lane finishes its current batch (hands in), then gets no new task; reviews and merges may finish (no captain judgement needed). Shown on the board.
  - Reset: the captain's reset period is agreed at kickoff (e.g. 5 h, or read from the status line); at reset the engine wakes the captain ("your quota is back — run `gear`") and dispatch resumes after the captain has looked.
- Lane quota "wait 5 h for reset": in M1 via T-15 (park until now + quota_reset_h, resume the same session) — check it really works live once T-15 lands.
- **Fresh lane sessions (root of a lane running out of context).** Long-lived lanes keep one session for every task; after 8 tasks and many rework rounds board was at 62% and asked to stop. At a task boundary (no started attempt, all handed in) and above a context threshold (OpenCode bar `620.2K (62%)`, per-kind in the profile), the engine exits the agent and starts a NEW session in the same pane (env kept), then dispatches. Captain did this by hand once for board (2026-10-23).
