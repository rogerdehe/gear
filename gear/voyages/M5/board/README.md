# M5 board prototype (agreed with the user, 2026-10-28)

`python3 gear/voyages/M5/board/prototype.py` runs it with fake data in any terminal (curses, no dependencies).

**Which one is the standard:** the M4 prototype (`gear/voyages/M4/board/`) stays the contract for everything this list does not change. The M5 prototype is the M4 prototype with the changes below and nothing else: the same colours (ok green, warn yellow, bad red, dim grey, acc cyan; lanes cyan, magenta, yellow, blue), the same symbols, views, keys, mouse clicks and Enter panels. A lane builds the real board (`crates/gear-board`) to the M4 prototype plus these changes; where the real data differs, keep the layout and the meaning.

Changes against M4, each from the user's review of the real board (2026-10-28):

1. **Every voyage, one at a time.** The board keeps every voyage of the project; `[` picks the older one, `]` the newer one, and the whole board then shows that voyage only (header, every view, every panel). The header's first line lists every voyage, newest first (⛵ sailing, ⚓ landed), with the picked one lit; the second line names the picked voyage. A landed voyage keeps its data (no "time unknown", no empty lanes: its lanes read "retired at the landing").
2. **The goal, boxed and bright,** under the title: the voyage's done sentence in a yellow bold box, and in the same box the quality bar with one line on what it blocks — the bar is how the goal is judged, so it is shown and locked with the goal (user, 2026-10-28). When the goal was changed (with the user's yes), a red line under the box gives the time, before and after.
3. **Every header row says what it is.** Under the goal box: `Goal check` (each clause of the done sentence, ✔ met or ✗ not yet, with how far it is; M4 listed them at the bottom of the Now view as "DONE SENTENCE"), `Milestones` (the voyage's stations: ✔ reached at, ▶ now and planned by, · next), `Time` and `Tasks`.
4. **Time against the plan:** a bar with █ used, ◆ expected finish and │ planned end, a legend line under it, and in words: used, planned, expected (slowest lane), how far ahead or behind plan; a landed voyage says how long it took against the plan.
5. **The full width.** Bars and the task-title columns grow with the terminal (M4 cut titles at 32-34 characters at fixed columns).
6. **Tasks of this voyage only**, as a bar in three parts (█ merged, ▓ in progress, ░ not started) with a legend; tasks moved to another voyage are named apart ("moved out: T-n") and not counted. In the Now view each lane also gets a small bar: n of m tasks done, what is left.
7. **Task stages in words**, one per task in time order: waits for T-n to merge · queued · delivered, not started · working · waits for the captain's answer · handed in, waits for a reviewer · in review · waits for the captain's verdict · sent back, rework queued · reworking (round n) · passed, waits to merge · merging (gate running) · merge failed, waits for the captain · merged · failed (rounds used up) · moved to another voyage. M4's symbols stay; new ones: ◌ delivered, ◍ passed or merging, ✗ failed or merge failed, → moved. A mark beside the stage when it holds (e.g. "past its budget").
8. **Tasks (was Graph) draws the dependencies like a git graph:** dev is the main line (green), newest on top; each task is a branch in its lane's colour that leaves dev (├─╮) where its work starts — after what it depends on merged, "starts +59m after T-258" — and joins dev at its merge (●─╯); an open task sits on its branch with its stage symbol; a task not started yet hangs dashed (○┆) in its lane with what it waits for. Columns: task, what, stage, time. ↑↓ selects a task and lights up what it needs and what needs it; Enter opens M4's task panel. This replaces M4's dev / rev / lane-dot layout.
9. **(Folded into Now, see Four views.) Workload says whether the work is balanced:** each lane's "ends about", and one line: balanced, or "not balanced: <lane> ends <time> after <lane> — move its next startable wave". M4's bars stay; M4's burn-up chart is removed (user, 2026-10-28): the time bar in the header already shows the pace against the plan.
10. **Log (was Decisions) starts with a summary:** what changed the course (★), then M4's full list and the friction reports.
11. **Cost (was Time & tokens), grouped by lane:** one row per lane with estimate, actual, ratio, rounds, development tokens and cost, review tokens and cost, total; Enter on a lane opens its tasks (M4's per-task rows). The footer gives the voyage's tokens and cost, development and review apart.
12. **Clicks pick a view and a voyage** (user, 2026-10-28: clicking wins over selecting text, since a terminal app cannot have both): a click on a tab picks the view, as in M4, and a click on a voyage in the first line picks the voyage. Selecting text is left to the terminal's own modifier (Option- or Shift-drag) where it has one.
13. **A selected row is lit across the whole width, in one colour** (M4 lit only the cells drawn with the selection, so a row showed two or three different backgrounds).
14. **The header is folded by default:** under the goal box, one row with the goal check (n of m met, the next unmet clause) and the current milestone (n of m reached), one row with the time bar (expected finish against the plan, ahead or behind) and the task bar, and the quality bar. `d` unfolds every clause, every milestone and the legends, and folds them again; the views keep their room however many milestones a voyage has. The header labels (Goal check, Milestone, Time, Tasks, Quality bar) are bold titles, not grey.
15. **The views are a tab bar that looks like one:** framed (╭─╮ │ ╰─╯), each tab separated, the picked one lit, under the header.

## Four views (chosen by the user, 2026-10-28, over the five-view draft)

Views: **Now** (what happens and what needs a hand), **Tasks** (the dependency graph), **Cost** (Time & tokens by lane), **Log** (the decisions and the friction reports). Workload folds into Now; "waiting on someone" moves from Decisions to Now.

Now is one table, one row per lane: LANE, NOW DOING, STAGE, TIME (actual/estimate, ⚠ over), LANE DONE (bar, n of m, minutes of estimate left), ENDS. Under it one line: balanced, or which lane ends how long after which. Then **TO HANDLE**: needs a look, waiting on someone, and every lane that will sit idle 20 minutes or more ("hull will sit idle ~1h31m: its next task T-267 waits for T-266 (mast) — captain: give hull work it can start now, or move T-266 to hull"). Quota on one line.

ENDS and the idle time come from each lane's queue played forward (the engine's run-dry forecast in feed.rs does the same; the board reuses it): a lane is free again when it hands a task in; its next task starts when the lane is free and every task it depends on has merged (hand-in plus the voyage's measured review-and-merge time); durations are the estimate left times the lane's measured pace (actual / estimate of its merged tasks); the gap between free and start is idle time. The board does not print this explanation.

## Golden frames

`python3 gear/voyages/M5/board/prototype.py --golden <dir>` draws the frames without a terminal and writes them to
`<dir>`; `gear/voyages/M5/board/golden/` holds the committed ones. The real board is checked against them cell by
cell, so a missing colour, a wrong width or a moved column fails the gate.

How they are made (export mode only; the prototype's drawing is frozen and the interactive mode, no flag, is
unchanged): the frame is drawn into a `FakeWin` of 48 rows × 160 columns — a window that keeps its cells instead of
opening a curses screen, with the calls the prototype makes (`getmaxyx`, `erase`, `addnstr`, `chgat` setting a run of
cells' attribute, `refresh`, `keypad`). `n` in `addnstr` counts characters as in curses, and a character that does not
fit in the line's remaining cells is not drawn (the prototype keeps every write inside its row). A cell is a terminal
cell, not a codepoint: a wide character (East Asian wide or fullwidth, here ⛵ ⚓ ⏳) is one glyph in two cells — the
first holds it, the second is its continuation, a space with the same attribute — and a combining character joins the
one before it. Overwriting one half of a wide character blanks the other half. `C` then holds marker bits instead of
colour pairs (`curses.A_BOLD` and `curses.A_REVERSE` are two more markers), so every cell's attribute is a set of
markers the export can name again. It draws the first voyage (`pick(0)`) with `sel = 0`, each view — 1 Now, 2 Tasks,
3 Cost, 4 Log — once with `DETAIL[0] = False` and once with `True`, header and footer included, exactly as `main`
draws a frame. Running it twice gives identical files.

`FakeWin.chgat(y, x, n, attr)` sets the attribute of each of the n cells to `attr`: as in curses it **replaces** the
old attribute (colour pair and bold included), it does not OR into it, so the row `hl` lights is reverse video with no
colour and no bold — exactly change 13, "one colour".

`fixture.json` is the fake data the frames were drawn from (VOYS, WHEN, NOW, REVIEW_MERGE and view_now's quota list);
`crates/gear-board/tests/it/golden_fixture.rs` loads it into a store.

Each frame is three files of 48 lines of 160 cells (UTF-8, one character per cell, a trailing newline per line):

- `<view>-<folded|open>.txt` — the characters, a blank cell as a space; the second cell of a wide character is a
  space too, so a line is 160 characters whether it holds wide glyphs or not;
- `<view>-<folded|open>.col` — one letter per cell for the colour: `.` no colour, `o` ok, `w` warn, `b` bad, `d`
  dim, `a` acc, `m` mag, `u` blu; the letter is uppercase when the cell is bold, and a bold cell with no colour is
  `*` (the header titles, the picked voyage and the picked tab); the cells a `chgat` of a selected row lights carry
  reverse and no colour, so they read `.`;
- `<view>-<folded|open>.rev` — `R` where reverse is on, else `.`.

The views are named as their tabs are: `now`, `tasks`, `cost`, `log`. Eight frames, 24 files. A change to the
prototype's drawing is a change to the golden files: regenerate them, commit them with the change, and only with the
user's yes (the prototype is frozen once agreed).
