# Review of T-356

Status: done
Verdict: pass
Reviewed: review/t356 @ ed702545   Reviewer: gear:review-11 (deepseek-v4-pro-0813)   Round: 2
Contract: v4

## Mechanical acceptance
- build: `cargo build -p gear-board --all-targets` → clean, zero warnings
- tests: `make check CRATE=gear-board` → 221 passed / 0 failed on the re-run (43.6 s). The first whole-crate run had one flaky failure: `board_contract::a_long_graph_scrolls_and_a_tall_panel_keeps_its_borders` (21.3 s, pty timing on a load-56 machine); it passed alone (2.0 s) and passed again on the full re-run. The compiler build is clean.
- allowed files: ok — the three-dot diff `origin/dev...origin/mast/t356` lists only `crates/gear-board/src/frame.rs`, the crate's test files (`tests/it/board_contract.rs`, `colour_contract.rs`, `golden_contract.rs`, `header_contract.rs`) and the required report `gear/voyages/M5/reports/T-356.md`. `src/lib.rs` is unchanged because the existing `run_with` handler already reads `Layout.tabs` and `Layout.tab_row` with inclusive ranges, which is exactly what this task needs.
- merges into dev: clean — `git merge-tree --write-tree origin/dev origin/mast/t356` exits 0, no conflicts.

## Spec compliance
- ok. `header` in `crates/gear-board/src/frame.rs` draws exactly the contracted bar: `╭`+`─`·(W−4)+`╮` Acc at column 1 on row `tab_row−1`; per tab `│` Acc at `x`, the label `"  1 Now  "` (two spaces per side) at `x+1`, Acc+bold+reverse for the shown tab and bold alone for the others, `(x, x+len(label))` pushed to `Layout.tabs`, `x += len(label)+1`; then `│` Acc at `x` and at `W−2`; `╰`+`─`·(W−4)+`╯` Acc at column 1 on `tab_row+1`; `Layout.tab_row = tab_row`, `Layout.top = tab_row + 3`. `Tab::label` stays because the frame and a board test read it, and now returns the bar's two-space text. The eight `T-356` golden entries are removed and rows 10–12 / 18–20 compare against the prototype with no exclusion; the gates pass.
- The click range matches the prototype's own `TABX.append((x, x + len(label), i, y))`, and the existing inclusive `column >= first && column <= last` handler picks the right tab. The new `a_click_on_column_15_of_the_tab_bar_picks_the_tasks_tab_in_both_headers` covers both the folded header (column 15 → Tasks) and the `d`-unfolded header on the moved-down row.

## Findings
- none

## Set aside
- The single flaky failure of `a_long_graph_scrolls_and_a_tall_panel_keeps_its_borders` on the first whole-crate run. The candidate did not touch that test (only added `Pty::open_size` beside `Pty::open`, which still opens 80x24), it is a >10 s pty/timing test, and it passed alone and on the full re-run — load, not this task. Not a failed fact.
- T-360's comments in `golden_contract.rs` (`the_to_handle_block_sits_at_its_anchor`) still say the "header is still short (T-356)" and the block is "one row higher until T-356". They are now stale (the difference is zero), but they came in with the T-360 merge and the rework note said keep both sides and change nothing else — outside this task's own commits.
- Narrow terminals (width below ~45) cut the bar's trailing rule, and width < 4 collapses the corner rules via `saturating_sub`. Same edge-cut convention as the canvas and the prototype; outside normal use (80/160 columns). Logged, not blocking.
- The contract's phrase "the range includes the separator after the label": with `(x, x+len(label))` a click on a separator between two labels lands in the right-hand tab's range, exactly as the prototype's `TABX` does — no misreading.

## Questions for the captain
- none