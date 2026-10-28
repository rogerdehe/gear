# M3.1 backlog — trial feedback, batch 2 (2026-10-26)

A small fix voyage after v0.3.0. Taken: what breaks a rework loop, stops reviews, or blinds the captain — fixed even when it costs. The rest is in m4-backlog.md; what a tool cannot solve is listed as excluded. The feedback stays with the user; only the problem shapes are kept here.

## Taken (M3.1)

1. **A rework packet leads with this round's rework (B02-06).** The task packet lists every review report and says nothing about the current rework note, so a lane read the first round's report, found nothing to fix and handed in the same candidate again. Fix: the packet opens with `## Rework (attempt n, from review of attempt n-1)` and the captain's rework text; older reviews are marked history. `gear send --kind result` refuses a candidate equal to a failed attempt's candidate with a clear reason.
2. **A dead temp seat frees its slot (B02-04).** A reviewer seat left unknown/void still counted, so `temps 4/2` showed while no reviewer ran. A temp seat that is not running a reviewer never counts toward the quota and is released; the temp count never exceeds the quota. A detached worktree answer without a branch field is handled, not "worktree reply is incomplete". Check live that T-69 (holder detach, stop after three tries, one item) ends the branch-held loop.
3. **`gear` knows the caller without env vars (B02-05, and the Codex env-var friction).** A captain whose shell lost GEAR_CTX and kept a stale pane id got NOT_REGISTERED for dozens of turns while doorbells kept coming. With T-104's real-pane proof, the seat comes from the pane the caller runs in and the store from the default path or the Herdr session's registered target; GEAR_CTX/GEAR_DB stay only as overrides. `gear whoami` prints the real pane, the seat bound to it and why a claim fails. (Moved from M4.)
4. **A reply never rings its own sender (B02-03).** The captain answering an item it raised itself got a reply addressed to itself and a fresh doorbell with no new fact. A reply to oneself is stored, never delivered.
5. **Turned-off idle warnings are visible (B02-08, tool part).** `- tail:` turns off the dry/idle warnings of those lanes until cleared; a captain put every lane in the tail during a dependency wait and forgot. `gear plan apply` prints which lanes' warnings are off; the home page shows it; one item when every lane is in the tail while required tasks remain.
6. **`gear admin stop` waits for in-flight merges (M3, T-110 review).** An upgrade during a voyage must not cut a merge; today the guide asks the user to read the engine log for every passed task. `gear admin stop` refuses or waits while any merge op is intended/executing, and says which.
7. **A used `# Staff` line goes away, and a failed merge's hint names the remerge (M3 acceptance).** After an apply, a `- remerge T-n` line stays in the file and fails the next apply once the op is no longer failed; the apply should drop it like `# Verdicts`. The failed-merge item says `- remerge` while its `next:` line suggests a fail verdict.
8. **Re-check B02-02 live.** A reused reviewer got earlier jobs' notices before its packet; M3's T-71 and T-103 should end it — prove it in the M3.1 trial, fix if not.

0. ~~M3's done clause (1), re-proven~~ — done 2026-10-26: the `fx` fixture voyage landed on 0.3.0+fixes (released as v0.3.1).

- **Acceptance runs on real work where it can (user, 2026-10-26).** A fixture's output is thrown away; only use one for a shape our own work cannot cover (gear is one repo, so the parent-plus-children shape needs `multi`). Otherwise prove a release on gear's own next tasks.

## To M4

- **A lane can move to where the work is (B02-01).** Capacity is bound to a repo through the lane; reusing an idle lane in another repo means a config change whose effect on its earlier tasks' review and merge targets is unclear, and the quota counts configured lanes, retired history included. Wanted: a task keeps its repo, base, candidate and gate; a seat can be rebound; the quota counts active seats; a supported retire/rebind flow with preview and rollback.
- **A tail can end by itself (B02-08).** A tail tied to a dependency or a time, so a temporary wait never silences a lane for good.
- **Resource ownership with acknowledgement (B02-07).** When the captain moves a shared resource (a service, a port) from one lane to another, the instruction needs an acknowledged handover, not just a delivery.

## Excluded (not a tool problem)

- Managing a project's own services and processes (B02-07): GEAR does not own them; it can only carry acknowledged instructions (M4).
- How the trial's feedback was written (B02-09).
- The captain's own conduct listed in the feedback — retrying one failing command for dozens of turns, reporting task counts as delivery, closing an item before its plan applied: the skill states the rules; judgment is the agent's.

## Docs and skill (fixed in M3's closing skill review)

- captain.md said "one lane per repo" — wrong, and the cause of B02-01's even split. Lanes follow the work and the critical path; one repo may have several lanes with disjoint `owns`.
- `- tail:` wording: say it turns off the lanes' idle and dry warnings until `- tail: -`, and is for lanes with no valuable work left, never for a dependency wait (that is `deps:`).
9. **An OpenCode lane's identity is proven, not just claimed (M3, T-104 follow-up).** OpenCode runs a lane's tools in its shared service, outside every pane, so T-104's ancestry proof cannot reach the lane's pane; the hotfix (6d82c7d) lets such a caller keep the env claim for an OpenCode pane. One OpenCode lane can still pose as another. Prove it with a fact the service carries per session (OpenCode's session id for the seat's pane, or Herdr vouching for daemon-spawned descendants).


## Closed at the landing review (2026-10-28)

Every item above, checked against the store, the plans' decisions, the later backlogs and the code. `done` names the merged tasks; `dropped` quotes the decision; `carried` names the later backlog that holds it (closed there in turn). Everything still `open`, and the open rest of a `partly`, is postponed to M5's backlog (Candidates, "postponed from M3.1").

| # | item | proposed status | evidence |
|---|------|-----------------|----------|
| 1 | "A rework packet leads with this round's rework (B02-06)" | done T-118 | T-118 done: packet opens with `## Rework (attempt n…)`, older reviews under `## Earlier reviews (history)`, result with a failed attempt's candidate refused. Voyage report: v0.3.2 "rework packets". |
| 2 | "A dead temp seat frees its slot (B02-04)" | partly: done T-116 covers releasing a reviewer seat that runs no reviewer, temps never above the quota, detached worktree reply with no `branch`; open: "Check live that T-69 … ends the branch-held loop" | T-116 done (contract matches). No record of the live T-69 check in M3.1's plan/reports. T-69 R2 (reuse the holder's seat) went to M4/backlog.md "Cut from M3 at station M-916"; M4 design §3.7 covers it (row A6), T-146 done. |
| 3 | "`gear` knows the caller without env vars (B02-05 …)" | done T-126, T-163 | T-122 cancelled (cut at M-2234 to M4), but T-126 done: no GEAR_CTX needed (seat from the caller's real pane / HERDR_PANE_ID / daemon-kind pane), GEAR_CTX an override, GEAR_DB default path. T-163 done: `gear whoami` subcommand; `gear_app::identity::whoami` prints pane, seat, store and `reason` when the claim fails (crates/gear-app/src/identity.rs:574). M3.1 report: "no-env came back as `gear captain take`". |
| 4 | "A reply never rings its own sender (B02-03)" | done T-119 | T-119 done: a reply to oneself is stored as the item's note, never delivered. |
| 5 | "Turned-off idle warnings are visible (B02-08, tool part)" | partly: done T-117 covers the one item when every lane is tailed while required tasks remain; done T-164 covers `gear plan apply` printing the silenced lanes (crates/gear-cli/src/main.rs:1710-1747); open: "the home page shows it" | crates/gear-app/src/home.rs has no tail/silenced line; no later backlog names it. |
| 6 | "`gear admin stop` waits for in-flight merges (M3, T-110 review)" | carried → gear/voyages/M4.2/backlog.md "## Fixes from real use" (line 21: "Cut from M4 … T-180 admin stop waits for merges (T-121)") | T-121 cancelled (cut at M-2277 to M4, M4/backlog.md "Cut from M3.1 (station M-2277)"); T-180 cancelled (cut at M-2503); M4.2 plan has no task or decision for it; not in M5/backlog.md. |
| 7 | "A used `# Staff` line goes away, and a failed merge's hint names the remerge" | done T-164 | T-120 cancelled (cut M-2277), but T-164 done (M4): "used `# Staff` lines removed after apply, an empty section removed, never a second `# Staff`; a failed merge's `next:` names `- remerge T-n`". |
| 8 | "Re-check B02-02 live" (reused reviewer got earlier jobs' notices) | open | No M3.1 decision, report or task records the live re-check; not in M4/M4.2/M5 backlogs. M3 T-71/T-103 are the fixes it was meant to prove. |
| 0 | "M3's done clause (1), re-proven" | done (no task id) | Struck through in the file: "done 2026-10-26: the `fx` fixture voyage landed on 0.3.0+fixes (released as v0.3.1)"; tag v0.3.1 and docs/releases/v0.3.1.md exist. |
| — | "Acceptance runs on real work where it can (user, 2026-10-26)" | open | A rule, not a task. Followed in practice (M3.1 done (1) proved on gear's own T-119; M4 done "on this project"), but written in no skill, CONVENTIONS or later backlog. |
| — | "A lane can move to where the work is (B02-01)" (To M4) | partly: done T-141 (a new task copies repo/base/gate from its lane), T-174 + T-175 (`- rebind <lane> repo= owns=` waits for open tasks, old tasks keep their repo); open: "the quota counts active seats" and "preview and rollback" | M4/backlog.md "## First item of M4 … (B02-01)". config.rs:927 still checks `cfg.lanes.len() > cfg.quota.lanes` (configured lanes); M4 design line 224 makes only the temps quota count live seats. Rebind prints what it waits for, but there is no preview/rollback step. Not in M4.2/M5 backlogs. |
| — | "A tail can end by itself (B02-08)" (To M4) | done T-164, T-191 | T-164: `- tail: <lanes> until=T-n` or `until=<RFC3339>`; T-191: an ended tail stays ended on replay. Skill captain.md:134 documents `until=`. |
| — | "Resource ownership with acknowledgement (B02-07)" (To M4) | carried → gear/voyages/M5/backlog.md "## For M5" ("Cut from M4: T-181 acknowledged instructions …") | T-181 cancelled (cut at M-2503). |
| — | Excluded: "Managing a project's own services and processes (B02-07)" | dropped | Backlog 2026-10-26 "Excluded (not a tool problem)": "GEAR does not own them; it can only carry acknowledged instructions (M4)". |
| — | Excluded: "How the trial's feedback was written (B02-09)" | dropped | Backlog 2026-10-26 "Excluded (not a tool problem)". |
| — | Excluded: "The captain's own conduct listed in the feedback" | dropped | Backlog 2026-10-26 "Excluded (not a tool problem)": "the skill states the rules; judgment is the agent's". |
| — | Docs: "captain.md said 'one lane per repo' — wrong" | partly: done (no task, commit d504a1ad "Skill review (M3): lanes follow the work …") in skills/gear/references/captain.md:13; open: skills/gear/references/conventions.md:134 still says "One lane per repo." | Both lines checked in the current skill text. |
| — | Docs: "`- tail:` wording" (turns warnings off; never for a dependency wait) | done (no task, commit d504a1ad) | skills/gear/references/course.md:32: "this turns its idle warnings off, so a lane that only waits for a dependency is never in the tail"; captain.md:134 gives the ways it ends. |
| 9 | "An OpenCode lane's identity is proven, not just claimed" | carried → gear/voyages/M5/backlog.md "## For M5" ("OpenCode and Codex daemon identity proof (T-123), moved from M4 by the user") | T-123 cancelled (cut at M-2234); M4/backlog.md "Cut from M3.1 (station M-2234)"; T-126 widened the env-claim exception to Codex instead of proving it. |

Fixed in the skill review after v0.4.9 (2026-10-28): README step 3 now says the captain registers the project (M3 #10); `conventions.md` no longer says "one lane per repo" (M3.1, the docs item). Not carried to M5.
