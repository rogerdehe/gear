# Review of T-368

Status: running
Verdict: pass
Reviewed: review/t368 @ 1b4d6193   Reviewer: gear-review-11 (pi)   Round: 2 · Contract: v6

## Mechanical acceptance
- build: `cargo build --release -p gear-cli` → clean (recompiled all workspace crates; no compiler warnings/errors, verified with a forced recompile of gear-cli); `target/release/gear --version` prints exactly `gear 0.5.2`.
- tests: `make check CRATE=gear-cli` → 175 passed, 0 failed, 0 skipped (7.7 s); no compiler warnings/errors.
- allowed files: ok — Cargo.toml, Cargo.lock, herdr-plugin.toml, README.md, docs/releases/v0.5.2.md, plus the packet's mandated gear/voyages/M5/reports/T-368.md workflow artifact.
- merges into dev: clean — `git merge-tree --write-tree origin/dev 1b4d6193` exits 0 (tree 218f4786); the merged Cargo.lock has all nine `gear-*` entries at 0.5.2 and no 0.5.1 among them.
- version fields: Cargo.toml `[workspace.package] version` = 0.5.2; Cargo.lock = nine `gear-*` entries at 0.5.2; herdr-plugin.toml `version` = 0.5.2; README status line, install `--ref` and `gear --version` check all = 0.5.2.

## Spec compliance
- ok — attempt 1's F1 is resolved: T-407 has its bullet under "Also released here, as v0.5.1's notes promised" (after T-420), and the deferral line no longer names it.
- ok — `docs/releases/v0.5.2.md` lists every task merged on origin/dev since v0.5.1 at this hand-in. `git log v0.5.1..origin/dev --grep='^merge(T-'` yields 33 unique T-numbers (331 333 335 350-360 except 361, 362-365, 380, 386, 407, 420-424, 426-432 minus 425); the note's bullets carry exactly those 33 and the sorted sets diff empty.
- ok — the deferral line "M5 tasks not merged when this note was written, so v0.5.3's notes: T-425; T-332 and T-396 are cancelled." is complete: the plan's tasks not merged anywhere on origin/dev are T-332, T-368, T-369, T-396, T-425; T-368 is this release and T-369 is v0.5.3's own release, so neither belongs in the line; T-332 and T-396 are cancelled per the plan's 05:43 and 12:00 decisions.
- ok — the "Skill" section carries the captain's release review text verbatim (T-424 and T-430).

## Findings
- none

## Set aside
- The candidate branch's merge base with origin/dev is 307342f8 (12:47), behind origin/dev's head d7fd32c6 (13:07). The rework told the writer not to merge origin/dev again, and the candidate touches only release files; the three-way merge into dev is clean and the merged Cargo.lock keeps all nine workspace bumps. No finding.
- T-365 is folded in despite the rework's "fold in nothing else": the rework note wrote that when T-365 merged after attempt 1's candidate, but at attempt 2's hand-in T-365 is merged since v0.5.1, so the contract's acceptance (list every such task) governs; the writer says it asked the captain (M-6306) and went with the contract. The note and `git log v0.5.1..origin/dev` agree, so no finding.
- T-361 merged on origin/dev at 06:21:40, before the v0.5.1 tag (08:00), so it is outside "since v0.5.1" and correctly absent. Its absence from v0.5.1's notes is a v0.5.1 defect, not this task's.

## Questions for the captain
- none