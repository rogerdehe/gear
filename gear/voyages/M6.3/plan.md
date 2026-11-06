<!-- gear-plan project=gear base=1236 -->
# Goal
The fleet runs without a hand to steady it: switching a seat's agent works one way for every seat, the engine tells the captain when it fails, the merge gate keeps its headroom, and the friction that stopped lanes and client fleets on 2026-11-05 and 06 is gone. Each fix is released and live for every project on this machine.

# Milestone M6.3: a fleet that needs no steadying hand
done: (1) a seat's agent switches one way: the same CLI with another model switches in place with `/model` for pi and Claude, and another CLI starts in a new pane on the same worktree and branch, takes the old agent's handover, and gets the seat, the old pane closing only after that; a failed start leaves the old agent in charge; (2) a project tick that fails for a minute opens one urgent captain item; (3) `make gate` keeps half its limit under a busy machine, and T-572's merge-red item is on dev; (4) the lane frictions of 2026-11-05 are gone: a nudge closes itself, a lane is never told a captain-only command, a lane busy with no attempt is freed, a dropped or stale `carried-by` never hides a merge, a respawned reviewer's first call names its new pane; (5) a GitHub issue closes only when its last task lands; a landed voyage's clean worktrees are removed by GEAR and records written after the landing reach the integration branch with no gate; (6) each fix is released as v0.6.x, live for every project on this machine, and each captain told
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 6h
depart: now

# Decisions
- station M-8316: move | why: projected 452m against 360m, hull 345m and mast 123m; finished.rs, quiet.rs and lane_nudge.rs move to mast with T-610, T-618 and T-619 (100m), so hull keeps the files of F-3, T-630, merge and review; the eta stays and the rest is reported to the user
- 2026-11-06 09:55 confirmed: lanes hull and mast owns, gear-engine whole -> hull all but finished.rs, quiet.rs, lane_nudge.rs, which go to mast | why: the user (2026-11-06, through Lawrence's captain, see T-623): a lane's paths are the captain's work, not staffing
- 2026-11-06 09:45 note: the GEAR captain told Lawrence's captain to release its idle DashScope reviewers so they restart; release ends a seat and GEAR starts reviewers only on demand, so two stayed down against the user's resident reviewers | why: logged as the cause of T-630; a reviewer is never released to restart it
- station M-8298: keep course | why: projected 384m is 24m over the 6h eta; T-625 is cancelled (the captain fixed it by hand in v0.6.90: the window comes from the CLI, nothing unknown compacts), which takes 25m off hull, and hull's T-604 boundary is decided; moving engine files between lanes would cost more than it saves
- 2026-11-06 09:26 hand-fix: (v0.6.90, gate 63 s) a lane is compacted only at half a window GEAR knows: the config's, else the CLI's own (pi models.json, Codex rollout, OpenCode config, Claude [1m]); an unknown window or context keeps the session | why: the user: compacting at 9 % is never right, and it had to be fixed at once, not queued
- station M-8289: keep course | why: projected 321m against the 6h eta; the lanes are even now (hull 255m, mast 209m)
- station M-8284: keep course | why: projected 299m against the 6h eta; hull holds 239m of engine work and mast 65m: when mast's queue runs out, the engine tasks that touch no hull file in flight move to mast
- 2026-11-06 07:58 confirmed: M6.3 departure, not departed -> departs now with the scope above (every proposed item but the graceful engine restart), eta 6h | why: the user agreed the scope after being told the plan would be written and the voyage would depart on that agreement
- 2026-11-06 user: M6.3 takes every item proposed for it except the graceful engine restart, which stays in M7
- 2026-11-06 scope: F-3 (one way to switch a seat's agent) is GEAR's third feature: its four members merge into gear/feature/f-3 and reach dev once | why: the user's design; the switch touches herdr, the restart state machine and the in-place switch together
- 2026-11-06 note: Codex keeps the restart for a same-CLI model change this voyage: its `/model` opens a picker with no argument form GEAR can drive yet | why: pi's typed `/model <id>` is proven by another project's captain; Claude's `/model <id>` takes the id directly

# Queue
## T-603 [must] lane=mast milestone=M6.3 required
feature: F-3
goal: [F-3 1/4] Herdr can split a pane for GEAR. crates/gear-herdr/src/lib.rs: add `fn pane_split(&mut self, pane_id: &str, direction: &str, cwd: &str) -> Outcome<String>` to `HerdrAct` (the new pane's id), implemented for the real Herdr client with `herdr pane split <pane> --direction <right|down> --cwd <cwd> --no-focus` (parse `.result.pane.pane_id`) and for the fake in crates/gear-herdr/src/fake.rs (a new pane that holds a shell, recorded in the fake's calls). No caller yet.
accept: Tests in crates/gear-herdr: the real client's reply parsing returns the new pane id; the fake creates a pane with no agent. Run only `make check CRATE=gear-herdr`; the merge gate runs `make gate`.
estimate: 20
deps: -

## T-604 [must] lane=hull milestone=M6.3 required
feature: F-3
goal: [F-3 2/4] Another CLI starts in a new pane while the old agent keeps the seat. crates/gear-engine/src/restart.rs: when the next CLI differs from the current one, the order becomes: (1) Stop as today (the outgoing agent wraps up, commits and writes parts 2 and 3 of the handover document, the engine fills part 1; `switch_at_once` and the grace stay), but the old agent is NOT exited; (2) a new phase Split: `herdr.pane_split(<old pane>, "right", <the seat's worktree>)` (T-603), the new pane id kept in the plan meta before anything else (an engine restart finds it there and never splits twice); (3) Start the next CLI in the new pane (today's start, against the plan's new pane instead of the seat's pane); a start that fails or is not up within today's wait closes the new pane, clears the plan and flags the seat, and the old agent stays bound and working. The phase after a successful start is Finish, written by the next task; in this task Finish only records that the new agent is up. A same-CLI change keeps today's path.
accept: Tests in restart.rs `mod tests` with the fake Herdr: a CLI change waits for the wrap-up, splits once and starts the new CLI in the new pane while the old agent is still there; an engine restart between Split and Start neither splits again nor starts a second agent; a failed start closes the new pane and leaves the old binding active. Run only `make check CRATE=gear-engine TESTS='test(restart::)'`; the merge gate runs `make gate`.
estimate: 45
deps: T-603

## T-630 [now] lane=hull milestone=M6.3 required
fresh: reviewer seats, unrelated to F-3
goal: Reviewers stay at their quota, and a reviewer restarts without losing its seat (Lawrence M2, 2026-11-06 09:37Z: to pick up a fixed DashScope account, the captain released its three idle reviewers on the GEAR captain's word; GEAR starts a reviewer only when a review is due, so two stayed down and the user saw one reviewer where three are resident by default). crates/gear-engine/src/review.rs and release.rs: (1) while the project's current voyage has open tasks and is not landed, the engine keeps `[quota] temps` reviewer seats up: when fewer seats occupy the quota than it allows and no review waits for a seat, it starts one reviewer a tick in `Standby` (its seat worktree on its seat branch, no job; `start_reviewer`'s pane, env and agent-start steps without the job's branch and packet), so the next review finds a warm seat; (2) `- restart review-n` under `# Staff` (crates/gear-app draft and plan, with the engine step in release.rs): an idle reviewer's agent is exited and started again in the same pane and worktree, seat and generation kept (a working one restarts right after its verdict); `- release` keeps its meaning. The skill's staff list in skills/gear/references/course.md names `- restart`.
accept: Tests in review.rs and release.rs `mod tests` with the fake Herdr: a voyage with work and two of three seats up starts the third in Standby, one a tick; a landed voyage starts none; `- restart review-1` on an idle reviewer exits and starts its agent in the same pane with the seat still Standby, and on a working one waits for the verdict. Run only `make check CRATE=gear-engine TESTS='test(review::) | test(release::)'`; the merge gate runs `make gate`.
estimate: 60
deps: -

## T-605 [must] lane=hull milestone=M6.3 required
feature: F-3
goal: [F-3 3/4] The new pane takes the seat, then the old pane closes. crates/gear-engine/src/restart.rs, the Finish phase after T-604's start: the seat gets a new generation bound to the new pane (old bindings void, as crates/gear-app/src/captain.rs `rebind` does), the seat's agent name moves to the new pane (clear it on the old pane first: Herdr names are unique), the Continue packet with the handover block is delivered there; then the old agent exits (today's exit keys) and the old pane is closed, and the plan is cleared. While a plan is between Split and the old pane's close, the old pane is not counted against the lane or temp quota and is not closed as a stray by the guardrails (crates/gear-engine/src/guardrails.rs and the live-pane quota count in review.rs and start.rs): it belongs to the seat's own switch.
accept: Tests in restart.rs `mod tests` with the fake Herdr: after the new agent is up the seat is bound to the new pane at a new generation, the name is on the new pane, the Continue is delivered there and the old pane is closed last; an engine restart in Finish does not deliver twice; the quota and the stray-pane check do not count the old pane during the switch. Run only `make check CRATE=gear-engine TESTS='test(restart::) | test(guardrails::)'`; the merge gate runs `make gate`.
estimate: 45
deps: T-604

## T-606 [must] lane=hull milestone=M6.3 required
feature: F-3
goal: [F-3 4/4] The same CLI with another model switches in place for pi and Claude. crates/gear-engine/src/switch.rs: `switches_in_place` covers "pi" and "claude" besides "opencode". For pi and claude, `switch_model` waits for an idle agent (as for OpenCode), types `/model <model id>` in one go and Enter (`pane_type` then the Enter key; the id is the chain entry's model, for pi `<provider>/<model>`), and confirms by the agent's status line/footer showing the model's bar label within WAIT_TICKS; no confirmation twice is `Failed`, and the caller falls back to today's restart. OpenCode keeps its session API path. Codex keeps the restart (decision above). Update the module comment and the test that lists kinds with no in-place switch.
accept: Tests in switch.rs and restart.rs `mod tests`: a pi seat and a claude seat get `/model <id>` typed once and Done when the footer shows the label; a footer that never shows it fails after two tries and the restart takes over; a codex seat still restarts. Run only `make check CRATE=gear-engine TESTS='test(switch::) | test(restart::)'`; the merge gate runs `make gate`.
estimate: 45
deps: -

## T-607 [must] lane=hull milestone=M6.3 required
goal: A project tick that fails for a minute rings the captain (2026-11-05: GEAR's engine logged `project_failed` every 4 s for 40 minutes and no item opened). crates/gear-engine/src/lib.rs, where a project's tick error is pushed to `report.project_failed`: count consecutive failed ticks per project in meta (`engine.project_failed.<project>`: count and first time); at 15 in a row (or 60 s, whichever is later) open one urgent System item to the project's captain on thread `engine-failed:<project>` with the error code and detail and how long it has failed; update its body while it goes on; resolve it on the first good tick and reset the count. If the store write itself fails, log it and go on.
accept: Tests in crates/gear-engine (lib.rs or a new module's `mod tests`): 14 failed ticks open nothing, the 15th opens one urgent item naming the error, more failures keep one item, a good tick resolves it. Run only `make check CRATE=gear-engine TESTS='test(project_failed) | test(engine_failed)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-608 [must] lane=mast milestone=M6.3 required
goal: `make gate` keeps half its 80 s limit on a busy machine (2026-11-05: T-572 and T-596 failed only on wall time, 81 s with a 67 s suite, while lanes built beside it; a quiet run takes 30-48 s). Run `cargo nextest run --workspace` with its timing output twice while another build runs (`cargo build --release` in a second worktree), list every test over 2 s, and for each: make it fast (fake clock, no real sleeps, smaller fixture) when it is in mast's crates, or quarantine it in the existing quarantine list with its measured time when the fix is not a few lines; for tests in hull's crates (gear-engine, gear-model, gear-usage) write each name, time and cause into the hand-in for the captain. Write the before and after suite times into the hand-in.
accept: The suite under the same load is at most 40 s, measured twice, numbers in the hand-in; no test of mast's crates over 2 s. Run `make check` for the crates changed; the merge gate runs `make gate`.
estimate: 45
deps: -

## T-609 [must] lane=hull milestone=M6.3 required
goal: T-572 again, on today's dev (its attempt 2 passed review but never merged; the commit is kept at refs/backup/t572, 6439eea6). Goal as in M6.2: in crates/gear-engine/src/merge.rs `run_batch` and the batch collection, (1) from the first red batch round on keep one urgent item per project on thread `merge-red:<project>` (opened at once, body updated each round, closed when a merge lands): the round's refusal lines as the gate printed them, the members, how many red rounds and minutes, and what the captain can do (send the cause back to a lane, take a member out with a fail verdict or a cancel, or let GEAR split); the halving goes on meanwhile; (2) keep each red round's refusal normalized (no shas, run ids or timings); two consecutive red rounds with different member sets and the same refusal stop the splitting: requeue every member, set the batch line waiting, make the item say "the same refusal with different members: the cause is on <integration>, not in the batch"; the line runs again when a merge op is queued anew. Start from `git cherry-pick` or a merge of refs/backup/t572 onto a fresh branch from dev and fix what today's dev changed (its own test `the_same_refusal_with_different_members_stops_the_splitting` failed Intended vs Unknown on today's dev).
accept: Tests in merge.rs `mod tests`: two red rounds with the same refusal and different members stop the splitting and leave one urgent merge-red item naming the refusal; a red round then a green one closes it; different refusals keep the halving. Run only `make check CRATE=gear-engine TESTS='test(merge::)'`; the merge gate runs `make gate`.
estimate: 40
deps: -

## T-610 [must] lane=mast milestone=M6.3 required
goal: A nudge to a lane closes itself (M-8211: hull got "No push to … for 20 min" after it had handed T-601 in; a lane may not resolve it and the captain may not either, so it stayed open). crates/gear-engine/src/quiet.rs (the no-push nudge) and crates/gear-engine/src/lane_nudge.rs: each tick, resolve an open engine nudge to a lane when the attempt it names has been handed in, has a newer attempt, or its task is terminal, with the reason as the result. Same for the "tell the captain why" follow-up item on the captain's side when the lane's hand-in answered it.
accept: Tests in quiet.rs and lane_nudge.rs `mod tests`: a no-push nudge closes on the hand-in of its attempt and on a cancel of its task; a nudge for a still-open attempt stays. Run only `make check CRATE=gear-engine TESTS='test(quiet::) | test(lane_nudge::)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-611 [must] lane=mast milestone=M6.3 required
goal: A lane's page never names a command its role may not run (M-8226: hull was shown `next: gear reply ... --resolve` for a captain reply and an engine nudge; both FORBIDDEN for a lane). crates/gear-app/src/home.rs and mail.rs: the `next:` line and every item hint shown to a lane or reviewer name only commands that role may run (check against the role checks the commands use); an Info item a lane has been shown is marked seen and is not shown again on its next `gear`; an item a lane cannot resolve says "the captain or GEAR closes this" instead of a command.
accept: Tests in crates/gear-app/tests/it: a lane's home page with a captain reply and an engine nudge shows no `gear reply` command; a second `gear` does not show the same Info item; the captain's page is unchanged. Run only `make check CRATE=gear-app TESTS='test(home) | test(mail)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-612 [must] lane=hull milestone=M6.3 required
goal: A lane busy with no open attempt while its next task waits is freed (Prometheus T-778: a lane looped inside one turn retrying `gear send --task T-778 --attempt 1` for a task not yet delivered, so it never went idle and the urgent delivery waited; one Esc fixed it). crates/gear-engine (dispatch.rs or a new step beside lane_nudge.rs): a lane seat that is working with no open attempt for 3 minutes while a task is ready for it (delivery intended) gets one Esc (`send_keys` "esc") and the delivery goes on at its next idle; if it is still working with no attempt 3 minutes later, one Info item tells the captain. Never for a lane with an open attempt.
accept: Tests in the module's `mod tests` with the fake Herdr: a working lane with no attempt and a ready task gets one Esc after 3 minutes and the item after 6; a lane with an open attempt gets nothing. Run only `make check CRATE=gear-engine TESTS='test(dispatch::)'` (or the new module's filter); the merge gate runs `make gate`.
estimate: 30
deps: -

## T-613 [should] lane=mast milestone=M6.3
goal: A `gear send` for a task the lane has not been given says what to do (same case, T-778). crates/gear-app/src/mail.rs: a lane's `gear send --task T-n` for a task that is ready for it but has no attempt yet fails with NO_TARGET and the text "T-n is not delivered yet: end your turn; GEAR delivers it when you are idle", instead of the bare NO_TARGET.
accept: A test in crates/gear-app/tests/it: the send for an undelivered ready task fails with that text. Run only `make check CRATE=gear-app TESTS='test(mail)'`; the merge gate runs `make gate`.
estimate: 15
deps: -

## T-614 [must] lane=mast milestone=M6.3 required
goal: A `carried-by:` line never hides a merge (Prometheus T-779: v1394 set `carried-by: T-752`, v1395 dropped the line, the mark stayed, and T-752 had merged before T-779 started; T-779 sat accepted and unmerged 20 minutes). crates/gear-app/src/plan.rs: (1) plan apply refuses `carried-by: T-m` when T-m has already merged ("T-m has merged: its merge cannot carry T-n's work; drop the line"); (2) a block of an existing task that no longer has a `carried-by:` line clears the mark (as `carried-by: -` does), and the apply output says so.
accept: Tests in crates/gear-app/tests/it: carried-by naming a merged task is refused; dropping the line clears the mark and the task's accepted work queues its merge. Run only `make check CRATE=gear-app TESTS='test(carried)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-615 [should] lane=hull milestone=M6.3
goal: The lost-work scan names a carried task whose carrier landed without its work (same case). crates/gear-engine (the lost-merge / done-branch scan): a task marked carried by T-m whose T-m has merged a candidate that does not contain the carried task's accepted candidate (`git merge-base --is-ancestor`) gets the existing merge-lost captain item, naming the carrier and "`carried-by: -` and `- remerge T-n` land it".
accept: A test in the scan's `mod tests`: a carried task whose carrier merged without its candidate gets one merge-lost item; one whose candidate is inside the carrier's gets none. Run only `make check CRATE=gear-engine TESTS='test(lost)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-616 [should] lane=mast milestone=M6.3
goal: A respawned seat's first GEAR call names its new pane (Prometheus M-11063: review-1 was rebound to w26P:p1 at generation 976; 11 s later a call from w26P claimed w26J:p1, two generations back and gone, and was refused once). Find where the old pane comes from in crates/gear-app/src/identity.rs (an inherited HERDR_PANE_ID in a process carried across the respawn, or a `--pane` given from old text in the resumed conversation) and fix it at that source: a call whose claimed pane is a void binding of the same seat while the caller runs in the seat's active pane resolves to the active binding; the refusal for a truly foreign pane stays.
accept: Tests in crates/gear-app/tests/it: a call claiming the seat's void older pane from inside its active pane succeeds as the active generation; a claim from another seat's pane is still refused. Run only `make check CRATE=gear-app TESTS='test(identity)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-617 [must] lane=mast milestone=M6.3 required
goal: A GitHub issue closes only when its last task lands. crates/gear-forge/src/sync.rs closes the issue after any bound task's landed attempt, and the claim PR body says `Closes #n`, so an issue split into tasks closes on the first merge (with `merge = "pr"` GitHub closes it too). Close the issue only when every task bound to it (same `issue:`) is done or cancelled with at least one done; claim PRs say `Part of #n`, and only the PR of the last open task says `Closes #n`; the landing comment per task stays.
accept: Tests in crates/gear-forge: an issue with two tasks stays open after the first lands and closes after the second; the first PR body says `Part of #n`, the last `Closes #n`. Run only `make check CRATE=gear-forge`; the merge gate runs `make gate`.
estimate: 35
deps: -

## T-618 [must] lane=mast milestone=M6.3 required
goal: A landed voyage's worktrees are removed by GEAR (Prometheus had 16 worktrees in its sidebar, 6 leftovers; Carthage M1's landing named 17 things to remove by hand). crates/gear-engine/src/finished.rs `reclaim`: for each worktree it names, remove it itself (`git worktree remove`) when it is clean (`git status --porcelain` empty) and its branch has nothing the remote does not have; delete a remote branch of the project whose tip is on the integration branch; close a Herdr workspace of the project whose panes hold no agent. Name to the captain only what it could not remove (dirty, unpushed, holding an agent), with the reason per entry. Never touch the main checkout or a worktree of another project.
accept: Tests in finished.rs `mod tests` with a temp repo: a clean pushed worktree is removed and not named; a dirty one is named with "uncommitted changes" and kept; a branch on the integration branch is deleted on the remote, one with extra commits is named. Run only `make check CRATE=gear-engine TESTS='test(finished::)'`; the merge gate runs `make gate`.
estimate: 50
deps: -

## T-619 [must] lane=mast milestone=M6.3 required
goal: Records written after a landing reach the integration branch with no gate (Carthage M1: the plan file rewritten by an apply after the landing went through a full 6-minute CI run, because the records step runs once per landing). crates/gear-engine/src/finished.rs `merge_records`: while the landed voyage is within the reclaim window, a seat branch `{seat_branch}` of the landed voyage that appears again on the remote with commits the integration branch lacks is merged the same way (records merge, no gate, branch deleted); the once-per-landing meta becomes the tip it merged, so a new tip merges again.
accept: Tests in finished.rs `mod tests`: after the first records merge, a new commit pushed to the captain's branch is merged on a later scan and the branch deleted; no new commit, nothing happens. Run only `make check CRATE=gear-engine TESTS='test(finished::)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-620 [should] lane=mast milestone=M6.3
goal: The captain's skill says how records travel after a landing (with the records task above). skills/gear/references/captain.md, in the landing part: a change that only touches the voyage's folder (plan, report, lessons) after the landing is committed and pushed to the captain's seat branch, and GEAR merges it with no gate; never run the project's full gate for it. One or two lines, nothing else.
accept: The two lines are in captain.md and `make check` passes. Run only `make check CRATE=gear-app TESTS='test(skill)'`; the merge gate runs `make gate`.
estimate: 10
deps: -

## T-622 [must] lane=hull milestone=M6.3 required
goal: A project's integration branch lives for one voyage: a landing's reclaim deletes it once it is on the release branch, and the next departure cuts it again (Carthage M1: carthage/dev was removed at the reclaim, and nothing would recreate it: every lane, review and merge worktree is created from `origin/<integration>`). crates/gear-engine: before the first worktree of a voyage is created (onboard.rs, review.rs and merge.rs call `worktree_create(..., &repo.integration, ...)`), when `refs/remotes/<remote>/<integration>` is missing after the fetch, push `<remote>/<release>` to `refs/heads/<integration>` once, fetch again and tell the captain in one Info item ("<integration> was missing; cut from <release> at <sha>"). An integration branch equal to the release branch is never created or deleted. In crates/gear-engine/src/finished.rs `reclaim`, the integration branch is named only when its tip is on the release branch (otherwise it holds unreleased work: it is named with "not on <release>" and kept).
accept: Tests with a temp repo: a missing integration branch is cut from release before the first lane worktree and the item names the sha; a present one is untouched; the reclaim names an integration branch whose tip is on release and keeps one with extra commits. Run only `make check CRATE=gear-engine TESTS='test(onboard::) | test(finished::)'`; the merge gate runs `make gate`.
estimate: 45
deps: -

## T-623 [must] lane=mast milestone=M6.3 required
goal: A lane's paths and words are the captain's, not staffing (Lawrence M2, 2026-11-06: adding 15 files to a lane's `owns` and fixing a command in a charter were refused with STAFF_LOCKED until the user confirmed; the user: that is the captain's work). crates/gear-app/src/admin.rs `staffing_changed`: compare `[quota]`, `[models]`, the set of lanes with each lane's name, kind, profile and models, and `[temps]` kind and profile; a change only to a lane's `duty`, `owns`, `not` or `charter`, or to `[temps].charter`, is not a staffing change and applies without `--confirmed`. `staffing_changes` keeps listing every difference, and the apply output prints those non-staffing lines under "changed (captain's)". Today's ownership checks (overlap between lanes, open tasks' paths) still run on the new owns.
accept: Tests in crates/gear-cli/tests/it/admin_init_contract.rs: mid-voyage, a change of a lane's owns and charter applies without --confirmed and is printed; a change of a lane's model or of quota.lanes is still STAFF_LOCKED. Run only `make check CRATE=gear-cli TESTS='test(admin_init)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-624 [must] lane=mast milestone=M6.3 required
goal: A feature's spread deps are kept apart from a member's own (Lawrence M2, 2026-11-06: T-86's block said `deps: -` and the apply kept T-63, T-79 and T-80; they were F-4's outside wait, written by T-587 into every member's stored deps, so every later apply read them back as the member's own and the captain could neither release a member nor cancel a task the feature waits on). crates/gear-app/src/plan.rs, rule (3) of the feature deps: record the spread per feature in meta (`feature_wait:{project}:{feature}`, the outside deps and the member that declares each); at the start of each apply, strip the previously recorded spread from every member's stored deps before the graph is read, then compute the spread again from the members' own deps and store it again; the apply prints one line per member that waits on work it did not name: `T-86 waits on T-63 (F-4: T-89 needs it)`. `gear plan show T-n` prints the spread on its own line (`waits (feature F-4): T-63, T-79, T-80`), not in `deps:`.
accept: Tests in crates/gear-app/tests/it/feature_plan_contract.rs: a member whose block drops its outside dep releases every other member on the next apply; a member's own `deps: -` is kept while another member still declares the outside dep, and the output names that member; `plan show` keeps the spread out of `deps:`. Run only `make check CRATE=gear-app TESTS='test(feature)'`; the merge gate runs `make gate`.
estimate: 40
deps: -

## T-625 [must] lane=hull milestone=M6.3 required
state: cancelled
goal: A model with no `context_window_k` is not compacted before every packet (Lawrence M2, 2026-11-06: web was compacted at 9.6 % of 1M and console at 22 % of 500k, because crates/gear-engine/src/session.rs `context_large` returns true when the entry has no window). With no window, read it from the seat CLI's own model table when the profile has one (pi: the `contextWindow` of the model in pi's models file), else use a default of 200k; nothing unknown is ever "large" by itself: a window that cannot be read and a context the usage log cannot read (today `usage.is_none_or(...)` returns large) both mean keep the session; only a measured context at half the window compacts. Keep today's rule when both are known. The user (2026-11-06): a lane continuing related work keeps its session, an unrelated task starts a new one, and a compaction at 9 % of the window is never right.
accept: Tests in session.rs `mod tests`: an entry with no window and a 30k context does not compact; a set window with an unreadable usage log does not compact; with no window and 120k it compacts (half of the 200k default); a set window keeps today's rule. Run only `make check CRATE=gear-engine TESTS='test(session::)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-626 [should] lane=mast milestone=M6.3
goal: `gear admin init` warns about a model with no `context_window_k` (same case). crates/gear-app/src/admin.rs (or kickoff.rs, where init prints the staffing): for every lane, review and captain model entry without `context_window_k`, print one warning line `model <id> has no context_window_k: GEAR assumes 200k (or the CLI's own table); set it to the model's window`. No refusal.
accept: A test in crates/gear-cli/tests/it/admin_init_contract.rs: a config with a model lacking the window prints the warning and applies. Run only `make check CRATE=gear-cli TESTS='test(admin_init)'`; the merge gate runs `make gate`.
estimate: 15
deps: -

## T-627 [must] lane=hull milestone=M6.3 required
goal: A feature member's merge never waits behind a full gate (Lawrence M2, 2026-11-06 09:05: T-86 and T-87, members of F-4 merging into lawrence/feature/f-4 with a conflict check only, went Executing → Unknown "merge will retry from the remote" every tick for minutes, because they take the merge lock of the same merge worktree that T-79's 6-minute gate into lawrence/dev held). crates/gear-engine/src/merge.rs: a merge whose target is not the repo's integration branch (a feature member, `task_branch_override`/`base` naming the feature branch) runs in its own merge worktree per target branch (`<project>-merge-<feature>` through herdr, as the integration one is made) with its own lock, so it runs beside the integration train; the worktree is removed when the feature's integration task lands.
accept: Tests in merge.rs `mod tests`: while an integration merge holds its worktree's lock, a feature member's merge runs and lands in the feature branch on the same tick; two members of one feature still take turns. Run only `make check CRATE=gear-engine TESTS='test(merge::) | test(feature::)'`; the merge gate runs `make gate`.
estimate: 40
deps: -

## T-628 [should] lane=hull milestone=M6.3
goal: A merge-failed item says when its remerge is queued (Lawrence, 2026-11-06: M-921 merge-failed:T-63 kept `next: remerge T-63` while O-760, the remerge, was Intended behind the running train; the captain's second remerge was refused and it asked why). crates/gear-engine/src/merge.rs: when a remerge op of the task is Intended or Unknown, the open merge-failed item's body ends with "remerge queued (O-n): it runs when the merge running into <branch> ends", and the hint offers no remerge; when the remerge starts, the item says it is running.
accept: A test in merge.rs `mod tests`: a failed merge then a queued remerge updates the item's body with the queued line; the line is gone once the op runs. Run only `make check CRATE=gear-engine TESTS='test(merge::)'`; the merge gate runs `make gate`.
estimate: 20
deps: -

## T-629 [must] lane=hull milestone=M6.3 required
goal: A provider's content refusal is seen (Lawrence M2, 2026-11-06 09:26-09:31Z: DashScope's content review cut the web lane's output four times, "Output data may contain inappropriate content", because the zh-CN copy named a channel stratum; the lane went idle after each and 12 tasks waited; GEAR noticed nothing). crates/gear-engine (where an idle lane with an open attempt is read, beside lane_nudge.rs or quota.rs): when the seat's session log's last assistant record ended with an error whose text is a content refusal (pi: `stopReason` "error" and an `errorMessage` matching the refusal markers of the profile; add `refusal_markers` to the pi and opencode profiles with "inappropriate content", "content_filter", "data_inspection_failed"), and that happened twice in a row on one attempt, the engine moves the lane to the next model of its chain the way a quota stop does (restart path, the task continues) and opens one captain item naming the provider, the attempt and the marker; with no next model, the item says so and the lane is held.
accept: Tests with a pi session fixture: two refusals in a row on one attempt switch the lane to the chain's next model and open one item; one refusal followed by a normal turn does nothing; no next model opens the item and holds the lane. Run only `make check CRATE=gear-engine TESTS='test(refusal)'`; the merge gate runs `make gate`.
estimate: 45
deps: -
