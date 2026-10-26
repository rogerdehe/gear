# Review of T-168

Status: done
Verdict: fail
Reviewed: review/t168 @ ff13cdb2ddb8b16db6c8c0d273c2a260d7377258   Reviewer: Codex   Round: 1

## Mechanical acceptance
- Build: `cargo build --workspace --all-targets` → clean, zero warnings.
- Tests: `scripts/test.sh` → 1063 passed, 0 failed, 0 skipped; nextest 59.308 s, script wall time 61 s (below the 300-second red line), build 70 s.
- Allowed files: no explicit path allowlist in contract v1. The T-168 changes implement the requested CLI/app behavior, plus the error code, test construction updates and report. The prescribed `dev..candidate` also includes already merged T-150 delivery-grace work; after `git fetch origin`, `origin/dev...candidate` contains only the T-168 paths.
- Merge: `git merge-tree --write-tree dev ff13cdb2ddb8b16db6c8c0d273c2a260d7377258` and the same check against fetched `origin/dev` → clean (tree c07d5e0eabe0cf29b001fb719ee1d299bf8176d3).
- `git diff --check dev..ff13cdb` → clean.
- Two isolated Rust probes linked against the candidate's compiled libraries, using the existing review-contract fixture, confirm F1 and F2. Probe source/binary live only in `/tmp`; candidate code was not edited.

## Spec compliance
- Implemented: result/review flag plumbing, transactional refusal and insertion through `insert_friction`, the specified error text, writer packet budget/help, open/all lists with round facts, and captain close.
- Missing/wrong: review time uses the writer's clock (F1); over-budget comparison loses seconds (F2); reviewer packets omit §6.2 budget/help (F3).

## Findings
F1 [P1, blocking] A review is charged the writer's elapsed time — `crates/gear-app/src/mail.rs:804`. `check_friction` runs for role `review`, but always reads `attempt_times.started` (the writer's check-in). Reproduction: start the writer at T0, insert a review round starting at T0+60 min, submit its verdict at T0+65 min with the default 30-minute review budget. The candidate refuses this five-minute review with `FRICTION_REQUIRED: T-1 took 65 min against 30 min`. Supplying friction also incorrectly stores `required=true`. This breaks §5.1/§6.2 and the STABLE bar's requirement to fix normal-use errors: reviewers must invent friction to hand in an under-budget review. Smallest fix: in `check_friction`, choose the review round's own check-in/delivery-confirmation clock for role `review` (using its round start as the available fallback), retaining the attempt check-in for dev/rework; subtract that role's pauses. About 10–20 lines plus a regression with distinct writer/reviewer start times. Never fall back to the writer's start for a review.

F2 [P1, blocking] A hand-in over budget by 1–59 seconds bypasses required friction — `crates/gear-app/src/mail.rs:816`. Active seconds are divided by 60 before `active > budget`. Independent probe: default 30-minute budget, confirmed start T0, result at T0+1801 seconds without friction → accepted as M-1 and no friction stored. With friction it would also incorrectly be classified optional. §6.2 requires the report whenever `active_s` exceeds the budget; this is a normal boundary case and breaks the STABLE correctness bar. Smallest fix: keep elapsed active seconds, compare them against `budget * 60`, and convert only for the error message (about 3–5 lines). Add checks at exactly 1800 seconds and at 1801 seconds, including `required=true` with a report.

F3 [P1, blocking] Reviewer delivery packets omit the budget and required flag instructions — `crates/gear-engine/src/review.rs:1236` (`review_packet_for`, and the template at line 1427). Only the writer's `render_task_packet` gains the §6.2 line. Reviewers receive their bespoke packet, which contains neither a review budget nor instructions to carry friction past it, even though their hand-in now enforces this requirement. The T-168 report calls this out as outside the lane's files, but contract v1 requests the packet line without a reviewer exception, and §6.1 explicitly includes reviewers. This breaks the STABLE usability bar: a normal reviewer discovers the new hand-in requirement only through refusal. Smallest fix: in `review_packet_for`, obtain the assigned reviewer model/round, call `round_budget_min(..., "review", model)`, and append the same budget/friction guidance and §6.1 examples (about 10–20 lines); assert the delivered reviewer packet contains its review budget and flag. Coordinate the shared engine file rather than changing unrelated reviewer behavior.

F4 [P2, logged] Close accepts strings that merely start with an outcome name — `crates/gear-app/src/friction.rs:84`. For example, `fixedwhatever` passes `starts_with("fixed")`, and bare `fixed` closes the report without saying what changed. The specified outcome is fixed (what), lesson (which), or not worth it (why). This requires malformed captain input and is outside normal use, so it does not block under the bar. A small follow-up can check an outcome word boundary and nonempty explanatory text before closing.

## Set aside
- T-150 delivery-grace changes in the prescribed diff have their own completed reviews and are already on fetched `origin/dev`; they are not new T-168 scope expansion.
- Engine friction notifications/item lifecycle and board rendering are explicitly outside the CLI half of §6.4 and this task's CLI scope.
- Round/token collection and pause accounting are separate engine work; this review checks their consumption here rather than requiring their implementation in T-168.
- Free-form friction prose is not semantically classified: the design describes useful content but does not specify an automatic classifier.
- The unchanged `gear-cli::it fixtures_contract::the_fixtures_build_replace_and_pass_their_gates` took 12.4 s and emitted a slow-test warning; per `ops/REVIEW.md`, an untouched slow test is recorded here rather than treated as a failed mechanical fact of this task. The script also warned at 61 s total wall time; the complete suite remains below the mandatory 300-second red line.

## Questions for the captain
- None. Contract v1 has no documented exemption for reviewer packet guidance.

## Verdict
Fail: F1, F2 and F3 block under the STABLE bar. F4 is logged only.
