# 05 Plan: Draft Format, Apply, Views

Implementation: [draft.rs](../../crates/gear-app/src/draft.rs), [plan.rs](../../crates/gear-app/src/plan.rs).
Invariants upheld: single authoritative plan; an invalid draft is rejected in full; completion requires a passing acceptance verdict; a draft cannot overwrite runtime facts.

## Plan format and files

The current format and voyage file workflow are in `gear --skill captain`
([captain.md](../../skills/gear/references/captain.md)). The parser lives in
[draft.rs](../../crates/gear-app/src/draft.rs); use these instead of the old
exported `plan.draft.md` and generated `PLAN.md` workflow.

New task blocks require `estimate:` in minutes. Existing tasks retain their
estimate unless the draft changes it; time reports compare the planned time
with the recorded attempt lifecycle times (T-38).

## Apply flow (`gear plan apply`, one transaction, via run_request)

1. Parse → any format error is `BAD_FORMAT` (with line number), nothing is written.
2. Identity: captain seat, generation matches, not in HANDOVER/UNAVAILABLE.
3. `base` ≠ current plan version → `STALE_VERSION`, returning a summary of changes since base.
4. Validation: dependencies exist and are acyclic; wont is not dispatched; Verdicts rules; resume rules; park/hold/retire only target this project's long-lived lanes.
5. Write: contract changes → new version (an in-progress attempt gets a pending-adoption notice, 06); rank; milestones and required; park/hold/retire change seat state directly in the transaction (pure data); temp worker starts/resumes are written as operations(INTENDED); Verdicts → 06 `settle`; Review conclusions → reviews; plan version +1; audit.
6. Return: new version number, change list, pending staff actions, queued items, tasks not mentioned, warnings.

## Views

`gear --show T-<n>` renders the task packet with its adopted contract and the
exact reporting commands. `gear board` displays the plan. Rendering lives in
[plan.rs](../../crates/gear-app/src/plan.rs) and [gear-board](../../crates/gear-board/src/lib.rs).

## Test points

Line numbers for format errors; an illegal state rejects the whole draft; stale base; dependency cycle; unmentioned tasks keep relative order; Verdict with mismatched candidate/contract is rejected; evidence file exists but verdict fail does not allow DONE; resume used for a new task is rejected; the same Staff target applied multiple times does not open twice; resubmitting a historical Verdict does not settle twice; fail needs no evidence header; changing the contract and accepting in the same apply validates against the final contract.

### Tail declaration (T-40)

A decision line `- tail: heat wire` replaces the set of lanes intentionally
finishing their queue; `- tail: -` clears that set. A draft without a tail line
preserves it. Names must resolve to lane seats in the project; an invalid name
rejects the whole apply. The state is persisted in `meta` under
`lane-tail:<project>:<lane>` and suppresses feed warnings without parking lanes.

### Station decisions (T-49)

A decision `- station M-123: keep course|time-box|cut|move [reason]` closes the
named station action item in the same transaction as the plan apply. The four
forms are alternatives. An invalid action or non-station/unknown reference
rejects the entire plan; a retained decision for an already resolved station is
a no-op. Reply resolve/wait cannot close or defer a station; notes keep it open.
