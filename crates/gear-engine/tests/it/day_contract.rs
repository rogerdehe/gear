//! T-492: a simulated day for one captain pane that holds two projects with
//! different working hours in different time zones. Each fleet works only in
//! its own hours, both work in the overlap, the captain gets one doorbell
//! naming both projects' items, and a reply with a qualified id settles the
//! right project's item.

use gear_app::identity::Fingerprint;
use gear_app::mail::{self, ReplyArgs, ReplyMode};
use gear_app::Env;
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{GearCtx, MsgKind, MsgState, OpState, SeatRole, TaskState};
use gear_store::{testing, ContractRow, NewMessage, Store, TaskRow};

const CAP_PANE: &str = "w1:p1";
const P_LANE: &str = "w1:p2";
const Q_LANE: &str = "w2:p1";

/// 2027-03-10: Shanghai (UTC+8, no DST) and New York (UTC-5, EST — DST starts
/// on 14 March). p works 08:00–20:00 local = 00:00–12:00 UTC; q works
/// 06:00–18:00 local = 11:00–23:00 UTC. Their overlap is 11:00–12:00 UTC.
const DAY: &str = "2027-03-10";

fn config_toml(project: &str, pane: &str, tz: &str) -> String {
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
pane = "{pane}"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "pi"
profile = "pi"

[models]
lanes = ["heat-model"]

[schedule]
hours = "{hours}"
tz = "{tz}"
"#,
        hours = if project == "p" {
            "08:00-20:00"
        } else {
            "06:00-18:00"
        },
    )
}

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

fn at(clock: &str) -> i64 {
    gear_app::plan::parse_rfc3339(&format!("{DAY}T{clock}:00Z")).unwrap()
}

/// Two projects whose captain seats share `CAP_PANE`, each with one lane and
/// two ready tasks on it (the second keeps the lane from running dry, so no
/// feed item joins the captain's inbox).
fn world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            for (project, lane_pane, tz) in [
                ("p", P_LANE, "Asia/Shanghai"),
                ("q", Q_LANE, "America/New_York"),
            ] {
                testing::seed_project(tx, project, "sess", at("00:00"))?;
                let mut row = tx.get_project(project)?.unwrap();
                row.config_toml = config_toml(project, CAP_PANE, tz);
                tx.update_project(&row)?;
                testing::seed_active_seat(
                    tx,
                    project,
                    "captain",
                    SeatRole::Captain,
                    CAP_PANE,
                    "codex",
                    at("00:00"),
                )?;
                testing::seed_active_seat(
                    tx,
                    project,
                    "heat",
                    SeatRole::Lane,
                    lane_pane,
                    "pi",
                    at("00:00"),
                )?;
                tx.set_meta(&format!("engine.session_fresh.{project}:heat"), "1")?;
                // The lane already runs the chain's model: a comeback needs no
                // model switch, just its own agent back.
                tx.set_meta(&format!("engine.model.{project}:heat"), "heat-model")?;
                for (task, rank) in [("T-1", 1), ("T-2", 2)] {
                    tx.insert_task(&TaskRow {
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        project: project.into(),
                        id: task.into(),
                        rank,
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
                        updated: at("00:00"),
                    })?;
                    tx.insert_contract(&ContractRow {
                        project: project.into(),
                        task: task.into(),
                        version: 1,
                        goal: "g".into(),
                        accept: "a".into(),
                        evidence_files: vec![],
                        evidence_due_min: None,
                        body: String::new(),
                        created: at("00:00"),
                        mechanical: None,
                    })?;
                }
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "captain");
    herdr.add_agent_named(P_LANE, "pi", AgentStatus::Idle, "p-heat");
    herdr.add_agent_named(Q_LANE, "pi", AgentStatus::Idle, "q-heat");
    let cfg = EngineCfg {
        profiles: gear_herdr::builtins(),
        ..Default::default()
    };
    World { store, herdr, cfg }
}

/// The tasks a project's lane was handed in the window: the `New task` lines
/// typed into its pane. A nudge (`Continue`) is not a new task.
fn delivered(w: &mut World, project: &str) -> Vec<String> {
    let pane = if project == "p" { P_LANE } else { Q_LANE };
    w.herdr
        .prompts_to(pane)
        .into_iter()
        .filter_map(|body| body.split("New task ").nth(1))
        .map(|rest| rest.split(' ').next().unwrap_or_default().to_string())
        .collect()
}

/// Ticks the engine every five seconds through the station. A delivery op the
/// fake agent cannot confirm (it never runs `gear`) is confirmed by hand, as
/// the lane's own check-in would.
fn station(w: &mut World, clock: &str, steps: i64) -> Vec<String> {
    let start = at(clock);
    let mut hours = Vec::new();
    for step in 0..steps {
        let now = start + step * 5;
        let report = tick(&mut w.store, &mut w.herdr, &w.cfg, now).unwrap();
        hours.extend(report.hours);
        for project in ["p", "q"] {
            // Every op the fake agent cannot settle (it never runs `gear`) is
            // settled by hand, as the seat's own check-in would: a delivery,
            // and a doorbell the captain has read.
            let mut ops = Vec::new();
            for state in [
                OpState::Intended,
                OpState::Executing,
                OpState::Submitted,
                OpState::Failed,
            ] {
                ops.extend(w.store.read(|tx| tx.ops_in_state(project, state)).unwrap());
            }
            for op in ops {
                w.store
                    .write(|tx| {
                        tx.set_op_state(
                            project,
                            &op.id,
                            OpState::Confirmed,
                            Some("the seat checked in"),
                            now,
                        )
                    })
                    .unwrap();
            }
        }
    }
    hours
}

/// Whether a project's lane was parked or released for its working hours.
fn rests(lines: &[String], project: &str) -> bool {
    lines.iter().any(|line| {
        line.starts_with(&format!("{project}:heat:"))
            && (line.contains("parked") || line.contains("released"))
    })
}

/// One open item for a project's captain; returns its id.
fn captain_item(w: &mut World, project: &str, at: i64) -> String {
    w.store
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: project.into(),
                    thread: format!("{project}:{at}"),
                    task: None,
                    attempt: None,
                    sender: format!("{project}:captain"),
                    sender_gen: 1,
                    recipient: format!("{project}:captain"),
                    kind: MsgKind::System,
                    urgent: false,
                    body: "act on it".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: at,
                })?
                .id)
        })
        .unwrap()
}

#[test]
fn a_day_of_two_fleets_in_different_zones_rings_once_and_settles_the_named_project() {
    let mut w = world();

    // 04:00 UTC is 12:00 in Shanghai (p works) and 23:00 in New York (q
    // rests): only p's fleet takes a task; q's lane is parked and released
    // for its working hours.
    let hours = station(&mut w, "04:00", 4);
    assert_eq!(delivered(&mut w, "p"), ["T-1"], "p works");
    assert!(delivered(&mut w, "q").is_empty(), "q rests");
    assert!(
        rests(&hours, "q"),
        "q's lane is parked for its hours: {hours:?}"
    );
    assert!(!rests(&hours, "p"), "p's lane works: {hours:?}");
    // The released agent left the pane (the engine typed its exit); the pane
    // is a shell until the fleet comes back.
    w.herdr.add_shell(Q_LANE);

    // 11:30 UTC is 19:30 in Shanghai and 06:30 in New York: the overlap, both
    // fleets work. p already has its task; q's lane comes back and takes its
    // own.
    let hours = station(&mut w, "11:30", 6);
    assert_eq!(delivered(&mut w, "p"), ["T-1"], "p keeps working");
    assert_eq!(delivered(&mut w, "q"), ["T-1"], "q works in the overlap");
    assert!(
        !rests(&hours, "p") && !rests(&hours, "q"),
        "both fleets work in the overlap: {hours:?}"
    );

    // The captain's two items, one per project, in the overlap: one doorbell
    // naming both projects' items. A resting project adds nothing, so both
    // projects ringing is both fleets working in the overlap.
    let p_item = captain_item(&mut w, "p", at("11:50"));
    let q_item = captain_item(&mut w, "q", at("11:50"));
    let before = w.herdr.prompts_to(CAP_PANE).len();
    tick(&mut w.store, &mut w.herdr, &w.cfg, at("11:51")).unwrap();
    let prompts = w.herdr.prompts_to(CAP_PANE);
    assert_eq!(
        prompts.len(),
        before + 1,
        "one doorbell for the pane: {prompts:?}"
    );
    let text = prompts.last().expect("the doorbell is typed");
    assert!(text.contains(&p_item), "p's item is named: {text}");
    assert!(text.contains(&q_item), "q's item is named: {text}");
    assert!(
        text.contains("- p:") && text.contains("- q:"),
        "each line names its project: {text}"
    );
    assert!(
        text.contains("across 2 projects"),
        "the doorbell says it holds two projects: {text}"
    );

    // A reply with a qualified id settles q's item and leaves p's open.
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
        kind: Some("codex".into()),
        session_ref: None,
    };
    let out = mail::reply(
        &mut w.store,
        &env,
        Some(&fp),
        &ReplyArgs {
            ids: vec![format!("q:{q_item}")],
            text: "the q item".into(),
            mode: ReplyMode::Resolve,
            wait_on: None,
            wait_until: None,
            downgrade: false,
            request_id: "day-reply-1".into(),
        },
        at("11:52"),
    )
    .unwrap();
    assert_eq!(out.handled, vec![(q_item.clone(), "resolved".into())]);
    let q = w
        .store
        .read(|tx| tx.get_message("q", &q_item))
        .unwrap()
        .unwrap();
    assert_eq!(q.state, MsgState::Resolved, "the named project's item");
    let p = w
        .store
        .read(|tx| tx.get_message("p", &p_item))
        .unwrap()
        .unwrap();
    assert_eq!(
        p.state,
        MsgState::Open,
        "the other project's item is untouched"
    );

    // 14:00 UTC is 22:00 in Shanghai (p rests) and 09:00 in New York: no new
    // task goes out to p's fleet.
    let hours = station(&mut w, "14:00", 6);
    assert_eq!(delivered(&mut w, "p"), ["T-1"], "p rests");
    assert_eq!(delivered(&mut w, "q"), ["T-1"], "q keeps its task");
    assert!(
        rests(&hours, "p"),
        "p's lane is parked for its hours: {hours:?}"
    );
    assert!(!rests(&hours, "q"), "q's lane works: {hours:?}");
    w.herdr.add_shell(P_LANE);
}
