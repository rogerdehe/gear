<!-- gear-plan project=gear base=1256 -->
# Goal
Client fleets run smoothly: the merge, restart and message problems Lawrence met are gone, a new GEAR binary takes over every engine on the machine by itself with no merge cut, and a voyage takes GitHub issues by a label that names the project and the voyage. Each fix is released and live for every project on this machine.

# Milestone M7: client fleets run smoothly
done: (1) an idle seat gets its messages as text in one prompt, a busy seat keeps the one-line doorbell, and a review message carries the verdict, the blocking findings by title and where the full review is; (2) a replaced `gear` binary restarts every engine on the machine by itself: no new merge starts, the running ones finish, and the engine runs the new binary under the same lock, with no pause and no cut merge; `gear admin restart` does the same at once; (3) an engine item lists at most 10 lines per section and names a command for the rest, and the store keeps only the last 20 plan versions per project; (4) the Lawrence merge frictions are gone: a red feature carrier is kept when a member is added, a queued remerge closes the merge-failed item, a carrier that merged without the carried work is named in the merge-lost item, the reclaim closes the merge workspace, and the idle-voyage item rings once per open land order; (5) a departed voyage of a GitHub project gets its `<project>/<milestone>` label on GitHub, a newly labeled open issue becomes one captain item, a removed label or changed text becomes an item, the board shows the voyage's issues by state, and a land is refused while a labeled issue is pending; (6) each fix is released as v0.6.x, live for every project on this machine, and each captain told
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 8h
depart: now

# Decisions
- station M-8571: keep course | why: 12 done, 9 left, projected 143m from departure against 404m left; hull 67m (T-635 then the F-4 forge members and T-639), mast 28m then the F-4 integration T-651 once hull's members merge; a move of hull's gear-engine/gear-forge work would need an owns change for little gain
- station M-8558: keep course | why: 9 done, 12 left (310m by estimate), projected 142m from departure against 418m left; hull 80m and mast 55m, no move needed
- station M-8555: keep course | why: 60m in, 8 done, 13 left (335m by estimate), projected 157m from departure; hull 97m vs mast 55m, but hull's rest is gear-engine and gear-forge work outside mast's paths, so a move would cost an owns change for about 20m
- station M-8540: keep course | why: 6 done, 15 left (400m) against 430m, projected 179m from departure; hull holds 129m and mast 78m, close enough that a move costs more than it saves
- 2026-11-06 hand-fix: T-640's merge gate failed: the gear-engine example opencode_live implements HerdrAct and lacked the new workspace_close; the captain added the forwarding method on mast/t640 (hull's file, three lines) and remerges | why: the packet missed that a new trait method needs every implementor, the example included
- station M-8521: keep course | why: 41m in, 3 done (T-640, T-641, T-642 class: small mast tasks); 18 tasks (505m) against 439m left, both lanes working and 2 reviews running; the pace is measured on cold builds under load and is rechecked at the next station
- 2026-11-06 18:40 confirmed: M7 departure, not departed -> departs now with the scope above, eta 8h | why: the user agreed the M7 goal and scope and said to start M7 at once
- 2026-11-06 scope: the shared-integration-branch item is already shipped (v0.6.68: a second project on a repo gets `<project>/dev`, and a shared branch is refused); it leaves the M7 list | why: `admin::with_own_integration` and `branch_clash` do both
- 2026-11-06 scope: the `context_window_k` warning (M6.3 T-626) is not taken: since v0.6.90 a model with no known window never compacts, and at `admin init` no session exists to read the CLI's own window | why: nothing in normal use goes wrong without it
- 2026-11-06 scope: F-341 (feature members merged with no light check) stays in the backlog: it needs a finding of why the checks were skipped before any task, and no client fleet is hit by it
- 2026-11-06 scope: F-4 (issue intake by label) is GEAR's fourth feature: its members merge into the feature branch and reach dev once
- 2026-11-06 note: GitHub lets only a collaborator with triage rights set a label, so a label set on GitHub enrolls the issue with no further check (backlog item 2)
- 2026-11-06 note: hull owns crates/gear-forge this voyage (the intake members), mast everything outside hull's crates; the plan store is backed up and vacuumed by the captain at the release that prunes it

# Queue
## T-631 [must] lane=hull milestone=M7 required
goal: An idle seat gets its waiting messages as text, not a one-line doorbell. crates/gear-engine/src/doorbell.rs, lib.rs (the ring inputs, about line 1607) and ops.rs (`intend_rings`, `confirm_pending_held`). (a) `DoorbellItem` gets `sender: String` and `body: String`, filled from the `MessageRow` where the items are built in lib.rs (and in ops.rs `deliver_doorbell_now`, which keeps today's one-line text). (b) In `rings`, a seat that is idle (`input.idle`), holds no slot and has no `needs_ack` item among its waiting items gets the inline text (a ring holding a needs_ack item keeps today's one-line text, since such an item closes only through `gear send --kind ack`; `DoorbellItem` gets `needs_ack: bool` for this): `GEAR: <n> message(s) for you.` then, per item in order, a blank line, `[<id> from <sender>, urgent]` (`, urgent` only when urgent) and the body; then a blank line and `Reply with \`<command> reply <id> -m "<answer>"\`; \`<command>\` shows them again.`; for a lane with no task (`no_task`) the line `You have no task now: answer these, then end your turn; do not continue earlier work.` follows; the captain's goal line follows as today (`with_goal`). The inline text is typed by the same path a packet delivery uses for its multi-line text (see how `execute_intended` types a Deliver op and do the same for an inline Doorbell op), and the composer's held-text check (`composer::holds`, v0.6.107) must not take the typed ring for text stuck in the input box: check it with a multi-line ring in a test. A joined ring of several captain seats on one pane (T-487) uses the same format, each item's header prefixed with its project. (c) When the inline text is longer than 4000 chars it is cut at the last line break before 3800 chars and ends with `… cut here: run \`<command>\` to read all <n> item(s).`; such a ring is NOT inline. A seat that is working (the at-once path) keeps today's one-line text. (d) The ring's op payload gets `"inline": true` when the full text went out. In `confirm_pending_held`, a Doorbell op in Submitted with `inline: true` is Confirmed at once and one `ViewRecord` is inserted for its seat and generation with `shown_msgs` = its covers (and for each other seat of the ring, its own covers), `ts` = now; needs_ack items stay open as today, only their showing is recorded.
accept: Tests in doorbell.rs `mod tests` and ops.rs `mod tests`: an idle seat with two short messages gets one prompt holding both bodies with headers and the reply line; the op confirms on submit and a view lists both ids, so the next tick rings nothing; a working seat with an at-once message gets the one-line text; a 6000-char body is cut with the `cut here` line and the op waits for a view as today; a ring with a needs_ack item is the one-line text; a lane with no task gets the no-task line; a multi-line ring is typed whole and not reported as held. Run only `make check CRATE=gear-engine TESTS='test(doorbell) | test(ops::)'`; the merge gate runs `make gate`.
estimate: 60
deps: -

## T-632 [must] lane=hull milestone=M7 required
goal: The merge workers can drain. crates/gear-engine/src/merge.rs: `MergeWorkers` gets a shared `draining` flag (`Arc<AtomicBool>`, cloned with the struct like `workers`), `pub fn drain(&self)` sets it and `pub fn idle(&self) -> bool` is true when no worker is in `workers` and no bisect is running (`bisect_running` for any project). While draining, every place that starts a worker thread (the single merge, about line 875; `launch_train` 1127; `launch_batch` 1234; the bisect, about line 2210) returns before marking any op Executing and starts nothing: the op stays in its state for the next engine. Finished workers are still collected by `collect_finished` as today. Export both through `gear_engine` (e.g. `pub fn drain(cfg: &EngineCfg)` and `pub fn merges_idle(cfg: &EngineCfg) -> bool` in lib.rs).
accept: Tests in merge.rs `mod tests`: with a ready merge and `drain()` set, a tick starts no worker and the op is still Intended; a worker started before `drain()` finishes and is collected, after which `idle()` is true. Run only `make check CRATE=gear-engine TESTS='test(merge::)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-633 [must] lane=hull milestone=M7 required
goal: An engine branch item stays short (Prometheus M-8671 was 60k chars: 28 branches with every commit). crates/gear-engine/src/merge.rs `sweep_in` (lines 6185-6290): (a) a done branch's line names its commit count and its newest commit only: `<name> (<task>, done, <n> commit(s) not on <integration>, newest: <first line of git log --oneline>)`; (b) each of the four sections (crowded, old, left, deleted) lists at most 10 lines; when more, a last line `- … and <k> more: \`git branch -r --list '<remote>/<prefix>*'\`` where the prefix is the task-branch template up to its first `{` (or `git branch -r` when the template starts with `{`); the section header keeps the full count. Nothing else in the item changes.
accept: Tests in merge.rs `mod tests`: a sweep over 25 left branches with 30 commits each writes 10 lines, each naming its count and newest commit, and the `and 15 more` line; the item is under 4000 chars. Run only `make check CRATE=gear-engine TESTS='test(merge::)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-634 [must] lane=hull milestone=M7 required
goal: The idle-voyage item rings once per open land order (GEAR M6.3: M-8480, M-8481 every few minutes while a land waited for the user's confirmation). crates/gear-engine/src/quiet.rs `captain_sailing`: when every task is done (`left == 0`) and `landing_open` is true, read the land order's message id (meta `station:<project>:<milestone>:land`); if meta `sailing-land:<project>:<milestone>` already holds that id, ring nothing; else insert the item with the body `Voyage <m>: every task is done and its land order <id> is open. Land it (\`gear\` shows the order); if the land is refused for a decision only the user can make, tell the user and wait: this item is not repeated while that order stays open.` and set the meta to the id. Voyages with tasks left keep today's text and repeat.
accept: Tests in quiet.rs `mod tests`: an idle fleet with every task done and an open land order gets one item, and none after 30 more minutes idle; a new land order id rings once more; a voyage with tasks left still repeats after `CAPTAIN_AGAIN`. Run only `make check CRATE=gear-engine TESTS='test(quiet::)'`; the merge gate runs `make gate`.
estimate: 20
deps: -

## T-635 [should] lane=hull milestone=M7 required
goal: The lost-work scan names a carried task whose carrier landed without its work (M6.3 T-615). crates/gear-engine (the scan that opens `merge-lost:<task>` items, merge.rs around line 6880): a task marked carried by T-m whose T-m has merged a candidate that does not contain the carried task's accepted candidate (`git merge-base --is-ancestor <carried candidate> <carrier's merged candidate>` fails) gets the existing merge-lost captain item, naming the carrier and the way out: `carried-by: -` on the task and `- remerge T-<n>` under # Staff.
accept: A test in the scan's `mod tests`: a carried task whose carrier merged without its candidate gets one merge-lost item naming the carrier; one whose candidate is inside the carrier's gets none. Run only `make check CRATE=gear-engine TESTS='test(lost)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-636 [must] lane=hull milestone=M7 required
feature: F-4
goal: [F-4 2/7] The forge can create a label. crates/gear-forge/src/lib.rs: the `Forge` trait gets `fn create_label(&self, repo: &str, name: &str, description: &str) -> Result<(), Error>`; github.rs runs `gh label create <name> --repo <repo> --description <description> --color 0E8A16 --force` through `run` (`--force` updates a label that exists, so the call is idempotent); local.rs and every test double get an Ok implementation that records the call.
accept: Tests in crates/gear-forge: the GitHub impl with a `GEAR_GH` stub script receives exactly those arguments; a non-zero exit is `Failed`. Run only `make check CRATE=gear-forge`; the merge gate runs `make gate`.
estimate: 20
deps: -

## T-637 [must] lane=hull milestone=M7 required
feature: F-4
goal: [F-4 4/7] The engine creates the voyage label at departure. crates/gear-forge/src/sync.rs (gear-forge cannot depend on gear-app, so it reads the store itself): its own config parse (lines 933-987) reads `[github.issues] voyage_label` with the default and `""` rule of T-643, and a small `voyage_label(template, project, milestone)` substitutes `{project}` and `{milestone}`. The voyage is the project's voyage with the latest departure (`tx.voyages_of_project`), unless meta `station:<project>:<milestone>:land` names a message that is Resolved (landed). Each sync of a GitHub project with such a voyage and a label, and meta `forge.label.<project>.<milestone>` not `created`: one forge call through `run_call` with key `label:<milestone>` that runs `create_label(repo, label, "GEAR voyage <milestone> of <project>")` (T-636); when Confirmed, set the meta to `created`. A failed call retries on a later tick as other calls do.
accept: Tests in sync.rs `mod tests` with a recording forge: a departed voyage gets one create_label call and none on the next tick; an undeparted voyage or a label set to `""` gets none; a failed call is retried. Run only `make check CRATE=gear-forge`; the merge gate runs `make gate`.
estimate: 30
deps: T-643

## T-638 [must] lane=hull milestone=M7 required
feature: F-4
goal: [F-4 5/7] Labeled issues become captain items. crates/gear-forge/src/sync.rs: at most every 120 s per project (meta `forge.intake_at.<project>`), for the voyage of T-637 with its label created: `issues(Filter { labels: vec![label], limit: 500 })` (gh lists 30 by default; with fewer than the limit returned, the list is complete; when exactly 500 come back, treat the list as partial and mark nothing unlabeled this poll). An error changes nothing (never read as an empty list). For each listed issue: unknown number → `upsert_voyage_issue` (labeled 1, body_hash = a hex hash of title + body) and one urgent captain item, thread `intake:<milestone>:#<n>`, body `Issue #<n> "<title>" was labeled <label>: plan it as task(s) with \`issue: #<n>\` and their contracts and deps, or resolve this item to defer it. It never moves the eta or the bar.`; known with a different hash → update the row and one item `Issue #<n> changed on GitHub: check its tasks' contracts.`; known with labeled 0 → labeled 1 and one item `Issue #<n> carries <label> again.` For each row with labeled 1 that the list no longer holds → labeled 0 and one item `Issue #<n> no longer carries <label> (closed or unlabeled): its tasks keep running; cancel them in the plan if the user wants.` Items use the store's normal insert, so the doorbell wakes the captain once.
accept: Tests in sync.rs `mod tests`: a new labeled issue makes one item and a second poll makes none; a changed body makes one changed item; a vanished issue makes one item and sets labeled 0; a forge error makes no item and changes no row; two polls within 120 s list once. Run only `make check CRATE=gear-forge`; the merge gate runs `make gate`.
estimate: 50
deps: T-645

## T-639 [should] lane=hull milestone=M7 required
goal: The reclaim closes a workspace whose worktree is already gone (GEAR M6.3 landing, M-8482: w1KM gear-merge stayed; `herdr workspace close w1KM` by hand worked). crates/gear-engine/src/finished.rs (lines 322-339): when `herdr.worktree_remove(&workspace.id)` is Refused or Uncertain, call `herdr.workspace_close(&workspace.id)` (T-640); only when that is not Ok too is the workspace named `(could not close)`.
accept: A test in finished.rs `mod tests` with the fake Herdr: a workspace whose worktree remove is refused is closed by `workspace_close` and the reclaim item names nothing left. Run only `make check CRATE=gear-engine TESTS='test(finished::)'`; the merge gate runs `make gate`.
estimate: 15
deps: T-640

## T-640 [should] lane=mast milestone=M7 required
goal: Herdr can close a workspace for GEAR. crates/gear-herdr/src/lib.rs: add `fn workspace_close(&mut self, workspace_id: &str) -> Outcome<()>` to `HerdrAct`, implemented for the real client with the socket call `workspace.close` and `{"workspace_id": id}` (the call behind `herdr workspace close <id>`; if Herdr names it differently, ask) and in crates/gear-herdr/src/fake.rs (removes the workspace, recorded in the fake's calls). No caller yet (T-639).
accept: Tests in crates/gear-herdr: the fake removes the workspace and records the call; the real client sends the method and params (the existing request-capture test pattern). Run only `make check CRATE=gear-herdr`; the merge gate runs `make gate`.
estimate: 15
deps: -

## T-641 [must] lane=mast milestone=M7 required
goal: A review message carries the verdict and where the review is, not the whole review. crates/gear-app/src/mail.rs (about line 562, the body of a `MsgKind::Review`): the message body is the headline (`review::headline`), then `blocking: <title line of each blocking finding>` (for each id in `review.blocking`, the first findings line whose text after an optional `- ` starts with that id followed by a space or `[`, cut to 160 chars; `blocking: none` when the list is empty), then `full review: <path>` when the reviewer handed a path (the original `args.body`, trimmed, when `findings_text` read a file) and `findings: \`gear --show <task>\`` always. The `review_verdicts` row keeps the full findings as today (the rework lane reads them there).
accept: Tests in crates/gear-app/tests/it (the review send contract): a fail with `--blocking F1,F3` and a findings file stores a message under 1000 chars naming both titles and the path, while the verdict row holds the whole findings; a pass with no blocking says `blocking: none`. Run only `make check CRATE=gear-app TESTS='test(review)'`; the merge gate runs `make gate`.
estimate: 20
deps: -

## T-642 [must] lane=mast milestone=M7 required
goal: A queued remerge closes the task's merge-failed item too (M6.3 T-628; Lawrence M-921 kept `merge-failed:T-63` open while its remerge was queued). crates/gear-app/src/plan.rs `close_lost` (v0.6.108, called by `remerge`): resolve the open and waiting messages of thread `merge-failed:<task>` as well as `merge-lost:<task>`, with the same `remerge queued (O-n)` result. A failed remerge opens its own merge-failed item as today.
accept: A test in crates/gear-app/tests/it/remerge_contract.rs: a task with an open merge-failed item and a `- remerge` line leaves the item resolved with `remerge queued`. Run only `make check CRATE=gear-app TESTS='test(remerge)'`; the merge gate runs `make gate`.
estimate: 15
deps: -

## T-643 [must] lane=mast milestone=M7 required
feature: F-4
goal: [F-4 1/7] The voyage label setting. crates/gear-app/src/config.rs: `IssuesSection` gets `voyage_label: Option<String>`; `Github` (the resolved form, line 370) gets `voyage_label: String`, default `{project}/{milestone}` when `[github]` is set, `""` (off) when the setting is `""`. Validation (`github_issues`, line 2323): a non-empty label must contain `{milestone}`. Add `pub fn voyage_label(github: &Github, project: &str, milestone: &str) -> Option<String>` (None when off; `{project}` and `{milestone}` substituted). Document the setting in skills/gear/references/github.md (one line).
accept: Tests in config.rs `mod tests`: default gives `carthage/M2`; `""` gives None; `"x"` is refused naming `{milestone}`. Run only `make check CRATE=gear-app TESTS='test(config)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-644 [must] lane=mast milestone=M7 required
goal: A member added to fix a red feature carrier keeps that carrier (Lawrence M2: T-96 passed review, went red at the full gate, the captain added member T-106, and the apply made T-107 the new F-1 carrier, so the fix cost a second integration review). crates/gear-app/src/plan.rs `feature_work` (lines 2296-2306): `carrier_open` is also true for a stored carrier that is Done but has no merge (`crate::tasks::merged_at(tx, project, carrier)` is None), so it is kept and touched (its deps get the new member) instead of minting a new carrier. A Done carrier that has merged keeps today's rule. The captain then merges the member and remerges the carrier (`- remerge T-<carrier>`).
accept: A test in crates/gear-app/tests/it (the feature plan contract): a feature whose carrier is Done and unmerged gets a new member: no task is created, the carrier's deps hold the new member; with the carrier merged, a new member still mints a fresh carrier as before. Run only `make check CRATE=gear-app TESTS='test(feature)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-645 [must] lane=mast milestone=M7 required
feature: F-4
goal: [F-4 3/7] The store records a voyage's labeled issues. crates/gear-store: a table `voyage_issues (project TEXT NOT NULL, number INTEGER NOT NULL, milestone TEXT NOT NULL, title TEXT NOT NULL, body_hash TEXT NOT NULL, labeled INTEGER NOT NULL, first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL, PRIMARY KEY (project, number))` added the way the schema adds tables today; `VoyageIssueRow` with those fields; `Tx::upsert_voyage_issue(&VoyageIssueRow)` and `Tx::voyage_issues(project, milestone) -> Vec<VoyageIssueRow>` ordered by number. crates/gear-board (gear-app and the board both read it; gear-board cannot depend on gear-app): a new module `issues.rs` with `pub fn issue_states(tx, project, milestone) -> GearResult<Vec<(i64, IssueState)>>` and `IssueState::{Done, Verifying, InProgress, Planned, Deferred, Pending, Unlabeled}`: Unlabeled when `labeled` is 0; for the tasks of the milestone with `issue = n` (cancelled ones left out): Done when every one is Done and merged (an attempt with a merged time, as `gear_app::tasks::merged_at` reads it from `attempt_times_of_task`); Verifying when every one is Done and one is not merged yet; InProgress when one has an attempt; Planned when there are such tasks and none has an attempt; Deferred when no task carries it and its intake item (thread `intake:<milestone>:#<n>`) is resolved; Pending otherwise.
accept: Tests in crates/gear-store/tests/it (upsert then read; a second upsert updates) and in issues.rs `mod tests` (one test per state). Run only `make check CRATE=gear-store` and `make check CRATE=gear-board`; the merge gate runs `make gate`.
estimate: 35
deps: -

## T-646 [must] lane=mast milestone=M7 required
goal: The store keeps only the last 20 plan versions per project (Selyra's store: 604 MB of 730 MB are plans, Prometheus 1429 versions). crates/gear-store/src/lib.rs `insert_plan`: after the insert, `DELETE FROM plans WHERE project = ?1 AND version <= ?2 - 20` with the new version (a `pub const PLAN_VERSIONS_KEPT: i64 = 20`). crates/gear-app/src/plan.rs `departure_from_history` (about line 5209, used only by `fix_departures` for a departed voyage with no recorded estimate): scan from the oldest stored version (`SELECT MIN(version)`, a new `Tx::oldest_plan_version`) instead of 1, and when that oldest version is above 1 (history pruned) and already holds the milestone's tasks, return None: the voyage's first draft is gone, so no departure or estimate is recovered rather than a wrong one. No VACUUM.
accept: A test in crates/gear-store/tests/it: 25 inserted versions leave versions 6-25, `plan_version` is 25 and `get_plan(…, 3)` is None; in crates/gear-app/tests/it a voyage whose first draft was pruned recovers no estimate; the tests at slice2_contract.rs:68 and read_indexes_contract.rs:107 still pass. Run only `make check CRATE=gear-store` and `make check CRATE=gear-app TESTS='test(departure)'`; the merge gate runs `make gate`.
estimate: 25
deps: -

## T-647 [must] lane=mast milestone=M7 required
goal: A new binary restarts the engine by itself, with no merge cut. crates/gear-cli/src/main.rs: (a) `run_engine_with` returns an outcome `Exit` or `Restart`. When the binary on disk changed (BinaryWatch: modification time differs from start and has not changed for 5 s; no 600 s grace for this) or a SIGHUP arrived (a `RESTART` atomic set by a new handler next to `install_stop_handler`), the loop calls `gear_engine::drain(cfg)` (T-632), logs `engine draining for restart`, keeps ticking, and once `gear_engine::merges_idle(cfg)` returns `Restart`. A SIGTERM stop drains the same way before it returns `Exit` (the `admin stop` wait already covers it). (b) `cmd_run` on `Restart` logs `engine restart: exec <path>`, drops the store, and calls `std::os::unix::process::CommandExt::exec` on `std::env::current_exe()` with the same arguments; the engine lock file is opened close-on-exec, so the new process takes it again; when exec fails it logs the error and exits 1 (the next `gear` call starts an engine). (c) `gear admin restart`: for every store `admin projects` knows, check that its `engine.lock` is held (a non-blocking `try_lock` fails; a free lock means no engine, and its pid is stale, never signalled), then read the pid in it and send SIGHUP; print one line per store (`restarting <db> pid <n>` or `no engine`). The BINARY_CHANGED captain item is kept only for an engine still draining 30 min after the change.
accept: Tests in main.rs `mod tests` with an injected watch and tick: a changed binary drains and returns `Restart` only after the merges are idle; a SIGTERM with a running merge keeps ticking until it is idle; `admin restart` with a store whose lock names a live pid sends it SIGHUP (use a child `sleep` process). Run only `make check CRATE=gear-cli TESTS='test(engine) | test(admin_restart)'`; the merge gate runs `make gate`.
estimate: 45
deps: T-632

## T-648 [must] lane=mast milestone=M7 required
feature: F-4
goal: [F-4 6/7] A land lists the voyage's issues and refuses while one is pending. crates/gear-app/src/station.rs `require_land_ready` (line 410): after the unmerged check, `gear_board::issues::issue_states` (T-645) of the milestone; any Pending → refused, no confirmation overrides it: `station <id>: land is refused: labeled issue(s) #a, #b are not planned or deferred; plan each as tasks with \`issue: #n\` or resolve its intake item to defer it`. The ready-to-land item (`station.rs`, the `{key}:land` item) gets one line `issues: done #…; deferred #…; unlabeled #…` when the voyage has any.
accept: Tests in crates/gear-app/tests/it (the station land contract): a pending issue refuses the land even with `--confirmed`; a deferred one lands and is listed. Run only `make check CRATE=gear-app TESTS='test(land)'`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-649 [must] lane=mast milestone=M7 required
feature: F-4
goal: [F-4 7/7] The board shows the voyage's labeled issues. crates/gear-board: in the voyage view, when the voyage has rows in `voyage_issues`, one block `Issues (<label>)` with one line per state that has members: `pending #3 #9`, `planned #4`, `in progress #5`, `awaiting verification #6`, `done #1`, `deferred #7`, `unlabeled #8`, from `issue_states` (T-645, the same crate). Nothing when there are none.
accept: A test in crates/gear-board with a fixture store: the block lists each state with its numbers; a voyage with no rows shows no block. Run only `make check CRATE=gear-board`; the merge gate runs `make gate`.
estimate: 30
deps: -

## T-650 [should] lane=mast milestone=M7 required
goal: docs/design/07-mail.md says how a message reaches an idle seat (T-631): a seat that is idle and holds no delivery gets the messages' text in one prompt (headers, bodies, the reply line), cut at about 4000 chars with the command that shows the rest; such a prompt counts as shown once Herdr took it, and needs_ack items stay open until answered; a working seat keeps the one-line doorbell. Edit only the doorbell section, a few lines.
accept: The section reads as above and contradicts no other line of the file. Run only `make check CRATE=gear-app TESTS='test(docs)'` if such a test exists, else nothing; the merge gate runs `make gate`.
estimate: 10
deps: T-631
