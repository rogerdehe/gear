# T-156 — the new board, attempt 2 (F1–F7)

Scope: T-156 attempt 2 — the review of attempt 1: F1, F2, F3, F4, F5, F6, F7.

Branch: `deck/t156` (the same branch as attempt 1, per the conventions; code @
the hand-in commit; `origin/dev` merged before verification)

## The findings, one by one

**F1 — the visible tabs and rows did not match the mouse.** The header put
the projection on a row of its own and the engine warning on another, so the
tab row moved; the hit testing always took the fourth row as the tabs. The
header now has the prototype's four rows (the title, the time, the tasks, the
tabs): the projection sits **on the time row**, a warning never moves the tab
row, the drawing and the hit testing share `TAB_ROW` and `FIRST_ROW`, and the
open tab is marked in reverse video, as the prototype marks it.

**F2 — the frame ignored the terminal.** Every row was printed at full
length, the footer was appended to the body and the screen could scroll its
own tabs away. The board now asks the terminal for its size (`Keys::size`,
the ioctl on the terminal's descriptor, with a default for a pipe), draws
every row on a **canvas** of that size (each row is cut at the edge), keeps
the footer on the last row, bounds the body to the viewport and scrolls so
that the selected row stays on the screen.

**F3 — the panel was appended and cut.** The panel is drawn on the same
canvas, over the view, with its border; its size comes from the terminal, not
from the length of the body, so a short graph shows a long panel; and **any
key closes it** before the board's own keys (the prototype's window).

**F4 — the selection walked past the last row.** The selection is clamped to
the number of rows of the view, after a move and after a rebuild (a view that
shrank).

**F5 — leaving the board enabled the mouse.** `leave` writes
`ESC[?1000l` (and `ESC[?1006l`) now: the terminal keeps its clicks, not the
user's shell.

**F6 — the test read partial frames.** The pty test now reads **whole
frames**: every read waits (by poll, without a sleep) for the next frame that
holds what the test looks for, and the frame ends with the footer. It clicks
the **rendered** tab and row positions (the column of "2 Graph" and of "1 Now"
as they are drawn), and it covers the left arrow, a row click with `Enter`, a
panel that any key closes, and a frame with a voyage, an idle reason, a quota
reading and the done sentence.

**F7 — the time bar was the pace, not the time.** The bar is now the voyage's
**elapsed minutes against its eta** (the eta of the plan is parsed: `12h` is
720 minutes), so a voyage an hour into a 12 h eta shows two cells of 24.
When either is unknown the bar says so.

One wording change came with F2: the prototype's footer is 91 characters and
cannot fit an 80 column terminal (the review named it), so the board's is
shorter — "←→ / Tab / 1-5: views · ↑↓ select · Enter: a lane or a task · q
quit" — with the same keys.

## Tests

`crates/gear-board/tests/it/board_contract.rs`:

- `the_new_board_draws_its_views_and_the_keys_move_between_them` — the board
  runs in a **pty of 80×24**; the test reads whole frames by poll (no sleep)
  and checks the frame and the first view from the store (the title, the five
  tabs, the lanes with their idle reason, the quota reading, the done
  sentence, the time bar with two cells of 24), the right and left arrows, a
  click on the rendered Graph tab and on the rendered Now tab, a click on the
  first lane row, `Enter` opening the lane's panel, any key closing it, `j`
  twice keeping the selection on the last lane, and `q` leaving the board.

## Verification

- `cargo test -p gear-board` → **49 passed, 0 failed** (the pty test in
  0.08 s).
- `cargo build --workspace --all-targets` → clean, **no warning**.
- `scripts/test.sh` → **1046 tests run, 1046 passed, 0 skipped**.
- The tests over 10 s that the suite warns about are the machine (the load
  average was 30–80 while the suite ran): the git-heavy engine tests, which
  this branch does not touch.

## Open questions

None.

## Out of scope

- Views 3 to 5 (Workload, Decisions, Time & tokens): the next tasks. They
  show the next task for now.
- The lane order of the columns is the seat order of the store (the board
  cannot read the config's lane order).
