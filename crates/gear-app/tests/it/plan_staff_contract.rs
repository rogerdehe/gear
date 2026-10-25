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
    apply(s, &env, Some(&fp), &parse(&text).unwrap(), &text, rid, T0 + 10)
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
        apply(s, &env, Some(&fp), &parse(&text).unwrap(), &text, rid, T0 + 10)
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
