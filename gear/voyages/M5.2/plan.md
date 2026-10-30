<!-- gear-plan project=gear base=885 -->
# Goal
After a voyage lands, the board shows that voyage, not an empty one: a milestone no plan described never becomes a voyage. Released as v0.5.6.

# Milestone M5.2: the board after a landing
done: (1) every M5.2 task merged; (2) on a store whose plan-less milestone departed after the last landed voyage, the board shows the landed voyage; (3) released as v0.5.6 and the shared engine runs it
bar: DELIVERABLE: an error in normal use blocks the task; edge cases outside normal use are logged
eta: 2h

# Decisions
- 2026-10-30 07:05 scope: fix at once the blank board another project saw after its M1.1 landed (user: handle it now): crates/gear-board/src/lib.rs fn workload_voyage, with no voyage sailing, takes the latest departure, and that store's milestone `M1.1-cancelled` (one cancelled task, no `# Milestone` block) departed after M1.1, so the board shows a voyage with no time, 0/0 tasks and a done sentence of 0 clauses; the item moves here from gear/voyages/M6/backlog.md | why: user | else: wait for M6
- 2026-10-30 07:05 model: the same staffing as M5.1: lanes hull and mast on pi with tokenrhythm/deepseek-v4.1-flash-1017 (600K); reviewers on pi with tokenrhythm/deepseek-v4-pro-0813; quota lanes 2, temps 2 | why: user | else: -
- 2026-10-30 07:05 config: gear/voyages/M5.2/gear.toml, CONVENTIONS.md and REVIEW.md, copied from M5.1 | why: nothing the captain writes spans voyages | else: -
- 2026-10-30 02:53 release: v0.5.6 published: T-443's merge c5ce5c68 merged into main, tagged v0.5.6, GitHub release from docs/releases/v0.5.6.md (T-441, T-442); the shared engine runs it since 07:53Z; on the other project's store the board now shows M1.1 (246 done) instead of the plan-less M1.1-cancelled, checked with `gear board --json` and on its board pane | why: done clauses (2) and (3)
- station M-6420: keep course 39 min in; all three tasks merged and v0.5.6 runs; the landing waits for the user's word
- station M-6426: keep course; every M5.2 task merged and v0.5.6 runs on the shared engine; the landing and the lanes' retirement wait for the user's word
- station M-6427: keep course; same as M-6426

# Queue

## T-441 [now] lane=mast milestone=M5.2 required
goal: [v0.5.6] With no voyage sailing, the board shows the last voyage the plan described, not a milestone no plan described (another project, 2026-10-30: after its M1.1 landed the board showed `M1.1-cancelled`, a milestone holding one cancelled task with no title and no done sentence, because it departed after M1.1: "the voyage's time is unknown", 0/0 done, "done sentence 0 clauses"). Change crates/gear-board/src/lib.rs fn workload_voyage only, no other design: the fallback after the active voyages keeps only reports whose `done` is Some and not empty (a voyage the plan described), then takes the latest departure as today; when none is described it falls back to today's choice, so a project with only plan-less milestones still shows one. The voyage list for `[` and `]` and a voyage named with a milestone are unchanged. The code change is in lib.rs only; tests go where the crate keeps them.
accept: A board test: a store with a landed described voyage M1 and a later-departed milestone M1-x that has one cancelled task and no done sentence shows M1 (its title, its tasks, its done clauses); with M1-x alone it shows M1-x as today; an active voyage still wins. Run only `make check CRATE=gear-board TESTS='<a nextest filter: the workload voyage tests>'`; the merge gate runs `make gate`.
estimate: 20

## T-442 [now] lane=hull milestone=M5.2 required
goal: [v0.5.6] Plan apply refuses a task whose milestone no plan describes, so a typo or a made-up milestone never becomes a voyage (the same project's `M1.1-cancelled`: a task block moved T-70 to `milestone=M1.1-cancelled`, which no `# Milestone` block describes; apply created a voyage row and a departure for it, and that phantom voyage later blanked the board). Change crates/gear-app/src/plan.rs only, no other design: before anything is written, a `NEW` block, or an existing task's block that changes its milestone, whose `milestone=` names a milestone that has neither a `# Milestone <id>` block in this plan file nor a voyage row with a non-empty done sentence in the store, is refused with `bad(<the block's line>, "<task>: milestone <id> is not described: add a `# Milestone <id>` block with its done, bar and eta, or use a milestone the plan describes")`; tasks already in such a milestone that the plan does not move are left as they are (history). Cancelling a task keeps its milestone. The code change is in plan.rs only; tests go where the crate keeps them.
accept: Plan tests: a NEW block with an undescribed milestone is refused with the text and nothing is written (no task, no voyage row); a block moving a task to an undescribed milestone is refused the same way; a NEW block in a milestone described in the file, or in the store, applies; an existing task already in an undescribed milestone and not moved applies untouched. Run only `make check CRATE=gear-app TESTS='<a nextest filter: the plan apply tests>'`; the merge gate runs `make gate`.
estimate: 25

## T-443 [must] lane=mast milestone=M5.2 required
goal: [v0.5.6] Release v0.5.6, M5.2's release. Change, no other design: (1) the version is 0.5.6 in the workspace Cargo.toml (`[workspace.package] version`), Cargo.lock (the workspace crates' entries, as `cargo build` rewrites them), herdr-plugin.toml and every version string README.md names; (2) docs/releases/v0.5.6.md, in the form of docs/releases/v0.5.5.md: one line per task merged on origin/dev since v0.5.5 when you start your final check (run git log origin/dev --grep='^merge(T-' then), its T-number and what it changed for a user, from its goal; then "Skill": "Nothing outside the tasks above."; "Upgrading" as in v0.5.5. The captain merges dev into main, tags v0.5.6 and publishes the release; this task does not. The code change is in these files only.
accept: `cargo build --release -p gear-cli` then `target/release/gear --version` prints "gear 0.5.6"; docs/releases/v0.5.6.md lists every task merged on origin/dev since v0.5.5 at the hand-in. Run only `make check CRATE=gear-cli`; the merge gate runs `make gate`.
deps: T-441, T-442
estimate: 15
