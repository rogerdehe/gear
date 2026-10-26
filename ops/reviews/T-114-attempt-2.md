# Review of T-114

Status: done
Verdict: pass
Reviewed: review/t114 @ 3ce9242a592cd7c420011106ca7735ba7a0af6e9   Reviewer: Codex   Round: 2
Contract: v1

## Mechanical acceptance
- Build: `cargo build --workspace --all-targets` passed with zero warnings.
- Tests: `cargo test --workspace` passed: 863 passed, 0 failed; doc tests passed. Zero warnings. All 175 engine integration tests passed, including the four pi startup, entry-argument, quota-restart/redelivery and exhaustion cases.
- Allowed files: no explicit path allow-list in contract v1. The task adds or changes five files relative to the integration merge base: quota/switch logic, integration-test registration, pi contract tests, and the task report. All are in scope.
- Merges into dev: `git merge-tree --write-tree dev 3ce9242` and the same check against freshly fetched `origin/dev` both passed. Local dev was `fa7b7ae`; fetched origin/dev was `928d912`. Both resulting trees differ from their integration tips only in the five intended files. The apparent removals in the requested two-dot diff are integration-only commits and are preserved by the merge.
- `git diff --check dev..3ce9242` and the equivalent origin/dev check passed. Review branch changes contain only review artifacts; the candidate remains untouched.

## Spec compliance
- Attempt 1 had no findings. Reviewed the attempt-2 delta: T-113 is now merged, the duplicate model-start implementation/test is resolved in favor of dev, and the restart test accepts the supplied profile's exit command. Quota/switch production logic is unchanged since the earlier review.
- Pi starts with `--model <provider>/<id>` and entry arguments in its worktree. Both the OpenCode session API and model gate remain conditional on OpenCode. Pi needs no `[models.switch]` labels.
- Pi-to-pi fallback uses the durable restart path, closes through the supplied profile, waits for the old agent to leave, starts the next model, updates the generation and queues the open task's Continue delivery. Exhaustion parks the lane and alerts the captain.
- Deck's built-in pi profile is present and used. Its `/quit` exit command takes precedence over the test's patched exit keys; the updated assertion correctly allows that interface. CLI profile registration and pi startup support are included from T-113.
- The live pi ancestry evidence accepted in attempt 1 remains unchanged: bash -> pi PID 25308 -> pane shell PID 24605 -> Herdr, with both pi and shell PIDs matching process-info. This satisfies `identity::claims_pane`; pi does not need the OpenCode service exception. Reviewed the recorded evidence and code, without replaying the live probe.

## Findings
None. No blocking or logged defects found in the attempt-2 delta.

## Set aside
- The exit assertion now checks one closing call rather than hard-coding ctrl+d. Inspection confirms the production restart uses the profile's `/quit`; accepting deck's profile is required by the contract, so this is not a regression.
- Integration-only config, consent notes and backlog additions shown as removals by `dev..candidate` survive the tested merges. They are not candidate changes to revert or findings.
- The report's older out-of-scope notes about pi CLI registration describe the pre-T-113 split. Current code includes registration, and the report's opening section records the merge; no engine defect results.
- The full voyage trial, reviewer quota completion, batch-2 tasks and release remain milestone work beyond this engine task. No redesign of existing restart fault handling is needed under the DELIVERABLE bar.

## Questions for the captain
None.

## Verdict
pass — mechanical acceptance passed; no finding breaks the contract, done-sentence or DELIVERABLE bar.
