# M5 board prototype (draft for the user's review, 2026-10-28)

`python3 gear/voyages/M5/board/prototype.py` runs it with fake data in any terminal (curses, no dependencies). It replaces the M4 prototype (`gear/voyages/M4/board/`) once the user agrees.

What changed from M4, from the user's review of the real board:
- **Every voyage, not only the current one.** The header lists the voyages, newest first (⛵ sailing, ⚓ landed); `[` and `]` pick an older or newer one, and every view shows the one picked. View 0 lists every voyage with its goal and how each clause of it ended.
- **The goal is always on screen.** The header carries the picked voyage's done sentence, its stations (✓ reached, ▶ now, · next), the distance to each clause, and a red line when the goal was changed (with the user's yes; Decisions shows before and after).
- **The full width.** Views split into two columns on a wide terminal; bars and columns follow the width.
- **Task stages in words**: waits for T-n to merge, queued, delivered not started, working, waits for the captain's answer, handed in and waits for a reviewer, in review, waits for the captain's verdict, sent back, reworking (round n), passed and waits to merge, merging, merge failed, merged, failed, moved to another voyage.
- Views:
  0. Voyages — every voyage: state, time against eta, bar, goal, each clause met (✓) or not (✗).
  1. Now — lanes left (task, stage, time against estimate, done and left), right: needs a look (open tasks only), quota.
  2. Graph — dev as the main line, newest on top: each task branches off dev where its work starts (after its dependencies merged) and joins dev at its merge ●; a waiting task hangs dashed (┆) with what it waits for.
  3. Workload — says whether the lanes are balanced: per lane what it did and has left, when it ends, and which lane to move a wave from.
  4. Decisions — a summary: what changed the course (★), what waits on whom, counts; the full log stays in the store (`gear report voyage`).
  5. Time & tokens — grouped by lane: time, development tokens and cost, review tokens and cost, total; Enter expands a lane into its tasks.
