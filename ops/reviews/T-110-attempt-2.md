# Review of T-110

Status: done
Verdict: fail
Reviewed: base/t110 @ d90f83ea39e0224cf197511590f405eb9682418b   Reviewer: Codex (GPT-6)   Round: 2

## Mechanical acceptance
- Build: `env -u GEAR_DB -u GEAR_CTX cargo build --workspace --all-targets` → exit 0, zero warnings.
- Tests: `env -u GEAR_DB -u GEAR_CTX cargo test --workspace` → exit 0, 835 passed, 0 failed. All 155 engine unit tests passed, including round 1's failing merge tests. Output retained during review at `/tmp/t110-review-attempt2-tests.log`.
- Commands: all documented commands parse with `zsh -n -c`. The built candidate binary executed `gear admin stop` against a scratch store (exit 0, engine not running) and `gear admin install-skill --agent claude`, `codex`, `opencode`, and `grok` in a scratch HOME (all exit 0, skill files written). These calls had no seat context. Candidate and inspected dev have identical product code.
- Herdr's installed help accepts `plugin install rogerdehe/gear --ref v0.3.0` and `plugin link .`; the manifest and fetch-or-build script support the stated replacement paths. Actual plugin replacement and checkout pull were not executed against the running voyage. The documented `tail -f` opened the actual engine log, whose recent output contained the stated `merged: [...]` field; the reader was then terminated.
- Structure/links: all three parts exist; README links to `docs/releases/v0.3.0.md#upgrading`; the return link targets README's Install section.
- Allowed files: ok. Candidate-originated changes (`git diff --name-only dev...d90f83e`) are README.md, docs/releases/v0.3.0.md and ops/reports/T-110.md. The requested two-dot diff additionally includes integration-only plan changes absent from this older candidate; these are not candidate deletions.
- Merge: `git merge-tree --write-tree dev d90f83ea39e0224cf197511590f405eb9682418b` → exit 0, clean against inspected dev 8742355; `git diff --check dev...d90f83e` → clean.

## Spec compliance
- The three procedures and requested migration, skill restart and changed-behavior topics are present, with the README link.
- F2 is resolved: the checkout command uses `.` and parses. F3 is resolved: the guide identifies the user's shell for stop/install. F4 is resolved for this candidate: the full workspace suite passes independently.
- F1 is partially corrected: merge completion is now checked in the real engine log rather than inferred from the task page. However, completion of the last task seen passing does not establish that every merge is finished. The running-voyage precondition remains unsatisfied in normal multi-repository operation.

## Findings
F1 [P1, blocking] Waiting only for the last task seen passing can still stop an active merge — `docs/releases/v0.3.0.md:89`. Dev runs a worker per repository (`crates/gear-engine/src/merge.rs:243-248`, `:285-307`) and collects each finished worker independently (`:164-180`); completion order is not pass order. In the milestone's parent/child project case, task A can pass first and run a slow gate in one repo, while task B passes later and finishes its merge in another repo. B's merge line satisfies the documented stop condition while A is still executing. This is established by the worker selection/collection code on both dev and the candidate; this review did not stop live workers to demonstrate it. It preserves round 1's unsafe stop condition, breaks the goal's no-merge-in-flight requirement, and breaks the DELIVERABLE bar's normal-use correctness. Smallest fix: in the running-voyage section, replace the singular last-task condition with checking completion (or terminal merge-failure item) for every outstanding passed task in every repo, including any that pass while waiting; immediately before stopping, check that no newly passed task remains outstanding. Keep the existing log command, and make the same correction to the F1 explanation in `ops/reports/T-110.md` (about 4–6 documentation lines total). Do not add product code or a new status command in this docs task.
F5 [P2, logged] The log reader can hide the completion it asks the user to find — `docs/releases/v0.3.0.md:93`. `tail -f` initially prints only ten lines. If the needed merge line is older, the command waits for future output while that task's completion will never be emitted again. This is a diagnostic inconvenience, not another blocking finding under the milestone bar. Use `tail -n +1 -f ~/.local/state/gear/engine.log`, or explicitly tell the reader to search existing log contents before following new lines; retain the actual log location next to the selected store.

## Set aside
- The task page still cannot prove that no merge is running. Using supported log evidence is the appropriate documentation-only correction suggested in round 1; the remaining blocker is specifically the claim that the latest passing task proves global completion.
- Store migration and refusal of newer stores are supported by Store::open and the passing store tests. No manual store edit or downgrade is prescribed.
- The default log path matches default_store_in and engine startup. A custom GEAR_DB puts the log beside that store, as the prose says; this override is an edge case and does not block.
- T-103/T-104/T-107 release behavior is required by the contract and unchanged in this delta. Some dependencies are still absent from the inspected product snapshot (including push allow-list removal); the captain must integrate them before the promised end-of-M3 upgrade check. No unrelated implementation is assigned to this docs writer.
- The new-user paragraph and step wrapping are unchanged from attempt 1 and do not justify another finding. Restarting lanes between tasks, never mid-task, follows the contract.
- No live engine was stopped, agent restarted or plugin replaced. The planned end-of-M3 live upgrade remains captain acceptance, rather than a claim made by this review.
- The existing remote review/t110 contained the first review. It was merged into this review branch to preserve that history and allow an ordinary push; the only differences from the candidate on this branch are review artifacts.

## Questions for the captain
- Ensure the release dependencies are integrated and perform the specified live upgrade acceptance at the end of M3. No decision is needed to make the F1 documentation correction.

## Verdict

Fail. F1 blocks because the revised stop condition still permits stopping during normal merging. F5 is logged. F2, F3 and F4 are resolved; the build, tests and merge checks pass.
