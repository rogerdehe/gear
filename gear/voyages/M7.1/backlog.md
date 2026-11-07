# Backlog

M7.1 goal (agreed with the user, 2026-11-07): clear the backlog — every reasonable item done, what is not GEAR's job dropped — and ship v0.7.0.

Every item of gear/voyages/M7/backlog.md was checked against the code on 2026-11-07: the fixed ones are gone, the dropped ones are recorded in plan.md's scope decisions, and the rest are M7.1's queue. New items found during M7.1 go below.

## Candidates
- **The captain's home page prints the voyage's done sentence three times** (M7.1, 2026-11-07): the voyage line, the goal line and the snapshot's voyage line each carry the full `done:` text, so a long goal fills the screen three times over. The page says it once (the goal line) and the other two name the voyage and its clock only.
- **The parallel-change check counts files a branch merged in from dev** (M7.1, 2026-11-07, M-8738/M-8739): hull/t658 and hull/t659 had merged hull/t657 (which had merged dev with mast's T-671), so a diff from a stale merge base (git warned "multiple merge bases") listed crates/gear-app/src/plan.rs, which only mast's commits touch. The check should compare the branch's own non-merge commits (or diff against the current integration tip's merge base), so a file another task already landed on dev is not reported as shared.
- **A lane restart request shares its key with the restart plan** (M7.1 T-665, 2026-11-07): `- restart <lane>` writes `engine.restart.<seat>` = `{"at":…}`, which is also restart.rs's plan key, so a request applied while a switch plan of that lane is pending overwrites the plan mid-switch. Give the request its own key (e.g. `engine.restart-request.<seat>`) or refuse the restart while a plan is pending.
