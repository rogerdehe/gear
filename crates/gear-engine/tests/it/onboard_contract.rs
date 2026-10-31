//! Contract for gear_engine::onboard::plan_onboard (T-58). Pure.

use gear_engine::onboard::{plan_onboard, WorktreeStep};
use gear_herdr::builtin;
use gear_model::{SeatRole, SeatState};
use gear_store::SeatRow;

fn seat(name: &str, role: SeatRole) -> SeatRow {
    SeatRow {
        id: format!("harbor:{name}"),
        project: "harbor".into(),
        role,
        name: name.into(),
        profile: "opencode".into(),
        generation: 0,
        state: SeatState::Queued,
        state_since: 0,
        state_reason: None,
        task_id: None,
    }
}

#[test]
fn lane_gets_worktree_env_and_unattended_args() {
    let p = builtin("opencode").unwrap();
    let plan = plan_onboard(
        &seat("heat", SeatRole::Lane),
        "opencode",
        &p,
        1,
        "/repo",
        "dev",
        gear_model::branch::SEAT_BRANCH,
        "/state/gear.db",
        None,
    )
    .unwrap();
    assert_eq!(
        plan.worktree,
        Some(WorktreeStep {
            repo: "/repo".into(),
            branch: "harbor/heat".into(),
            base: "dev".into(),
            label: "harbor-heat".into()
        })
    );
    assert_eq!(
        plan.env_command,
        "export GEAR_CTX=harbor:heat:1; export GEAR_DB=/state/gear.db"
    );
    assert_eq!(
        (plan.agent_name.as_str(), plan.kind.as_str()),
        ("harbor-heat", "opencode")
    );
    assert_eq!(plan.args, vec!["--auto".to_string()]);
}

#[test]
fn captain_has_no_worktree_and_resume_appends_args() {
    let p = builtin("opencode").unwrap();
    let plan = plan_onboard(
        &seat("captain", SeatRole::Captain),
        "opencode",
        &p,
        3,
        "/repo",
        "dev",
        gear_model::branch::SEAT_BRANCH,
        "/db",
        Some("ses_1"),
    )
    .unwrap();
    assert!(plan.worktree.is_none());
    assert_eq!(
        plan.env_command,
        "export GEAR_CTX=harbor:captain:3; export GEAR_DB=/db"
    );
    assert_eq!(
        plan.args,
        vec!["--auto".to_string(), "-s".to_string(), "ses_1".to_string()]
    );
}

#[test]
fn refusals() {
    let mut p = builtin("opencode").unwrap();
    let long = seat("averyveryverylongnamethatwillnotfit", SeatRole::Lane);
    assert!(
        plan_onboard(
            &long,
            "opencode",
            &p,
            1,
            "/r",
            "dev",
            gear_model::branch::SEAT_BRANCH,
            "/db",
            None
        )
        .is_err(),
        "agent name > 32 chars"
    );
    p.resume_args = None;
    assert!(
        plan_onboard(
            &seat("heat", SeatRole::Lane),
            "opencode",
            &p,
            1,
            "/r",
            "dev",
            gear_model::branch::SEAT_BRANCH,
            "/db",
            Some("s")
        )
        .is_err(),
        "no resume support"
    );
    p.verified = false;
    assert!(
        plan_onboard(
            &seat("heat", SeatRole::Lane),
            "opencode",
            &p,
            1,
            "/r",
            "dev",
            gear_model::branch::SEAT_BRANCH,
            "/db",
            None
        )
        .is_err(),
        "unverified profile"
    );
}
