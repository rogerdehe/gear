# M4 board prototype (agreed with the user, 2026-10-26)

`python3 ops/plan/m4-board/prototype.py` runs it with fake data in any terminal (curses, no dependencies). This is the agreed design for `gear board` in M4: build the real board to match it, reading the store instead of the fake data. Where the real data differs, keep the layout and the meaning.

Views (←/→, Tab, 1-5, or a click on the tab row; ↑↓ select; Enter details; q quit):

1. Now — lanes (current task, elapsed vs estimate, done/left count and minutes; an idle lane says why: nothing planned, waiting for a named task, or ready but not delivered), "needs a look" (over estimate, rework, an overloaded lane next to idle ones), quota per provider (used %, reset time), the done sentence. Enter on a lane: what it did, what it does now, what it has left in order.
2. Graph — like `git log --graph`: dev, a review column, one column per lane, newest on top; ○ queued, ◉ working, ◎ in review, ↺ reworked, ✓● reviewed and merged. Cross-lane dependencies as "needs T-n"; the selected row lights up what it needs and what waits for it. Enter on a task: its rounds (dev, review, rework, re-review: time, tokens, verdict), what it depends on, what needs it.
3. Workload — done and left per lane (minutes of estimate), the projected end of the last lane, a burn-up per voyage hour.
4. Decisions — the captain's decisions (★ = worth a look at landing) and the agents' friction reports.
5. Time & tokens — per task: estimate, actual, ratio, rounds, tokens, cost.

Not on the board, by design: an agent's pane, its last lines, sending it a message (Herdr's agent panel does that), and items that wait for the captain.
