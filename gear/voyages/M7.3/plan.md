<!-- gear-plan project=gear base=1309 -->
# Goal
A merge leaves one commit per gated change and no merge commits: each task lands as one squashed commit on the integration tip, a feature lands by replaying its tasks' commits, and everything in GEAR that asked git "is this merged" still answers right. Shipped as a v0.7.x patch live for every project on this machine.

# Milestone M7.3: squash and rebase, released
done: (1) with `[conventions].merge_method = "squash"` (the default) a task merge in push mode lands one commit on the integration tip: subject `T-n: <goal>`, the lane's commit subjects in the body, trailers `Gear-Task`, `Gear-Attempt`, `Gear-Candidate` and one `Gear-Source` per merged commit; `merge_method = "merge"` keeps today's merge commits; (2) a feature's integration task lands by replaying the feature's own commits onto the integration tip with no merge commit, and falls back to one squashed commit when the replay conflicts; bisect finds a feature's members by their `Gear-Task` trailer; (3) every check that asked "is this commit merged" by ancestry also accepts a commit on the branch whose trailer names it: the merge restart, the lost-merge scan, carried tasks, the coverage report, the plan's merged check, the leftover-branch sweep and the parallel-change check; the release check accepts an integration branch whose content is on the release branch after a squash; (4) the M7.1 leftovers in the same code: the parallel-change check reads the branch's own commits, an `integrated-by` pass whose candidate differs from the integration branch only in voyage files is accepted, a landing whose only unreleased change is its records says so once and not as urgent, and the records merge refuses a seat branch that changes files outside the voyage folder; (5) released as a v0.7.x patch, installed, skills reinstalled, the three engines on it, the captains told
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 5h
depart: now

# Decisions
- 2026-11-17 note: T-708's merge gate failed on gear-engine's upgrade_contract.rs, a home page assertion outside mast's files; a carrier task on T-708's candidate fixes it and T-708 is carried by it | why: M-9044/M-9045; the packet named only home and board tests, not every crate that asserts the page
- station M-9036: keep course | why: 15 done at 112m, pace 0.25x, projected 133m from departure; hull on T-696 then T-693 rework and T-694, mast on its backlog tasks
- station M-9009: keep course | why: 12 done at 97m, pace 0.25x, projected 118m from departure; both lanes busy
- 2026-11-17 note: T-696's packet named a review.rs `on_integration` that does not exist (it is plan.rs's, done in T-697); the item is dropped from T-696. The CLI and skill go back to mast with T-700, T-704 and T-705, since mast ran dry and hull still holds T-693's rework and T-694 | why: M-8988 hull looped 55 min on the wrong name; M-8987 mast idle
- station M-8986: keep course | why: 9 done at 82m, pace 0.26x, projected 109m from departure; both lanes busy
- station M-8978: keep course | why: 75m in, pace 0.22x, projected 102m from departure against the 5h eta; both lanes busy
- station M-8966: keep course | why: 60m in, pace 0.21x, 9 left projected 87m from departure; T-693 and T-698 back in rework, both lanes busy
- 2026-11-17 05:00 confirmed: M7.3 done gains clause (6), estimates by measured model speed, with three tasks for mast and two for hull; the CLI and skill move from mast to hull with T-700 | why: the user asked for it mid-voyage: captains estimate in human hours (an hour for a few dozen lines, days for a small voyage) while this voyage's own pace is 0.14x
- station M-8939: keep course | why: 6 done at 37m, both lanes busy, 5 left (about 4h of estimates over two lanes) inside the 263m left
- 2026-11-17 note: T-698 drops the records-only line (M-8935): since v0.7.11 (T-622) the reclaim keeps an unreleased integration branch and never names it, so done (4)'s records-only clause is met by that: nothing urgent, nothing repeated | why: mast found the item the M7.1 leftover described no longer exists
- station M-8917: keep course | why: 3 done at 18m (T-691 merged, T-690 and T-692 queued to merge), both lanes busy, T-695 in review; well inside the 5h eta
- 2026-11-17 01:40 confirmed: M7.3 departure, not departed -> departs now with the scope above, eta 5h | why: the user ordered the voyage's work and staffing in their own words
- 2026-11-17 note: scope agreed with the user: local squash plus rebase (one commit per gated change, never one per voyage), and the four M7.1 leftovers that sit in the same merge and landing code (parallel check, integrated-by, records-only landing, records merge files) | why: the user's call after the M8 backlog review; M7.3 is a small voyage, patches stay v0.7.x
- 2026-11-17 note: staffing: hull and mast on the self-hosted DeepSeek V4.1 Flash, two reviewers on DeepSeek V4 Pro through opencode-go | why: the user's staffing for M7.3
- 2026-11-17 note: a squash is built from the merge GEAR already does: merge as today on the base, then `git reset --soft <base>` and one commit, so conflicts, the gate and the train behave as before | why: the smallest change that keeps every merge path (single, train, batch, feature member) on one code path

# Queue
## T-690 [must] lane=hull milestone=M7.3 required
goal: The parallel-change check reads the branch's own commits. In crates/gear-engine/src/parallel.rs: `changed(repo, from, to)` for a task branch lists the files of the branch's own non-merge commits, `git log --no-merges --name-only --format= -z <from>..<to>`, instead of `git diff <from>...<to>` (a stale or multiple merge base listed files another task already landed, M7.1 M-8738). Keep the cache keyed as today.
accept: tests: a branch that merged dev after dev took another task's change to `plan.rs` is not reported as sharing `plan.rs`; the existing parallel tests pass.
estimate: 45
deps: -

## T-691 [must] lane=mast milestone=M7.3 required
goal: Config and commit message for the squash. (a) In crates/gear-app/src/config.rs add `merge_method: MergeMethod` to `Conventions` (reuse the existing `MergeMethod` enum, default `Squash`), parsed from `[conventions].merge_method` ("squash" | "merge"), and resolve it onto `Repo` as `pub merge_method: MergeMethod` the way `Repo.integration` is resolved (a `[[repos]]` entry may not override it). (b) In crates/gear-model/src/commit.rs add `pub const TRAILER_TASK: &str = "Gear-Task"`, `TRAILER_ATTEMPT = "Gear-Attempt"`, `TRAILER_CANDIDATE = "Gear-Candidate"`, `TRAILER_SOURCE = "Gear-Source"` and `pub fn squash_message(task: &str, attempt: &str, goal: &str, candidate: &str, sources: &[String], lines: &[String]) -> String`: subject `{task}: {first line of goal, cut to 72 chars at a word}`, a blank line, each of `lines` as `- {line}` (none: no body block), a blank line, then `Gear-Task: {task}`, `Gear-Attempt: {attempt}`, `Gear-Candidate: {candidate}` and one `Gear-Source: {sha}` per source. (c) In crates/gear-app/src/tasks.rs, where the merge op is inserted (`kind: OpKind::Merge`, payload with "branch"/"candidate"/"review"/"keep_branch"), add `"goal": <the task's goal>` to the payload.
accept: unit tests: `merge_method` parses both values and defaults to squash, and reaches `Repo`; `squash_message` builds the exact text for a goal longer than 72 chars, with and without lines, with two sources; the merge op payload carries the goal. `make check` for gear-app and gear-model.
estimate: 45
deps: -

## T-692 [must] lane=mast milestone=M7.3 required
goal: Shared git helpers in crates/gear-app/src/base.rs. (a) `pub fn landed_in(dir: &Path, reference: &str, commit: &str) -> Result<bool, String>`: true when `git merge-base --is-ancestor <commit> <reference>` says yes, or when `git log -1 --format=%H -F --grep="Gear-Candidate: <commit>" --grep="Gear-Source: <commit>" <reference>` prints a sha (full sha of `commit` resolved first with `git rev-parse --verify <commit>^{commit}`; a commit git cannot resolve is `Ok(false)`; any other git failure is `Err`). (b) `pub fn adds_nothing(dir: &Path, release: &str, integration: &str) -> Result<bool, String>`: true when `integration` is an ancestor of `release`, or when `git merge-tree --write-tree <release> <integration>` exits 0 and prints the same tree as `git rev-parse <release>^{tree}` (the integration branch's content is already on release, e.g. after a squash). (c) `pub fn added_paths(dir: &Path, release: &str, integration: &str) -> Result<Vec<String>, String>`: the paths `git merge-tree --write-tree --name-only`'s result tree changes against `<release>^{tree}` (`git diff --name-only <release>^{tree} <tree>`).
accept: unit tests on a temp repo: an ancestor is landed; a squashed commit whose message carries `Gear-Candidate: <sha>` or `Gear-Source: <sha>` makes that sha landed; an unrelated commit is not; `adds_nothing` is true for an ancestor and for a branch squashed onto release, false for a branch with a change of its own; `added_paths` lists only that change's files. `make check PKG=gear-app` (or the gear-app crate check).
estimate: 45
deps: -

## T-693 [must] lane=hull milestone=M7.3 required
goal: A task merge lands one squashed commit. In crates/gear-engine/src/merge.rs `prepare_merge`, after the merge loop and the conflict-marker check, when `repo.merge_method == MergeMethod::Squash`: collect `lines` = `git log --no-merges --reverse --format=%s <base_ref>..HEAD` minus subjects that start with the review prefix, run `git reset --soft <base_ref>`, then `git commit -m <gear_model::commit::squash_message(task, attempt, goal, sha, sources, lines)>` where `goal` is `op.payload["goal"]` (empty: the merge commit text `commit` as the goal), `sha` the candidate and `sources` the shas of the `merges` targets. Add `"goal"` to the payload of the merge op inserted in `collect_finished`/the queue step (around the `kind: OpKind::Merge` insert near line 555), read from the task row. `MergeMethod::Merge` changes nothing. The train and the batch call `prepare_merge`, so each of their tasks becomes one commit too.
accept: tests in merge.rs on a temp remote: with squash, a task branch of three commits plus a review commit lands as exactly one non-merge commit on dev whose message has the subject, the three lane subjects and the four trailer kinds, and dev's tree equals today's merge result; with `merge_method = "merge"` the merge commit is as before; a train of two tasks lands two commits in order. Existing merge tests pass (fix fixtures that read a merge commit's parents only where squash is now the default, by setting `merge_method = "merge"` in those fixtures).
estimate: 60
deps: T-691

## T-694 [must] lane=hull milestone=M7.3 required
goal: A feature lands by replaying its commits. In crates/gear-engine/src/merge.rs `prepare_merge`, for a feature's integration task (`op.payload["keep_branch"] == true`) in squash mode: instead of the merge loop, from `base_ref` cherry-pick each commit of `git rev-list --reverse --first-parent --no-merges <base_ref>..<task_tip>` in order (`git cherry-pick --allow-empty --keep-redundant-commits`); then amend the last commit's message to append `Gear-Source: <task_tip>` and `Gear-Candidate: <sha>`. When a cherry-pick fails: `git cherry-pick --abort`, `git reset --hard <base_ref>`, and land the feature as one squashed commit through the task squash path (body line `- replay conflicted; squashed`). The feature's own commits keep their messages and trailers.
accept: tests: a feature branch holding two squashed member commits and one merge of dev into it lands as those two commits on dev, in order, no merge commit, the last one with the two extra trailers; a feature whose replay conflicts lands as one squashed commit; the gate still runs once on the result.
estimate: 60
deps: T-693

## T-695 [must] lane=hull milestone=M7.3 required
goal: Members are found by trailer (write the key as the literal `Gear-Task`; T-691 adds the constant in parallel, no wait for it). In crates/gear-engine/src/merge.rs: `bisect_members` also takes a non-merge commit as a member when its message carries `Gear-Task: <task>` (read the log with `--format=%H%x09%P%x09%s%x09%(trailers:key=Gear-Task,valueonly,separator=%x2C)`; the member's name is the trailer's task); `run_bisect`'s probe stays a merge of the k-th member onto the base (on a linear feature branch that brings members 1..k). In `prepare_merge`'s `members_in` (the feature integration task's members), a first-parent commit with a `Gear-Task` trailer is a member too, its own commit (not `^2`) left out of the foreign-commit check.
accept: tests: a feature branch of three squashed members bisects to the member that breaks the gate; a feature integration task whose branch holds squashed members and no lane commit after the candidate is not refused as foreign; merge-commit members still work as before.
estimate: 45
deps: -

## T-696 [must] lane=hull milestone=M7.3 required
goal: The engine's merged checks accept a squash. Replace the ancestry test with `gear_app::base::landed_in(dir, reference, commit)` in crates/gear-engine: merge.rs `prepare_merge`'s `already` (the candidate is on `base_ref`), `on_a_stable_branch`, the carried-task check in `land_carried`; parallel.rs, the "candidate already in its base" test (`ancestor(&input.repo, &candidate, &base_tip)`). An error from `landed_in` keeps each site's current behavior for a git error (a check that cannot answer does not pass).
accept: tests: after a squash merge, a restart of the same merge op says it was already merged and pushes nothing; the lost-merge scan does not name a squashed task; a task squashed onto dev is not an open change in the parallel-change check.
estimate: 45
deps: T-692

## T-697 [must] lane=mast milestone=M7.3 required
goal: The plan and the coverage report accept a squash, and an integrated-by pass with only voyage files missing. (a) crates/gear-app/src/plan.rs: in the function around line 2780-2830 that runs `git merge-base --is-ancestor <candidate> <reference>` to tell whether a candidate is merged, use `crate::base::landed_in`; when it is not landed and the check serves an `integrated-by` pass, accept it when `git diff --name-only <reference> <candidate>` lists only paths under the project's voyage folder (`Repo.voyage_dir` expanded for the project). (b) crates/gear-cli/src/main.rs: the coverage report's commit check (`merge-base --is-ancestor` near line 2614-2690) uses `gear_app::base::landed_in`.
accept: tests: a squashed candidate counts as merged in the plan check and is not listed by `gear report coverage`; an `integrated-by=T-m` pass whose candidate differs from dev only in `gear/voyages/M1/reports/T-1.md` is applied; one that differs in a code file is still refused with today's text.
estimate: 45
deps: T-692

## T-698 [must] lane=mast milestone=M7.3 required
goal: The reclaim knows a squashed release and a records-only remainder. In crates/gear-engine/src/finished.rs: `branch_on_integration` uses `gear_app::base::landed_in`; `branch_on_release` and the integration-branch deletion guard use `gear_app::base::adds_nothing(dir, <remote>/<release>, <remote>/<integration>)` instead of ancestry (the namespace and `may_delete_branch` rules stay as they are). Where the reclaim names the integration branch as not on the release branch, compute `gear_app::base::added_paths`; when every path is under the voyage folder, the item says "only the voyage records on <integration> are not on <release>", is not urgent, and is written once per landing (a meta key `records_unreleased:<project>:<milestone>`).
accept: tests: a leftover task branch whose work is squashed on dev counts as on the integration branch; an integration branch squashed onto main by a pull request is deleted by the reclaim like one merged; one with a code change of its own is named as today; one whose only difference is a file under the voyage folder gets the one non-urgent line once, and a second scan writes nothing.
estimate: 45
deps: T-692

## T-699 [must] lane=mast milestone=M7.3 required
goal: The records merge follows the merge method and takes only voyage files. In crates/gear-engine/src/finished.rs `records_step`: before merging, list `git diff --name-only <remote>/<integration>...<remote>/<seat branch>`; any path outside the project's voyage folder refuses the step with `Recorded::Failed` naming up to 10 such paths ("the captain's branch changes files outside <voyage folder>: ...; move them to a task"). In squash mode the records land as one commit `records: <project> <milestone>` (merge, then `git reset --soft` to the integration tip, then commit), in merge mode as today.
accept: tests: a seat branch changing a code file is refused, its item names the file, nothing is merged or deleted; in squash mode the records land as one non-merge commit; existing records tests pass.
estimate: 45
deps: T-691

## T-700 [must] lane=mast milestone=M7.3 required
goal: The departure move and the docs. (a) crates/gear-cli/src/main.rs, the integration branch moved forward to the release head at departure (near line 1640-1660, `behind` from two `merge-base --is-ancestor` calls): `behind` is also true when `gear_app::base::adds_nothing(release, integration)` and integration is not equal to release, so an integration branch squashed onto release starts from release. (b) skills/gear/references/conventions.md documents `merge_method` ("squash" default: one commit per task, a feature replays its tasks' commits; "merge": merge commits), and the trailers; skills/gear/references/github.md says `merge_method` under `[github]` is the pull-request method and the push mode reads `[conventions].merge_method`.
accept: a test: an integration branch squashed onto main is moved to main's head at departure; one with its own work is left as it is. The two reference files carry the text.
estimate: 30
deps: T-692

## T-701 [must] lane=mast milestone=M7.3 required
goal: Record the model of every attempt. In crates/gear-app/src/tasks.rs `start_attempt` (where `tx.insert_attempt(&row)` runs) write meta `attempt.model:<project>:<task>:<n>` = the value of meta `engine.model.<seat>` (the model the seat runs now; write nothing when that meta is empty or absent). In the captain's done line path (the `insert_attempt` near line 744, a captain recording its own task done) write the same key with the captain seat's `engine.model.<seat>`, or `<binding kind> (captain)` when that meta is absent. Add `pub fn attempt_model_key(project, task, n) -> String` in tasks.rs.
accept: tests: a started attempt of a lane whose seat meta names a model carries that model under the key; a seat with no model meta writes nothing; a captain done line writes the captain's model or `claude (captain)`.
estimate: 15
deps: -

## T-702 [must] lane=mast milestone=M7.3 required
goal: Measure each model's speed. New module crates/gear-app/src/speed.rs (add `pub mod speed;` in lib.rs): `pub struct ModelSpeed { pub model: String, pub samples: u32, pub median_work_min: u32, pub median_ratio: f64 }` and `pub fn measured(tx: &Tx<'_>, now: Ts) -> GearResult<Vec<ModelSpeed>>`: over every project's attempts in the store from the last 90 days that were handed in, the work time is the dev time `crate::time` already computes for `gear report time` (reuse it, do not compute a second one) and the estimate is the task's `estimate_min` for attempt 1, `rework_estimate_min` for a later one (an attempt without an estimate counts for the work time, not the ratio); the model is `attempt.model:` meta, else the seat's current `engine.model.<seat>`, else the attempt is skipped. A captain's own done task counts too: its work time is its merge time minus the previous captain-done merge of the same voyage, or the voyage's departure for the first. Sorted by samples, most first. And `pub fn calibrated(speeds: &[ModelSpeed], model: &str, estimate_min: u32) -> Option<u32>` = `estimate_min × median_ratio` rounded, only when that model has at least 5 samples with a ratio.
accept: unit tests on an in-memory store: three models with known attempts give the right samples, medians and ratio; a captain-alone voyage's done tasks are measured from merge to merge; `calibrated` is None under 5 samples and the scaled minutes at 5 or more.
estimate: 20
deps: -

## T-703 [must] lane=mast milestone=M7.3 required
goal: Plan apply names estimates far from the measured speed. In crates/gear-app/src/plan.rs, after a successful apply, for each task of the applied file whose estimate is new or changed in this apply and whose lane has a model (the lane's first `models` entry in the config, else `engine.model.<seat>`; a captain's task: the captain seat's model), when `speed::calibrated` gives a value and the estimate is more than twice it or less than half of it, add a warning: `T-n: estimate <e> min; <model> has taken <ratio>x its estimates over <samples> tasks (median <m> min of work): about <c> min`. In the apply that departs a voyage (or applies with `--confirmed` to a departed one), add one line `eta <eta>; the measured speed of the lanes' models puts the open tasks at about <h> on the slowest lane` (per lane: the sum of calibrated minutes, or the estimate where none; the largest lane sum). Warnings only: nothing is refused.
accept: tests: with 5 measured attempts of a model at ratio 0.2, a new 60 min task on its lane gets the warning naming about 12 min; a 15 min task gets none; under 5 samples no warning; the departing apply prints the eta line.
estimate: 20
deps: T-702

## T-704 [must] lane=mast milestone=M7.3 required
goal: `gear report speed`. In crates/gear-cli/src/main.rs add `ReportOp::Speed` (`gear report speed`, with `--json`): one row per model from `gear_app::speed::measured`: model, samples, median work minutes, median actual/estimate ratio (`0.14x`), under a header naming the store's period (last 90 days); no rows: one line saying nothing is measured yet. It reads the store of the caller's session like `gear report time`.
accept: a CLI test: a store with attempts of two models prints both rows with their numbers; `--json` gives the same fields; an empty store prints the one line.
estimate: 15
deps: T-702

## T-705 [must] lane=mast milestone=M7.3 required
goal: The skill tells the captain how to estimate. skills/gear/references/captain.md (where estimates and the eta are described) gets one paragraph: an estimate is the agent's working time on the model that runs the task, never a human's hours; run `gear report speed` before estimating and scale by the lane model's measured ratio; with nothing measured, a lane task of a few dozen lines is minutes, not an hour, and the station's pace corrects it; the voyage's eta is the slowest lane's sum, and an apply warning about an estimate is answered by fixing the estimate. skills/gear/references/departure.md (the sheet): the Eta row shows the eta and, when apply printed it, the measured forecast. Reinstall nothing (the captain does at release).
accept: both files carry the text; `make check` for nothing else is needed (docs only).
estimate: 10
deps: -


## T-706 [should] lane=mast milestone=M7.3 required
goal: Fix the flaky reclaim test. `finished::tests::a_clean_pushed_worktree_is_removed_and_not_named` (crates/gear-engine/src/finished.rs) fails when the finished tests run in parallel in one process (`cargo test -p gear-engine --lib finished::`, and reviewers' `make check CRATE=gear-engine` runs) and passes alone; it cost T-698's review several minutes (F-351). Find what the parallel tests share (a fixed path or name, the process scan of GEAR worktrees, a Herdr call) and give the test its own; never add a retry or a sleep.
accept: `cargo test -p gear-engine --lib finished::` passes 5 times in a row; the test still asserts the worktree is removed and not named.
estimate: 15
deps: -

## T-707 [should] lane=mast milestone=M7.3
goal: A new block can be named so another new block depends on it without guessing its id. (a) crates/gear-app/src/draft.rs `parse_block_header`: the header id is `NEW` or `NEW:<label>` (label: lowercase letters, digits and `-`); add `pub label: Option<String>` to `QueueItem`; two blocks with one label in a file are refused with the line. (b) crates/gear-app/src/plan.rs, before the loop that refuses `dep {dep} does not exist and is not created by this draft`: build label -> id from `new_ids` (the id each NEW block gets in this apply) and replace every `NEW:<label>` in an item's `deps` with that id; a label no block carries is refused (`dep NEW:<label> names no block of this draft`). (c) The file rewrite in draft.rs (where `## NEW` headers become `## T-n`, near `ids.next()`): a `## NEW:<label>` header becomes `## T-n` the same way, and each `NEW:<label>` in a `deps:` line becomes its `T-n`. (d) skills/gear/references/captain.md, where `## NEW` blocks are described: one sentence on `## NEW:<label>` and `deps: NEW:<label>`.
accept: tests: a draft with `## NEW:core` and a second block `deps: NEW:core` applies in one go, the second task's stored deps name the first's id, and the rewritten file shows both ids; an unknown label and a repeated label are refused; a plain `## NEW` block works as before.
estimate: 20
deps: -

## T-708 [should] lane=mast milestone=M7.3
goal: The captain's home page says the voyage's done sentence once. On `gear` (no arguments) the done text is printed three times: the board voyage block (crates/gear-board/src/voyage.rs, the line `Voyage {} {} · done: {}{waits}`), the goal line (crates/gear-board/src/stations.rs, `Your goal, voyage {milestone}: done: ...`, which stays as it is) and the snapshot's voyage line (crates/gear-app/src/home.rs line ~736, `crate::plan::voyage_line(voyage)`). Make voyage.rs's line `Voyage {} {}{waits}` (no done text), and in home.rs print a short line instead of `voyage_line`: `Voyage <milestone> <title> · eta <eta> · bar <first word of the bar>`. `plan::voyage_line` and its other callers (time.rs, plan.rs) do not change.
accept: a home test: the captain's page carries the done sentence exactly once (in the goal line); the board's own voyage view still shows the voyage name and its clock; existing home and board tests pass with their expected texts updated.
estimate: 15
carried-by: T-712
deps: -

## T-709 [should] lane=mast milestone=M7.3
goal: A restart request has its own key. Today `- restart <lane|reviewer>` writes meta `engine.restart.<seat>` = `{"at":…}` (crates/gear-app/src/plan.rs, the `tx.set_meta(&format!("engine.restart.{seat_id}"), …)` near line 3735), and crates/gear-engine/src/restart.rs uses the same key for its restart plan (`plan_key`, line ~1943), so a request applied while a plan is pending overwrites the plan mid-switch (M7.1 T-665). Write and read the request under `engine.restart-request.<seat>`: plan.rs writes it; restart.rs reads and clears it where it reads the flag today (line ~327 and the consume at ~361, and drop the comment that says the flag and the plan share the key); `plan_key` stays `engine.restart.<seat>`. A request found under the old key with an `at` field and no plan fields is read once as a request, so one written before the upgrade is not lost.
accept: tests: a restart request applied while a restart plan is pending leaves the plan as it was and runs after it; a request on an idle lane restarts it as before; an old-key request is honored once. Update the restart.rs tests that set `engine.restart.p:heat` to `{"at":…}` to the new key.
estimate: 15
deps: -

## T-710 [should] lane=mast milestone=M7.3
goal: A landing whose seat branch is missing names the captain branches the remote does have. In crates/gear-engine/src/finished.rs `merge_records`, the `Recorded::Missing` arm today resolves the records item with "there is no branch {branch} on the remote" (Lawrence M2: the seat branch was pinned `lawrence/captain/M2`, the captain pushed `lawrence/captain/M1`, and the records were never merged). List the remote's `<project>/captain/*` branches (`git for-each-ref --format=%(refname:short) refs/remotes/<remote>/<project>/captain/`, the `<remote>/` prefix cut). When there are any, open (or keep) the captain item on thread `records:<milestone>` through the existing `tell_records_failed` path with the text `no branch <branch> on <remote>; the remote has <list>: push this voyage's records to <branch>`, and do not resolve; when there are none, resolve as today.
accept: tests: a landing whose seat branch is missing while `p/captain/M0` is on the remote gets one item naming `p/captain/M0` and nothing is merged; a landing with no captain branch at all resolves as before.
estimate: 15
deps: -

## T-711 [should] lane=mast milestone=M7.3
goal: A Claude lane or reviewer whose plan limit is spent is seen as out of quota. In crates/gear-engine/src/quota.rs, where a seat's quota signal is read (`let quota = obs.flags.contains(&ScreenFlag::Quota) && obs.idle;`, near line 284), a seat whose profile's `usage_log` is `claude-jsonl` also signals quota when `gear_usage::claude_limit_stop(<the seat's session>)` returns a stop whose `resets_at` is absent or later than now (the session is `obs.session`, the same value captain_quota.rs passes; skip when it is empty). The rest of the lane quota machine (confirm rounds, source, fallback, park) is unchanged; when the stop has a `resets_at`, the park or fallback uses it as the reset instead of `[models] quota_reset_h` if the machine already takes a reset time, else leave the reset as it is.
accept: tests: a claude lane whose session log carries a rejected `quotaLimits` record with a future reset is treated as out of quota after its confirm rounds (parked or switched as its policy says); one whose record's reset has passed is not; a pi lane is unaffected.
estimate: 20
deps: -

## T-712 [must] lane=mast milestone=M7.3
goal: Carry T-708 onto dev: its merge gate failed on crates/gear-engine/tests/it/upgrade_contract.rs (line ~71 asserts the home page contains `Voyage M5 the next voyage · done: it sails`, the line T-708 shortened). Start your branch from T-708's candidate 61fb9235f2faab7729a34404c459f63068e32d07 (`git switch -c <your task branch> 61fb9235f2faab7729a34404c459f63068e32d07`), change that assertion to the short line T-708's home page now prints for that voyage (`Voyage M5 the next voyage` with its eta and bar as home.rs writes them) and check the done sentence still appears once through the goal line, then grep crates/ for any other test asserting `· done: ` on the home page and fix it the same way.
accept: `make check CRATE=gear-engine TESTS='test(upgrade)'` passes; `make check CRATE=gear-app` passes; the diff touches only test files.
estimate: 10
deps: -

