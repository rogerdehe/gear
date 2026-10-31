//! T-488: working hours in a project's own time zone. Two projects whose hours
//! overlap both deliver in the overlap, and only the one whose zone says it is
//! working delivers outside it.

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

fn config_toml(project: &str, captain_pane: &str, tz: &str) -> String {
    format!(
        r#"
[project]
id = "{project}"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "{captain_pane}"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "pi"
profile = "pi"

[schedule]
hours = "10:00-18:00"
tz = "{tz}"
"#
    )
}

/// Two projects with one lane each and a ready task; `p` is in Shanghai and
/// `q` in Tokyo, so their hours overlap between 02:00 and 09:00 UTC.
fn world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            for (project, captain, lane, tz) in [
                ("p", "w1:p1", "w1:p2", "Asia/Shanghai"),
                ("q", "w2:p1", "w2:p2", "Asia/Tokyo"),
            ] {
                testing::seed_project(tx, project, "sess", T0)?;
                let mut row = tx.get_project(project)?.unwrap();
                row.config_toml = config_toml(project, captain, tz);
                tx.update_project(&row)?;
                testing::seed_active_seat(
                    tx,
                    project,
                    "captain",
                    SeatRole::Captain,
                    captain,
                    "codex",
                    T0,
                )?;
                testing::seed_active_seat(tx, project, "heat", SeatRole::Lane, lane, "pi", T0)?;
                // Onboarding has started the lane's agent: it is ready for its
                // first task in this tick (as in the ready-state tests).
                tx.set_meta(&format!("engine.session_fresh.{project}:heat"), "1")?;
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: project.into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: project.into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "g".into(),
                    accept: "a".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    for (pane, kind, name) in [
        ("w1:p1", "codex", "p-captain"),
        ("w1:p2", "pi", "p-heat"),
        ("w2:p1", "codex", "q-captain"),
        ("w2:p2", "pi", "q-heat"),
    ] {
        herdr.add_agent_named(pane, kind, AgentStatus::Idle, name);
    }
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    World { store, herdr, cfg }
}

/// What was typed into the project's lane pane: the task packet, when the
/// lane was handed one.
fn delivered(w: &mut World, project: &str) -> Vec<String> {
    let pane = if project == "p" { "w1:p2" } else { "w2:p2" };
    w.herdr
        .prompts_to(pane)
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[test]
fn both_projects_deliver_in_the_overlap_of_their_hours() {
    // 05:00 UTC is 13:00 in Shanghai and 14:00 in Tokyo: both work.
    let mut w = world();
    let at = gear_app::plan::parse_rfc3339("2027-03-10T05:00:00Z").unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, at).unwrap();
    for project in ["p", "q"] {
        assert!(
            delivered(&mut w, project)
                .iter()
                .any(|body| body.starts_with("New task T-1")),
            "{project} delivers in the overlap: {:?}",
            delivered(&mut w, project)
        );
    }
}

#[test]
fn only_the_project_inside_its_hours_delivers_outside_the_overlap() {
    // 09:30 UTC is 17:30 in Shanghai (in hours) and 18:30 in Tokyo (off).
    let mut w = world();
    let at = gear_app::plan::parse_rfc3339("2027-03-10T09:30:00Z").unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, at).unwrap();
    assert!(
        delivered(&mut w, "p")
            .iter()
            .any(|body| body.starts_with("New task T-1")),
        "p works: {:?}",
        delivered(&mut w, "p")
    );
    assert!(
        !delivered(&mut w, "q")
            .iter()
            .any(|body| body.starts_with("New task T-1")),
        "q rests, so no task goes out (the release may type /quit): {:?}",
        delivered(&mut w, "q")
    );

    // 01:30 UTC is 09:30 in Shanghai (off) and 10:30 in Tokyo (in hours).
    let mut w = world();
    let at = gear_app::plan::parse_rfc3339("2027-03-10T01:30:00Z").unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, at).unwrap();
    assert!(
        !delivered(&mut w, "p")
            .iter()
            .any(|body| body.starts_with("New task T-1")),
        "p rests, so no task goes out: {:?}",
        delivered(&mut w, "p")
    );
    assert!(
        delivered(&mut w, "q")
            .iter()
            .any(|body| body.starts_with("New task T-1")),
        "q works: {:?}",
        delivered(&mut w, "q")
    );
}
