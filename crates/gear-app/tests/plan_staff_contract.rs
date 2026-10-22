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
