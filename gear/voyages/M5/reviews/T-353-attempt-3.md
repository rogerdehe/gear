# Review of T-353

Status: done
Verdict: pass
Reviewed: mast/t353 @ 1a13379e   Reviewer: Codex   Round: 3
Contract: v3

## Mechanical acceptance

- Build: `make check CRATE=gear-board` compiled cleanly, zero warnings (55.77s).
- Tests: the same command passed 201 tests, 0 skipped (16.014s), including all eight golden frames, STRICT quality-bar meaning, landed/reverse voyage label, layout-based tab click, and both wide-glyph regressions. The eight T-353 PENDING entries remain removed.
- Allowed files: ok. This attempt's own implementation commit 4deb8ebd changes only frame.rs and crate tests; d7bcbfa5 adjusts the tab-click test to the integrated Log heading. The task report and integration merges are accounted for separately. Against fetched origin/dev, the complete remaining diff comprises frame.rs/lib.rs, crate tests and the T-353 report.
- Merge: clean. `git merge-tree --write-tree origin/dev 1a13379e` exits 0 against origin/dev 355945569e9e6ac989ee32ba59ab844882a5e9e8 (tree 8c4f6fc340eefd30d072a775542fda8fa9ffaa3a). Candidate includes the fixed T-351 link via 08e842a0 and its subsequent integration into dev.
- Candidate code untouched. Independent probes linked against the freshly compiled candidate library; scratch under target/tmp/t353-review was removed before hand-in.

## Spec compliance

Compliant. Reviewed this attempt's delta and verified all earlier findings:

- F1 resolved: continuation cells remain represented on the canvas and are omitted from serialization; the 160-column terminal regression passes.
- F2 resolved: candidate merges cleanly and preserves view_bottom/offset behavior with layout.top and converted mouse coordinates. The fixed T-351 link is included.
- F3 resolved: clear_wide_at is called by put_styled, put_line and put_wide before replacement; replacing a continuation blanks the leading glyph, and replacing the leading glyph clears its continuation. Independently exercised both overwrite directions through all three methods. ANSI-output fixture probes show clock column 35 at width 43, column 72 at width 80 with the review's five appended voyages, and column 152 at width 160; row widths are respectively 42, 79 and 157, all within their canvases. The existing last-column clipping is preserved.

The goal box, quality-bar styles/meaning, voyage labels and dynamic layout retain their passing contract coverage. No missing or extra behavior identified in this delta.

## Findings

None.

## Set aside

- Inherited T-351 and unrelated integrated Cost/Log view changes are outside this task's own delta; their integration seams compile and pass the crate suite.
- Voyage navigation actions, folded header rows, framed tabs and goal-change notices belong to other tasks.
- Long words and quality-bar lines retain the specified clipping behavior.
- The writer reported a prior pty-test timing flake; this independent complete run passed, so there is no reproduced finding.
- No make gate or workspace-wide suite was run; the packet reserves the gate for merge.

## Questions for the captain

None.
