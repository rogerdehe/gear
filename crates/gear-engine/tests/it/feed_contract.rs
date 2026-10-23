//! Keep-lanes-fed contract (T-7): the engine tells the captain before a lane
//! runs dry — one item per lane, no duplicates, closed when the queue is
//! refilled, and nothing for a lane that is not supposed to work.

use gear_engine::{feed, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, MsgKind, MsgState, SeatRole, SeatState, TaskState};
use gear_store::{testing, NewMessage, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec![],
        quota_markers: vec![],
        auth_markers: vec![],
        tail_lines: 16,
        confirm_rounds: 2,
        unattended_args: vec!["--auto".into()],
        session_ref: false,
        resume_args: None,
        exit_keys: vec![],
        exit_command: None,
        verified: true,
    }
}

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// A project with a captain and one lane, `tasks` ready tasks on that lane.
/// The lane's agent is `lane` status: a busy lane is not assigned its ready
/// tasks by the same tick, which is what these tests want to observe.
fn world(lane: AgentStatus, tasks: usize) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            for n in 1..=tasks {
                task(tx, &format!("T-{n}"), n as i64)?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", lane);
    let mut cfg = EngineCfg { tick_s: 4, cooldown_s: 30, doorbell_check_budget: 3, ..Default::default() };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), profile(kind));
    }
    World { store, herdr, cfg }
}

fn task(tx: &gear_store::Tx<'_>, id: &str, rank: i64) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        project: "p".into(),
        id: id.into(),
        rank,
        state: TaskState::Ready,
        priority: "must".into(),
        lane: Some("heat".into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: Vec::new(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: None,
        updated: T0,
    })
}

/// The lane's items, open and closed.
fn items(store: &mut Store, lane: &str) -> Vec<gear_store::MessageRow> {
    store
        .read(|tx| tx.thread_messages("p", &format!("lane:{lane}")))
        .unwrap()
}

#[test]
fn a_lane_running_dry_gets_one_item_and_loses_it_when_refilled() {
    let mut w = world(AgentStatus::Working, 1);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Open);
    assert_eq!(item[0].kind, MsgKind::System);
    assert_eq!(item[0].recipient, "p:captain");
    assert_eq!(item[0].body, "lane heat has 1 ready task left");

    // The next tick creates no duplicate.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(items(&mut w.store, "heat").len(), 1, "no duplicate");

    // The queue is refilled: the item closes and nothing new opens.
    w.store.write(|tx| task(tx, "T-2", 2)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Resolved);
}

#[test]
fn a_lane_with_nothing_at_all_is_idle() {
    let mut w = world(AgentStatus::Idle, 0);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].body, "lane heat is IDLE");
}

#[test]
fn a_parked_or_held_lane_is_not_warned_about() {
    let mut w = world(AgentStatus::Idle, 0);
    w.store
        .write(|tx| {
            tx.set_seat_state("p:heat", SeatState::Parked, Some("parked for the test"), T0)?;
            testing::seed_active_seat(tx, "p", "runtime", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            tx.set_seat_state("p:runtime", SeatState::Held, Some("held for the test"), T0)?;
            Ok(())
        })
        .unwrap();

    // The observation is not what this test is about: the watch over the
    // store is.
    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 1).unwrap();

    assert!(changes.is_empty(), "{changes:?}");
    assert!(items(&mut w.store, "heat").is_empty());
    assert!(items(&mut w.store, "runtime").is_empty());
}

#[test]
fn an_item_from_a_previous_engine_is_not_duplicated() {
    let mut w = world(AgentStatus::Working, 1);
    // What a previous engine run left open.
    w.store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                project: "p".into(),
                thread: "lane:heat".into(),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: MsgKind::System,
                urgent: false,
                body: "lane heat has 1 ready task left".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0,
            })?;
            Ok(())
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 30).unwrap();

    assert_eq!(items(&mut w.store, "heat").len(), 1, "no duplicate after a restart");
}

#[test]
fn a_task_taken_by_a_delivery_is_not_ready_work() {
    let mut w = world(AgentStatus::Working, 2);
    // The first task's delivery was intended: a Delivered attempt while the
    // task is still Ready (the delivery is confirmed on a later tick).
    w.store
        .write(|tx| {
            tx.insert_attempt(&gear_store::AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Delivered,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            Ok(())
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    // One task is taken, one left: the lane is in the danger zone (R4).
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].body, "lane heat has 1 ready task left");
}

#[test]
fn a_parked_lane_closes_its_open_warning() {
    let mut w = world(AgentStatus::Working, 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Open);

    // Parked while the warning is open: the warning goes (R3).
    w.store
        .write(|tx| tx.set_seat_state("p:heat", SeatState::Parked, Some("parked"), T0 + 2))
        .unwrap();
    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 3).unwrap();

    assert_eq!(changes.len(), 1, "{changes:?}");
    let item = items(&mut w.store, "heat");
    assert_eq!(item[0].state, MsgState::Resolved);
}

#[test]
fn a_new_shortage_after_recovery_is_reported_at_once() {
    let mut w = world(AgentStatus::Working, 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(items(&mut w.store, "heat").len(), 1, "the first shortage opens an item");

    // Refilled: the item closes.
    w.store.write(|tx| task(tx, "T-2", 2)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Resolved);

    // The new work is finished, the lane is dry again: the alert must be
    // there on that same tick — no cooldown hides a new shortage (R1).
    w.store
        .write(|tx| {
            let mut done = tx.get_task("p", "T-2").unwrap().unwrap();
            done.state = TaskState::Done;
            tx.update_task(&done)?;
            Ok(())
        })
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 9).unwrap();

    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 2, "{item:?}");
    assert_eq!(item[1].state, MsgState::Open);
    assert_eq!(item[1].body, "lane heat has 1 ready task left");
}

#[test]
fn a_deferred_alert_is_the_same_item_and_resolves_on_refill() {
    let mut w = world(AgentStatus::Working, 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    // The captain defers the item: Waiting, due later (what
    // mail::reply(Wait) writes).
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                "M-1",
                MsgState::Waiting,
                None,
                Some("p:heat"),
                Some(T0 + 600),
                T0 + 2,
            )?;
            Ok(())
        })
        .unwrap();

    // The condition persists: the deferred item is the lane's item, so no
    // second System message appears (R2).
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Waiting);

    // Refilled: the deferred item resolves and never becomes due later.
    w.store.write(|tx| task(tx, "T-2", 2)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Resolved);
}

fn measured_history(w: &mut World) {
    use gear_store::{AttemptRow, AttemptStep};
    w.store.write(|tx| {
        task(tx, "T-90", 90)?;
        let mut done = tx.get_task("p", "T-90")?.unwrap();
        done.state = TaskState::Done;
        tx.update_task(&done)?;
        tx.insert_attempt(&AttemptRow {
            project: "p".into(), task: "T-90".into(), n: 1, seat: "p:heat".into(),
            generation: 1, contract_version: 1, adopted_version: 1, pending_update: None,
            state: AttemptState::Accepted, candidate: None, report: None, safe_point: None,
            started: T0 - 1800, updated: T0,
        })?;
        tx.set_attempt_time("p", "T-90", 1, AttemptStep::Started, T0 - 1800)?;
        tx.set_attempt_time("p", "T-90", 1, AttemptStep::HandedIn, T0)
    }).unwrap();
}

#[test]
fn measured_early_warning_stays_quiet_after_acknowledgment_until_worse() {
    let mut w = world(AgentStatus::Working, 1);
    measured_history(&mut w);
    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0).unwrap();
    assert_eq!(changes.len(), 1);
    let first = items(&mut w.store, "heat");
    assert_eq!(first[0].body, "lane heat runs dry in ~30 min");
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 4).unwrap().is_empty());
    w.store.write(|tx| tx.update_message_state("p", &first[0].id, MsgState::Resolved,
        Some("acknowledged"), None, None, T0 + 5)).unwrap();
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 8).unwrap().is_empty());
    assert_eq!(items(&mut w.store, "heat").len(), 1);
    // The last task is now finished: the shortage worsens to actual idle.
    w.store.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)
    }).unwrap();
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 10).unwrap();
    let all = items(&mut w.store, "heat");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].body, "lane heat is IDLE");
    assert_eq!(all[1].state, MsgState::Open);
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 12).unwrap().is_empty());
}

fn cap_env() -> (gear_app::Env, gear_app::identity::Fingerprint) {
    (gear_app::Env {
        ctx: Some(gear_model::GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some(CAP_PANE.into()), admin: false,
    }, gear_app::identity::Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None })
}

fn show_captain(w: &mut World, now: i64) {
    let (env, fp) = cap_env();
    let home = gear_app::home::build(&mut w.store, &env, Some(&fp), now).unwrap();
    gear_app::home::record_view(&mut w.store, &home.view).unwrap();
}

#[test]
fn a_twenty_minute_old_viewed_item_rings_urgently_once_per_period() {
    let mut w = world(AgentStatus::Working, 2); // no feed warning
    let id = w.store.write(|tx| Ok(tx.insert_message(&NewMessage {
        project: "p".into(), thread: "question".into(), task: None, attempt: None,
        sender: "p:heat".into(), sender_gen: 1, recipient: "p:captain".into(),
        kind: MsgKind::Question, urgent: false, body: "needs a decision".into(),
        reply_to: None, needs_ack: false, state: MsgState::Open, created: T0,
    })?.id)).unwrap();
    show_captain(&mut w, T0 + 60);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 600).unwrap();
    assert!(w.herdr.prompts_to(CAP_PANE).is_empty(), "viewed, not stale yet");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1200).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);
    assert!(w.herdr.prompts_to(CAP_PANE)[0].contains("1 urgent"));
    assert!(w.store.read(|tx| tx.get_message("p", &id)).unwrap().unwrap().urgent);
    show_captain(&mut w, T0 + 1201);
    for now in [1202, 1300, 1800, 2099] {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + now).unwrap();
    }
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1, "no reminder every tick");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 2100).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 2, "next period reminds again");
    show_captain(&mut w, T0 + 2101);
    w.store.write(|tx| tx.update_message_state("p", &id, MsgState::Resolved,
        Some("answered"), None, None, T0 + 2102)).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3000).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 2, "resolved items stay silent");
}

#[test]
fn tail_plan_closes_existing_idle_alert_and_prevents_its_doorbell() {
    let mut w = world(AgentStatus::Working, 0);
    feed::keep_lanes_fed(&mut w.store, "p", T0).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Open);
    let (env, fp) = cap_env();
    let text = "<!-- gear-plan project=p base=0 -->\n# Decisions\n- tail: heat\n";
    let draft = gear_app::draft::parse(text).unwrap();
    gear_app::plan::apply(&mut w.store, &env, Some(&fp), &draft, text, "tail1", T0 + 1).unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1200).unwrap();
    assert!(w.herdr.prompts_to(CAP_PANE).is_empty(), "tail prevents even the old stale idle alert ringing");
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Resolved);
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 1300).unwrap().is_empty());
    // Omitted declaration persists; explicit clearing resumes monitoring.
    let omitted = "<!-- gear-plan project=p base=1 -->\n# Decisions\n- keep sailing\n";
    let draft = gear_app::draft::parse(omitted).unwrap();
    gear_app::plan::apply(&mut w.store, &env, Some(&fp), &draft, omitted, "tail-omitted", T0 + 1301).unwrap();
    assert!(w.store.read(|tx| gear_app::plan::lane_in_tail(tx, "p", "heat")).unwrap());
    let text = "<!-- gear-plan project=p base=2 -->\n# Decisions\n- tail: -\n";
    let draft = gear_app::draft::parse(text).unwrap();
    gear_app::plan::apply(&mut w.store, &env, Some(&fp), &draft, text, "tail2", T0 + 1400).unwrap();
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 1401).unwrap();
    let all = items(&mut w.store, "heat");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].state, MsgState::Open);
}

#[test]
fn measured_horizon_counts_the_current_task_before_the_ready_queue() {
    use gear_store::{AttemptRow, AttemptStep};
    let mut w = world(AgentStatus::Working, 2);
    measured_history(&mut w);
    w.store.write(|tx| {
        let mut task = tx.get_task("p", "T-1")?.unwrap();
        task.state = TaskState::Running;
        tx.update_task(&task)?;
        tx.insert_attempt(&AttemptRow {
            project: "p".into(), task: "T-1".into(), n: 1, seat: "p:heat".into(),
            generation: 1, contract_version: 1, adopted_version: 1, pending_update: None,
            state: AttemptState::Started, candidate: None, report: None, safe_point: None,
            started: T0, updated: T0,
        })?;
        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
    }).unwrap();
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0).unwrap().is_empty(), "60m remaining, 30m threshold");
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 29 * 60).unwrap().is_empty());
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 30 * 60).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].body, "lane heat runs dry in ~30 min");
}

#[test]
fn tail_rejects_unknown_lanes_without_changing_the_plan() {
    let mut w = world(AgentStatus::Working, 0);
    let (env, fp) = cap_env();
    let text = "<!-- gear-plan project=p base=0 -->\n# Decisions\n- tail: missing\n";
    let draft = gear_app::draft::parse(text).unwrap();
    let error = gear_app::plan::apply(&mut w.store, &env, Some(&fp), &draft, text, "bad-tail", T0).unwrap_err();
    assert_eq!(error.code, gear_model::ErrorCode::BadFormat);
    assert_eq!(w.store.read(|tx| tx.plan_version("p")).unwrap(), 0);
    assert!(!w.store.read(|tx| gear_app::plan::lane_in_tail(tx, "p", "heat")).unwrap());
}

#[test]
fn configured_stale_period_keeps_an_unviewed_item_from_ringing_every_tick() {
    let mut w = world(AgentStatus::Working, 2);
    w.store.write(|tx| {
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
[timing]
captain_item_stale_min = 30
"#.into();
        tx.update_project(&project)?;
        tx.insert_message(&NewMessage {
            project: "p".into(), thread: "question".into(), task: None, attempt: None,
            sender: "p:heat".into(), sender_gen: 1, recipient: "p:captain".into(),
            kind: MsgKind::Question, urgent: false, body: "needs a decision".into(),
            reply_to: None, needs_ack: false, state: MsgState::Open, created: T0,
        })?;
        Ok(())
    }).unwrap();
    show_captain(&mut w, T0 + 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1200).unwrap();
    assert!(w.herdr.prompts_to(CAP_PANE).is_empty(), "30-minute override, not default 15");
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1800).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);
    // Model a settled failed doorbell with no new view: the reminder clock,
    // not the old shown set or normal cooldown, prevents another reminder.
    w.store.write(|tx| {
        let op = tx.last_op("p:captain", gear_model::OpKind::Doorbell)?.unwrap();
        tx.set_op_state("p", &op.id, gear_model::OpState::Failed, Some("test refusal"), T0 + 1801)
    }).unwrap();
    for now in [1900, 2400, 3599] {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + now).unwrap();
    }
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3600).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 2);
}
