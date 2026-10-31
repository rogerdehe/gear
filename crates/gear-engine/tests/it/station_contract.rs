//! A station is persisted before the tick's ordinary doorbell decision.
use gear_app::{home, identity::Fingerprint, Env};
use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{GearCtx, MsgState, SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow, VoyageRow};

const T0: i64 = 1_800_000_000;

#[test]
fn a_checkpoint_rings_in_the_same_tick_and_survives_reopening_the_store() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gear.db");
    let mut store = Store::open(&path).unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M2".into(),
                title: "course".into(),
                done: Some("ship it".into()),
                bar: Some("stable".into()),
                eta: Some("4h".into()),
                departure: Some(T0),
                departure_tasks: Some(2),
                departure_estimate_min: Some(60),
                updated: T0,
            })?;
            for n in 1..=2 {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: format!("T-{n}"),
                    rank: n,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: Some("M2".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: T0,
                })?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "codex", AgentStatus::Idle);
    herdr.add_agent("w1:p2", "opencode", AgentStatus::Working);
    let mut cfg = EngineCfg::default();
    for kind in ["codex", "opencode"] {
        cfg.profiles
            .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
    }
    assert!(tick(&mut store, &mut herdr, &cfg, T0 + 3599)
        .unwrap()
        .stations
        .is_empty());
    assert!(herdr.prompts_to("w1:p1").is_empty());
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 3600).unwrap();
    assert_eq!(report.stations, vec!["p:M-1"]);
    assert_eq!(
        herdr.prompts_to("w1:p1").len(),
        1,
        "station rings on the crossing tick"
    );
    let env = Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some("w1:p1".into()),
        admin: false,
    };
    let fp = Fingerprint {
        pane_id: "w1:p1".into(),
        kind: Some("codex".into()),
        session_ref: None,
    };
    let page = home::build(&mut store, &env, Some(&fp), T0 + 3601).unwrap();
    assert!(
        page.text.contains("Station M2 [60 min, 25% eta]"),
        "{}",
        page.text
    );
    assert!(page
        .text
        .contains("next: gear plan apply gear/voyages/M2/plan.md"));
    home::record_view(&mut store, &page.view).unwrap();
    tick(&mut store, &mut herdr, &cfg, T0 + 3602).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 4000).unwrap();
    assert!(
        report.stations.is_empty(),
        "checkpoint marker survives restart"
    );
    assert_eq!(herdr.prompts_to("w1:p1").len(), 1);
    let messages = store
        .read(|tx| tx.thread_messages("p", "station:M2"))
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].state,
        MsgState::Open,
        "a view/restart cannot answer the station"
    );
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 7200).unwrap();
    assert_eq!(report.stations.len(), 1);
    assert_eq!(
        herdr.prompts_to("w1:p1").len(),
        2,
        "the next quarter is a new station"
    );
}
