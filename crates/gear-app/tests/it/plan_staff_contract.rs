//! Contract for `# Staff` park/hold/retire in plan apply (T-55).

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole, SeatState};
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
        for (name, pane) in [("heat", "w1:p2"), ("wire", "w1:p3"), ("prompts", "w1:p4")] {
            testing::seed_active_seat(tx, "p", name, SeatRole::Lane, pane, "opencode", T0)?;
        }
        Ok(())
    }).unwrap();
    s
}

fn run(s: &mut Store, staff: &str, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let text = format!("<!-- gear-plan project=p base=0 -->\n# Staff\n{staff}");
    let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP.into()), admin: false };
    let fp = Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None };
    apply(s, &env, Some(&fp), None, &parse(&text).unwrap(), &text, rid, T0 + 10)
}

fn seat(s: &mut Store, name: &str) -> (SeatState, Option<String>) {
    let r = s.read(|tx| tx.get_seat(&format!("p:{name}"))).unwrap().unwrap();
    (r.state, r.state_reason)
}

#[test]
fn park_hold_retire() {
    let mut s = world();
    let r = run(&mut s, "- park heat until-task=T-12\n- hold wire reason=waiting for user authorization\n- retire prompts\n", "r1").unwrap();
    assert_eq!(r.staffed, vec!["park heat".to_string(), "hold wire".to_string(), "retire prompts".to_string()]);
    assert_eq!(seat(&mut s, "heat"), (SeatState::Parked, Some("until-task=T-12".into())));
    assert_eq!(seat(&mut s, "wire"), (SeatState::Held, Some("hold: waiting for user authorization".into())));
    assert_eq!(seat(&mut s, "prompts"), (SeatState::Retiring, Some("retire".into())));
}

#[test]
fn bad_staff_writes_nothing() {
    let mut s = world();
    for (rid, line) in [
        ("a", "- park nobody until-task=T-1\n"),                 // unknown seat
        ("b", "- park captain until-task=T-1\n"),                // not a lane
        ("c", "- temp R-4 task=T-45 profile=opencode-review\n"), // not in this cut
    ] {
        let staff = format!("- hold wire reason=x\n{line}");
        assert_eq!(run(&mut s, &staff, rid).unwrap_err().code, ErrorCode::BadFormat, "{rid}");
    }
    assert_eq!(seat(&mut s, "wire").0, SeatState::Active, "the valid line before the bad one is not applied either");
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 0);
}

/// T-82 (trial G02, G04): a lane the engine parked can be retried from the
/// plan, and a running lane cannot.
#[test]
fn retry_puts_a_parked_lane_back_in_the_queue() {
    // `run_at` applies against the plan version the store holds.
    fn run_at(s: &mut Store, staff: &str, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
        let base = s.read(|tx| tx.plan_version("p")).unwrap();
        let text = format!("<!-- gear-plan project=p base={base} -->\n# Staff\n{staff}");
        let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP.into()), admin: false };
        let fp = Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None };
        apply(s, &env, Some(&fp), None, &parse(&text).unwrap(), &text, rid, T0 + 10)
    }

    let mut s = world();
    // The engine parked `heat` in NeedsCheck with the step's reason.
    s.write(|tx| {
        tx.set_seat_state("p:heat", SeatState::NeedsCheck, Some("agent_start: refused (agent_pane_busy)"), T0 + 1)
    })
    .unwrap();

    let r = run_at(&mut s, "- retry heat\n", "retry-1").unwrap();
    assert_eq!(r.staffed, vec!["retry heat".to_string()]);
    assert_eq!(
        seat(&mut s, "heat"),
        (SeatState::Queued, Some("retried by the captain".into())),
        "the engine onboards it again"
    );

    // A lane that is running cannot be retried: the transition says no, with
    // the code the acceptance names.
    let error = run_at(&mut s, "- retry wire\n", "retry-2").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition);
    assert!(error.detail.contains("p:wire"), "{}", error.detail);
    assert_eq!(seat(&mut s, "wire").0, SeatState::Active, "nothing written");

    // A lost lane (Unknown) is retried too; a queued one is already there.
    s.write(|tx| tx.set_seat_state("p:prompts", SeatState::Unknown, Some("the pane is gone"), T0 + 2))
        .unwrap();
    run_at(&mut s, "- retry prompts\n", "retry-3").unwrap();
    assert_eq!(seat(&mut s, "prompts").0, SeatState::Queued);
    let r = run_at(&mut s, "- retry heat\n", "retry-4").unwrap();
    assert_eq!(r.staffed, vec!["retry heat".to_string()], "a queued lane is a no-op");

    // A lane that does not exist, and a seat that is not a lane, are refused.
    assert_eq!(run_at(&mut s, "- retry nobody\n", "retry-5").unwrap_err().code, ErrorCode::BadFormat);
    assert_eq!(run_at(&mut s, "- retry captain\n", "retry-6").unwrap_err().code, ErrorCode::BadFormat);
}

/// T-140: a reviewer seat in Standby is released by the captain's line, and a
/// reviewer that is reviewing is refused with the task it is reviewing.
///
/// A reviewer release needs no session fact (the seat is idle or it is
/// refused), so it works without the session's agents.
#[test]
fn release_brings_a_standby_reviewer_back() {
    /// `run_at` applies against the plan version the store holds.
    fn run_at(s: &mut Store, staff: &str, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
        let base = s.read(|tx| tx.plan_version("p")).unwrap();
        let text = format!("<!-- gear-plan project=p base={base} -->\n# Staff\n{staff}");
        let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP.into()), admin: false };
        let fp = Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None };
        apply(s, &env, Some(&fp), None, &parse(&text).unwrap(), &text, rid, T0 + 10)
    }

    let mut s = world();
    s.write(|tx| {
        // A reviewer in a job: it is reviewing T-12.
        testing::seed_temp_seat(tx, "p", "review-1", "T-12", "w1:p5", "codex", T0)?;
        // A reviewer without a job: the engine left it in Standby.
        testing::seed_active_seat(tx, "p", "review-2", SeatRole::Temp, "w1:p6", "codex", T0)?;
        tx.set_seat_state("p:review-2", SeatState::Standby, Some("review done"), T0 + 1)
    })
    .unwrap();

    // The idle reviewer goes: the engine closes its pane as for any release.
    let r = run_at(&mut s, "- release review-2\n", "release-1").unwrap();
    assert_eq!(r.staffed, vec!["release review-2".to_string()]);
    assert_eq!(
        seat(&mut s, "review-2"),
        (SeatState::Releasing, Some("released by the captain: it was idle".into())),
    );

    // A reviewer that is reviewing is not pulled out of its job.
    let error = run_at(&mut s, "- release review-1\n", "release-2").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition);
    assert!(
        error.detail.contains("review-1 is reviewing T-12"),
        "{}",
        error.detail
    );
    assert_eq!(seat(&mut s, "review-1").0, SeatState::Active, "nothing written");

    // A reviewer that is not idle is refused with the reason: it is not in
    // Standby, so the captain cannot let it go.
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "review-3", SeatRole::Temp, "w1:p7", "codex", T0)?;
        Ok(())
    })
    .unwrap();
    let error = run_at(&mut s, "- release review-3\n", "release-3").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition);
    assert!(
        error.detail.contains("only a reviewer in Standby is released"),
        "{}",
        error.detail
    );
    assert_eq!(seat(&mut s, "review-3").0, SeatState::Active, "nothing written");

    // A seat that is neither a lane nor a reviewer is refused, and so is a
    // name that does not exist.
    assert_eq!(run_at(&mut s, "- release captain\n", "release-4").unwrap_err().code, ErrorCode::BadFormat);
    assert_eq!(run_at(&mut s, "- release nobody\n", "release-5").unwrap_err().code, ErrorCode::BadFormat);
}

/// T-133: a held lane comes back from the plan — `Active` while its agent is
/// still there, `Queued` (the engine onboards it again) once it is gone — and a
/// lane that is not held is refused with the reason.
///
/// Review F1: the store keeps an exited agent's binding Active, so the session's
/// agents decide; without them the release is refused, never guessed.
#[test]
fn release_brings_a_held_lane_back() {
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::{AgentStatus, HerdrRead as _};

    // `run_at` applies against the plan version the store holds, with the
    // session's agents as the caller read them.
    fn run_at(
        s: &mut Store,
        herdr: &mut FakeHerdr,
        staff: &str,
        rid: &str,
    ) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
        let agents = herdr.agents().unwrap();
        run_with(s, Some(&agents), staff, rid)
    }

    fn run_with(
        s: &mut Store,
        agents: Option<&[gear_herdr::AgentInfo]>,
        staff: &str,
        rid: &str,
    ) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
        let base = s.read(|tx| tx.plan_version("p")).unwrap();
        let text = format!("<!-- gear-plan project=p base={base} -->\n# Staff\n{staff}");
        let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some(CAP.into()), admin: false };
        let fp = Fingerprint { pane_id: CAP.into(), kind: Some("codex".into()), session_ref: None };
        apply(s, &env, Some(&fp), agents, &parse(&text).unwrap(), &text, rid, T0 + 10)
    }

    let mut s = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Idle); // heat
    herdr.add_agent("w1:p3", "opencode", AgentStatus::Idle); // wire

    // The captain holds a lane whose agent keeps running.
    run_at(&mut s, &mut herdr, "- hold heat reason=waiting for the user\n", "hold-1").unwrap();
    assert_eq!(seat(&mut s, "heat").0, SeatState::Held);

    // Release it: an agent of its binding's kind is in the binding's pane, so
    // the engine continues with it.
    let r = run_at(&mut s, &mut herdr, "- release heat\n", "release-1").unwrap();
    assert_eq!(r.staffed, vec!["release heat".to_string()]);
    assert_eq!(
        seat(&mut s, "heat"),
        (SeatState::Active, Some("released by the captain: its agent continues".into())),
        "the agent continues"
    );

    // Review F1: the agent exits and its binding stays Active. The lane is
    // released to the queue, so the engine onboards it again.
    run_at(&mut s, &mut herdr, "- hold wire reason=the machine is recovering\n", "hold-2").unwrap();
    herdr.exit_agent("w1:p3");
    let binding = s.read(|tx| tx.get_binding("p:wire", 1)).unwrap().unwrap();
    assert_eq!(binding.status, gear_store::BindingStatus::Active, "the exited agent keeps its binding");
    run_at(&mut s, &mut herdr, "- release wire\n", "release-2").unwrap();
    assert_eq!(
        seat(&mut s, "wire"),
        (SeatState::Queued, Some("released by the captain: its agent is gone".into())),
        "the engine onboards it again"
    );

    // An unread session is not "gone": the release is refused and the lane stays
    // held (prompts has no agent in its pane either).
    run_at(&mut s, &mut herdr, "- hold prompts reason=the machine is recovering\n", "hold-3").unwrap();
    let error = run_with(&mut s, None, "- release prompts\n", "release-3").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(error.detail.contains("the session's agents were not read"), "{}", error.detail);
    assert_eq!(seat(&mut s, "prompts").0, SeatState::Held, "nothing written, still held");

    // Only a held lane is released: a running one is refused with the reason.
    let error = run_at(&mut s, &mut herdr, "- release heat\n", "release-4").unwrap_err();
    assert_eq!(error.code, ErrorCode::BadTransition);
    assert!(error.detail.contains("p:heat"), "{}", error.detail);
    assert!(error.detail.contains("only a lane in Held is released"), "{}", error.detail);
    assert_eq!(seat(&mut s, "heat").0, SeatState::Active, "nothing written");

    // A lane that does not exist, and a seat that is not a lane, are refused.
    assert_eq!(run_at(&mut s, &mut herdr, "- release nobody\n", "release-5").unwrap_err().code, ErrorCode::BadFormat);
    assert_eq!(run_at(&mut s, &mut herdr, "- release captain\n", "release-6").unwrap_err().code, ErrorCode::BadFormat);
}
