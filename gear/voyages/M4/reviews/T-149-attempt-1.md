# Review of T-149

Status: done
Verdict: fail
Reviewed: keel/t149 @ ec3cec3   Reviewer: gear:review-10 (Codex)   Round: 1

## Mechanical acceptance
- Build: `cargo build --workspace --all-targets` exited 0 with zero compiler warnings/errors.
- Tests: `scripts/test.sh` exited 0; 949 passed, 0 skipped, 0 failed. Nextest measured 70.949 seconds; script measured 72 seconds, below the 300-second red line. Test compilation took 53 seconds. All five `targeting_contract` cases passed. There were 19 existing tests over 10 seconds and the suite target warning; these are runtime warnings, not build warnings.
- Independent supplemental probes: compiled a temporary copy of `targeting_contract.rs` against the candidate's built libraries, adding the two tests reproduced below. Both failed their contract assertions: the refused live-agent delivery became Failed, and the sender prompted immediately after renaming. Repository source files were not modified.
- Allowed files: contract v1 gives no path allowlist. Inspected all 19 paths in the requested `dev..ec3cec3` diff; targeting implementation/tests and report are the task changes, with T-139 implementation/review artifacts and a rendered voyage snapshot also present in the supplied comparison. No path-based mechanical failure.
- Merge: `git merge-tree --write-tree dev ec3cec3` exited 0 (tree `67a04ca01d5d07fd3eb17d8a02bb7d142deaf1b7`). After fetching, `git merge-tree --write-tree origin/dev ec3cec3` also exited 0 (tree `aba3c8550ea2d00511b12801589b21335cc1a83f`); fetched integration was `2e243490807c6046628225f946388e39b4834575`.
- `git diff --check dev..ec3cec3` passed. Review commits change only this review artifact.

## Spec compliance
- The direct engine delivery path prompts by the observed agent name, restores an unnamed live agent without restarting it, and leaves that op Intended for another execution. A shell-only or different-kind occupant fails immediately with `agent-gone`. The real `pane_run` checks `agent_get` and refuses an agent-bearing pane; the fake models that refusal.
- Design §10.1 is not implemented exactly: the trait documentation invents a pane-id exception for lifecycle commands; refusal recovery depends on an additional name-list test instead of the required `agent_get` decision; the sender bypasses the required next-tick retry for an unnamed agent. These omissions are findings F1–F3.

## Findings
F1 [P1, blocking] Lifecycle prompts still use pane ids, preserving the wrong-target hazard expressly prohibited by §10.1 — `crates/gear-herdr/src/lib.rs:201`, `crates/gear-engine/src/onboard.rs:441`, `crates/gear-engine/src/restart.rs:318`, `crates/gear-engine/src/release.rs:371`. The changed trait comment explicitly permits pane ids for commands, but the design says `HerdrAct::prompt` targets the live agent name, never the pane id, without this exception. Normal onboarding retry, restart and retirement can send an exit command through these paths; an agent exiting between the observation/ensure call and the prompt leaves the command aimed at its shell. The writer's report acknowledges the remaining stray `/exit` hazard and defers it based on file ownership, which does not narrow contract v1. Breaks the STABLE bar's requirement to fix normal-use stability problems and the task's wrong-target safety requirement supporting v0.4.0 session/reviewer lifecycle. Smallest fix: in these three call sites resolve the occupant's live name and prompt that name, withholding the command if identity/name cannot be established; reuse/expose the targeting helper rather than retaining pane-id fallback, and remove the trait exception (roughly 30–50 implementation lines). Add fake-Herdr assertions that these exit-command paths target a name and do not prompt a shell.
F2 [P1, blocking] A refused prompt to a still-live named agent fails instead of renaming and retrying next tick — `crates/gear-engine/src/ops.rs:517`. Reproduction: create the named idle opencode lane and Intended delivery using `targeting_contract` helpers; inject `Refuse { code: "agent_not_ready" }`; call `execute_intended`. `agent_get` confirms the correct kind, but the extra `agent_names` check sees `Some("p-heat")` and returns Refused; the stored result is `(Failed, Some("agent_not_ready: injected"))`, with no rename/retry. A transient refusal immediately after an agent start is an ordinary supported Herdr condition (also documented in this candidate's refusal-code table). §10.1 explicitly makes the live-kind `agent_get` result select rename-and-retry, so the name-list gate can strand a delivery rather than recover it. Breaks the STABLE bar's normal-use reliability requirement supporting v0.4.0 unattended session/reviewer operation. Smallest fix: in `aim` remove the additional lost-name gate and use the existing rename-and-Retry branch whenever the post-refusal `agent_get` confirms the seat's kind (about 10–15 lines removed); retain Gone for absent/wrong-kind agents and Retry for unreadable observations. Add the reproduction as a contract test, asserting Intended, no restart, and successful submission on the next execution.
F3 [P1, blocking] The sender gives unnamed-agent deliveries a same-call exception and skips refusal recovery — `crates/gear-engine/src/ops.rs:561` (enabled by `deliver_doorbell_now` at line 137). Reproduction: use the active `p:heat` binding, add its unnamed idle opencode agent, then call `deliver_doorbell_now(..., "M-1", true, now)`; the call renames it and immediately prompts `p-heat`, returning a submitted delivery, rather than leaving the op Intended and typing nothing until next tick. If that second prompt is refused, this branch also omits the required `agent_get` decision and can fail without the `agent-gone` reason. Contract v1 and §10.1 provide no sender exception; the report's T-130 justification does not supersede this contract's explicit next-tick recovery. Breaks the task's unnamed-agent acceptance behavior and the STABLE bar's requirement for consistent normal-use delivery recovery. Smallest fix: remove `type_after_rename` and its immediate-prompt branch, returning the existing Retry result for every `Target::Renamed` (roughly 15–20 lines removed, plus the two call arguments). Preserve immediate sender delivery for an already-named agent. Add the sender reproduction as a test: no prompt on first call, Intended op, then one prompt by name on the next execution, without restart.

## Set aside
- The full tick's pre-existing name-restoration phase can rename before dispatch. The supplied direct-helper acceptance tests pass; however, that does not exercise the refusal decision or justify the explicit sender exception in F2/F3. The existing full-tick test still asserts same-tick delivery after early restoration; it is not evidence for post-refusal recovery.
- FakeHerdr still accepts pane-id prompt selectors and logs attempted refused calls. This is useful for detecting F1 through `prompt_targets`; a logged attempted call alone is not proof text reached a shell. The unsafe selectors are established by the production call sites.
- Shell `agent_get`/`pane_run` has an observation-to-action window. The contract specifically prescribes this guard, and the real adapter implements it; no stronger atomic shell API is required by this review.
- Broader session restart/reuse, reviewer checkout redesign (§3.7), quota, board, reporting, repo rebinding and release work belongs to the voyage's other tasks. This review does not require those features here. It does require all prompt call sites to honor §10.1, whose wording is explicit.
- T-139 changes in the supplied diff implement its separate reviewed mail/reply task. No new defect found in that inherited work. The rendered voyage snapshot differences are not treated as intentional task-policy changes.
- Runtime warnings come from unchanged merge/review/parallel fixtures, already identified for separate work in the voyage's decisions. The new targeting tests each took under one second; the full suite stayed below the mandatory 300-second limit. No duplicate timing finding.

## Questions for the captain
None. Contract v1 explicitly requires §10.1 exactly; file ownership and the earlier sender behavior do not authorize its exceptions.

## Reproduction details
The supplemental tests below were appended to a temporary copy of `crates/gear-engine/tests/it/targeting_contract.rs`, keeping its existing `world`, `intended_delivery`, `op_state`, `T0` and `LANE_PANE` helpers. They link the reviewed libraries and do not alter candidate code.

```rust
#[test]
fn review_refusal_with_live_named_agent_retries_next_tick() {
    let mut store = world();
    let id = intended_delivery(&mut store);
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(LANE_PANE, "opencode", AgentStatus::Idle, "p-heat");
    herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse {
        code: "agent_not_ready".into(),
    });
    execute_intended(&mut store, &mut herdr, "p", T0 + 10).unwrap();
    assert_eq!(op_state(&mut store, &id).0, OpState::Intended);
    // Actual: Failed; result "agent_not_ready: injected".
}

#[test]
fn review_sender_unnamed_agent_waits_until_next_tick() {
    let mut store = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    gear_engine::ops::deliver_doorbell_now(
        &mut store, &mut herdr, "p", "p:heat", 1, "M-1", true, T0 + 10,
    ).unwrap();
    assert!(herdr.prompt_targets().is_empty());
    // Actual: ["p-heat"], prompted in this call after the rename.
}
```

## Verdict
Fail. The supplied mechanical gate passes, but F1, F2 and F3 block contract v1 under the STABLE bar. All three have bounded fixes and reproduction/validation directions for one rework round.
