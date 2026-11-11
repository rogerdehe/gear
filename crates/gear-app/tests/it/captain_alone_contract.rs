//! A project run by its captain alone (M7.2): no lanes and no reviewers; the
//! captain plans, works and records its own tasks.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

const ALONE: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 0
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"
"#;

const WITH_A_LANE: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#;

fn world(config: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
        project.config_toml = config.to_string();
        tx.update_project(&project)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "claude",
            NOW,
        )?;
        Ok(())
    })
    .unwrap();
    s
}

fn run(s: &mut Store, body: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n{body}");
    let env = Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    };
    let fp = Fingerprint {
        pane_id: CAP_PANE.into(),
        kind: Some("claude".into()),
        session_ref: None,
    };
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(&text).unwrap(),
        &text,
        "r1",
        NOW + 10,
    )
}

const NO_LANE: &str = "# Queue\n## NEW [must]\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";

#[test]
fn a_captain_alone_plans_a_task_without_a_lane() {
    let mut s = world(ALONE);
    let r = run(&mut s, NO_LANE).unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    let task = s.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
    assert_eq!(task.lane, None, "the task is the captain's");
}

#[test]
fn a_project_with_lanes_still_needs_a_lane() {
    let mut s = world(WITH_A_LANE);
    let err = run(&mut s, NO_LANE).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat);
    assert!(err.detail.contains("needs `lane=`"), "{}", err.detail);
}
