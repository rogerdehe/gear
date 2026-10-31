//! Keep-lanes-fed contract (T-7): the engine tells the captain before a lane
//! runs dry — one item per lane, no duplicates, closed when the queue is
//! refilled, and nothing for a lane that is not supposed to work.

use gear_engine::{feed, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, MsgKind, MsgState, SeatRole, SeatState, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, NewMessage, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const DECK_PANE: &str = "w1:p3";

fn profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
        dialogs: Vec::new(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec![],
        quota_markers: vec![],
        quota_bar: None,
        auth_markers: vec![],
        backoff_markers: vec![],
        blocked_markers: vec![],
        tail_lines: 16,
        confirm_rounds: 2,
        unattended_args: vec!["--auto".into()],
        instructions_args: vec![],
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
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
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
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        ..Default::default()
    };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), profile(kind));
    }
    World { store, herdr, cfg }
}

fn task(tx: &gear_store::Tx<'_>, id: &str, rank: i64) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
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
        issue: None,
        updated: T0,
    })
}

/// Startable work elsewhere in the project: the feed talks about a lane only
/// while some lane has work to start (T-232). Two ready tasks and no
/// measurements keep `deck`'s own thread quiet.
fn startable_elsewhere(w: &mut World) {
    w.store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", "deck", SeatRole::Lane, DECK_PANE, "opencode", T0)?;
            lane_task(tx, "T-8", "deck", 8, TaskState::Ready, None, vec![])?;
            lane_task(tx, "T-9", "deck", 9, TaskState::Ready, None, vec![])
        })
        .unwrap();
    w.herdr
        .add_agent(DECK_PANE, "opencode", AgentStatus::Working);
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
    // The project has work to start elsewhere: the idle lane is told (T-232).
    startable_elsewhere(&mut w);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].body, "lane heat is IDLE: no next task to dispatch (its agent may still be finishing a turn); plan a task for it or move one to it");
}

/// An unanswered captain instruction to the lane (`kind = Instruction`); its
/// message id comes back for the reply.
fn instruct(w: &mut World, created: i64) -> String {
    w.store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "hull:175".into(),
                task: None,
                attempt: None,
                sender: "p:captain".into(),
                sender_gen: 1,
                recipient: "p:heat".into(),
                kind: MsgKind::Instruction,
                urgent: false,
                body: "please look at the merge".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created,
            })
        })
        .unwrap()
        .id
}

/// The lane answers that instruction.
fn reply(w: &mut World, instruction: &str, created: i64) {
    w.store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "hull:175".into(),
                task: None,
                attempt: None,
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Reply,
                urgent: false,
                body: "done".into(),
                reply_to: Some(instruction.into()),
                needs_ack: false,
                state: MsgState::Open,
                created,
            })
        })
        .unwrap();
}

/// T-324: a lane that helps on an unanswered captain instruction is not idle:
/// the open IDLE item closes with the fact, no new one opens while the help
/// lasts, and the lane's reply brings the IDLE item back.
#[test]
fn a_lane_helping_on_an_unanswered_instruction_is_not_idle() {
    let mut w = world(AgentStatus::Idle, 0);
    // The project has work to start elsewhere: the idle lane is told first.
    startable_elsewhere(&mut w);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(
        items(&mut w.store, "heat").len(),
        1,
        "the lane is idle first"
    );

    // The captain asks the lane for something and it has not answered yet.
    let instruction = instruct(&mut w, T0 + 2);

    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 5).unwrap();
    assert_eq!(
        changes,
        vec![feed::FeedChange {
            lane: "heat".into(),
            action: "closed"
        }]
    );
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 1, "{item:?}");
    assert_eq!(item[0].state, MsgState::Resolved);
    assert_eq!(
        item[0].result.as_deref(),
        Some("the lane is helping the captain: please look at the merge")
    );

    // The next pass opens nothing while the help lasts.
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 6)
        .unwrap()
        .is_empty());
    assert_eq!(
        items(&mut w.store, "heat").len(),
        1,
        "no IDLE item while helping"
    );

    // The lane answers: the IDLE item opens again on the next feed pass.
    reply(&mut w, &instruction, T0 + 7);
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 10).unwrap();
    let item = items(&mut w.store, "heat");
    assert_eq!(item.len(), 2, "{item:?}");
    assert_eq!(item[1].state, MsgState::Open);
    assert!(
        item[1].body.starts_with("lane heat is IDLE"),
        "{}",
        item[1].body
    );
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
                meta: None,
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

    assert_eq!(
        items(&mut w.store, "heat").len(),
        1,
        "no duplicate after a restart"
    );
}

#[test]
fn a_task_taken_by_a_delivery_is_not_ready_work() {
    let mut w = world(AgentStatus::Working, 2);
    // The first task's delivery was intended: a Delivered attempt while the
    // task is still Ready (the delivery is confirmed on a later tick).
    w.store
        .write(|tx| {
            tx.insert_attempt(&gear_store::AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
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
    assert_eq!(
        items(&mut w.store, "heat").len(),
        1,
        "the first shortage opens an item"
    );

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
    w.store
        .write(|tx| {
            task(tx, "T-90", 90)?;
            let mut done = tx.get_task("p", "T-90")?.unwrap();
            done.state = TaskState::Done;
            tx.update_task(&done)?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-90".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Accepted,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0 - 1800,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-90", 1, AttemptStep::Started, T0 - 1800)?;
            tx.set_attempt_time("p", "T-90", 1, AttemptStep::HandedIn, T0)
        })
        .unwrap();
}

#[test]
fn measured_early_warning_stays_quiet_after_acknowledgment_until_worse() {
    let mut w = world(AgentStatus::Working, 1);
    startable_elsewhere(&mut w);
    measured_history(&mut w);
    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0).unwrap();
    assert_eq!(changes.len(), 1);
    let first = items(&mut w.store, "heat");
    assert_eq!(first[0].body, "lane heat runs dry in ~30 min");
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 4)
        .unwrap()
        .is_empty());
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                &first[0].id,
                MsgState::Resolved,
                Some("acknowledged"),
                None,
                None,
                T0 + 5,
            )
        })
        .unwrap();
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 8)
        .unwrap()
        .is_empty());
    assert_eq!(items(&mut w.store, "heat").len(), 1);
    // The last task is now finished: the shortage worsens to actual idle.
    w.store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = TaskState::Done;
            tx.update_task(&task)
        })
        .unwrap();
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 10).unwrap();
    let all = items(&mut w.store, "heat");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].body, "lane heat is IDLE: no next task to dispatch (its agent may still be finishing a turn); plan a task for it or move one to it");
    assert_eq!(all[1].state, MsgState::Open);
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 12)
        .unwrap()
        .is_empty());
}

fn cap_env() -> (gear_app::Env, gear_app::identity::Fingerprint) {
    (
        gear_app::Env {
            ctx: Some(gear_model::GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        gear_app::identity::Fingerprint {
            pane_id: CAP_PANE.into(),
            kind: Some("codex".into()),
            session_ref: None,
        },
    )
}

fn show_captain(w: &mut World, now: i64) {
    let (env, fp) = cap_env();
    let home = gear_app::home::build(&mut w.store, &env, Some(&fp), now).unwrap();
    gear_app::home::record_view(&mut w.store, &home.view).unwrap();
}

#[test]
fn a_twenty_minute_old_viewed_item_rings_urgently_once_per_period() {
    let mut w = world(AgentStatus::Working, 2); // no feed warning
                                                // This test isolates captain reminders, not a taskless working lane (T-46).
    w.store
        .write(|tx| {
            tx.set_seat_state(
                "p:heat",
                SeatState::Held,
                Some("captain reminder fixture"),
                T0,
            )
        })
        .unwrap();
    let id = w
        .store
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "question".into(),
                    task: None,
                    attempt: None,
                    sender: "p:heat".into(),
                    sender_gen: 1,
                    recipient: "p:captain".into(),
                    kind: MsgKind::Question,
                    urgent: false,
                    body: "needs a decision".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: T0,
                })?
                .id)
        })
        .unwrap();
    show_captain(&mut w, T0 + 60);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 600).unwrap();
    assert!(
        w.herdr.prompts_to(CAP_PANE).is_empty(),
        "viewed, not stale yet"
    );
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1200).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);
    assert!(w.herdr.prompts_to(CAP_PANE)[0].contains("1 urgent"));
    assert!(
        w.store
            .read(|tx| tx.get_message("p", &id))
            .unwrap()
            .unwrap()
            .urgent
    );
    show_captain(&mut w, T0 + 1201);
    for now in [1202, 1300, 1800, 2099] {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + now).unwrap();
    }
    assert_eq!(
        w.herdr.prompts_to(CAP_PANE).len(),
        1,
        "no reminder every tick"
    );
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 2100).unwrap();
    assert_eq!(
        w.herdr.prompts_to(CAP_PANE).len(),
        2,
        "next period reminds again"
    );
    show_captain(&mut w, T0 + 2101);
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                &id,
                MsgState::Resolved,
                Some("answered"),
                None,
                None,
                T0 + 2102,
            )
        })
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3000).unwrap();
    assert_eq!(
        w.herdr.prompts_to(CAP_PANE).len(),
        2,
        "resolved items stay silent"
    );
}

#[test]
fn tail_plan_closes_existing_idle_alert_and_prevents_its_doorbell() {
    let mut w = world(AgentStatus::Working, 0);
    startable_elsewhere(&mut w);
    feed::keep_lanes_fed(&mut w.store, "p", T0).unwrap();
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Open);
    let (env, fp) = cap_env();
    let text = "<!-- gear-plan project=p base=0 -->\n# Decisions\n- tail: heat\n";
    let draft = gear_app::draft::parse(text).unwrap();
    gear_app::plan::apply(
        &mut w.store,
        &env,
        Some(&fp),
        None,
        &draft,
        text,
        "tail1",
        T0 + 1,
    )
    .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1200).unwrap();
    assert!(
        w.herdr.prompts_to(CAP_PANE).is_empty(),
        "tail prevents even the old stale idle alert ringing"
    );
    assert_eq!(items(&mut w.store, "heat")[0].state, MsgState::Resolved);
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 1300)
        .unwrap()
        .is_empty());
    // Omitted declaration persists; explicit clearing resumes monitoring.
    let omitted = "<!-- gear-plan project=p base=1 -->\n# Decisions\n- keep sailing\n";
    let draft = gear_app::draft::parse(omitted).unwrap();
    gear_app::plan::apply(
        &mut w.store,
        &env,
        Some(&fp),
        None,
        &draft,
        omitted,
        "tail-omitted",
        T0 + 1301,
    )
    .unwrap();
    assert!(w
        .store
        .read(|tx| gear_app::plan::lane_in_tail(tx, "p", "heat"))
        .unwrap());
    let text = "<!-- gear-plan project=p base=2 -->\n# Decisions\n- tail: -\n";
    let draft = gear_app::draft::parse(text).unwrap();
    gear_app::plan::apply(
        &mut w.store,
        &env,
        Some(&fp),
        None,
        &draft,
        text,
        "tail2",
        T0 + 1400,
    )
    .unwrap();
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 1401).unwrap();
    let all = items(&mut w.store, "heat");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].state, MsgState::Open);
}

#[test]
fn measured_horizon_counts_the_current_task_before_the_ready_queue() {
    use gear_store::{AttemptRow, AttemptStep};
    let mut w = world(AgentStatus::Working, 2);
    startable_elsewhere(&mut w);
    measured_history(&mut w);
    w.store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-1")?.unwrap();
            task.state = TaskState::Running;
            tx.update_task(&task)?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
        })
        .unwrap();
    assert!(
        feed::keep_lanes_fed(&mut w.store, "p", T0)
            .unwrap()
            .is_empty(),
        "60m remaining, 30m threshold"
    );
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 29 * 60)
        .unwrap()
        .is_empty());
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 30 * 60).unwrap();
    assert_eq!(
        items(&mut w.store, "heat")[0].body,
        "lane heat runs dry in ~30 min"
    );
    // Starting the queued task keeps the same runway, so it is not another
    // warning. Only a shorter measured runway is a worsening condition.
    w.store
        .write(|tx| {
            let mut previous = tx.get_attempt("p", "T-1", 1)?.unwrap();
            previous.state = AttemptState::Submitted;
            tx.update_attempt(&previous)?;
            let mut next = previous;
            next.task = "T-2".into();
            next.state = AttemptState::Started;
            tx.insert_attempt(&next)?;
            tx.set_attempt_time("p", "T-2", 1, AttemptStep::Started, T0 + 30 * 60)
        })
        .unwrap();
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 30 * 60)
        .unwrap()
        .is_empty());
    assert_eq!(items(&mut w.store, "heat").len(), 1);
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 45 * 60).unwrap();
    let all = items(&mut w.store, "heat");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].state, MsgState::Open);
    assert_eq!(all[0].body, "lane heat runs dry in ~15 min");
}

#[test]
fn tail_rejects_unknown_lanes_without_changing_the_plan() {
    let mut w = world(AgentStatus::Working, 0);
    let (env, fp) = cap_env();
    let text = "<!-- gear-plan project=p base=0 -->\n# Decisions\n- tail: missing\n";
    let draft = gear_app::draft::parse(text).unwrap();
    let error = gear_app::plan::apply(
        &mut w.store,
        &env,
        Some(&fp),
        None,
        &draft,
        text,
        "bad-tail",
        T0,
    )
    .unwrap_err();
    assert_eq!(error.code, gear_model::ErrorCode::BadFormat);
    assert_eq!(w.store.read(|tx| tx.plan_version("p")).unwrap(), 0);
    assert!(!w
        .store
        .read(|tx| gear_app::plan::lane_in_tail(tx, "p", "heat"))
        .unwrap());
}

#[test]
fn configured_stale_period_keeps_an_unviewed_item_from_ringing_every_tick() {
    let mut w = world(AgentStatus::Working, 2);
    // This test isolates captain reminders, not a taskless working lane (T-46).
    w.store
        .write(|tx| {
            tx.set_seat_state(
                "p:heat",
                SeatState::Held,
                Some("captain reminder fixture"),
                T0,
            )
        })
        .unwrap();
    w.store
        .write(|tx| {
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
"#
            .into();
            tx.update_project(&project)?;
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "question".into(),
                task: None,
                attempt: None,
                sender: "p:heat".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Question,
                urgent: false,
                body: "needs a decision".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0,
            })?;
            Ok(())
        })
        .unwrap();
    show_captain(&mut w, T0 + 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1200).unwrap();
    assert!(
        w.herdr.prompts_to(CAP_PANE).is_empty(),
        "30-minute override, not default 15"
    );
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1800).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);
    // Model a settled failed doorbell with no new view: the reminder clock,
    // not the old shown set or normal cooldown, prevents another reminder.
    w.store
        .write(|tx| {
            let op = tx
                .last_op("p:captain", gear_model::OpKind::Doorbell)?
                .unwrap();
            tx.set_op_state(
                "p",
                &op.id,
                gear_model::OpState::Failed,
                Some("test refusal"),
                T0 + 1801,
            )
        })
        .unwrap();
    for now in [1900, 2400, 3599] {
        tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + now).unwrap();
    }
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 3600).unwrap();
    assert_eq!(w.herdr.prompts_to(CAP_PANE).len(), 2);
}

// ---- the imbalance item and the one-task-from-dry warning (T-162)

/// A task of a lane, with an estimate and a state.
fn lane_task(
    tx: &gear_store::Tx<'_>,
    id: &str,
    lane: &str,
    rank: i64,
    state: TaskState,
    estimate: Option<i64>,
    deps: Vec<&str>,
) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: "p".into(),
        id: id.into(),
        rank,
        state,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: deps.into_iter().map(str::to_string).collect(),
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: estimate,
        issue: None,
        updated: T0,
    })
}

/// The imbalance item of the project.
fn balance_items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store.read(|tx| tx.thread_messages("p", "balance")).unwrap()
}

/// A project with two lanes: `mast` has nothing left, `deck` has three
/// ready tasks of 40 minutes each.
fn imbalance_world() -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "mast", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "deck", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            for n in 1..=3 {
                lane_task(
                    tx,
                    &format!("T-{n}"),
                    "deck",
                    n as i64,
                    TaskState::Ready,
                    Some(40),
                    vec![],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    // The lane with the pile of work is busy: its tasks stay queued.
    herdr.add_agent("w1:p3", "opencode", AgentStatus::Working);
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        ..Default::default()
    };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), profile(kind));
    }
    World { store, herdr, cfg }
}

#[test]
fn a_retired_idle_lane_does_not_open_a_balance_item() {
    let mut w = imbalance_world();
    w.store
        .write(|tx| tx.set_seat_state("p:mast", SeatState::Retired, Some("retired"), T0))
        .unwrap();

    feed::keep_lanes_fed(&mut w.store, "p", T0 + 1).unwrap();
    assert!(balance_items(&mut w.store).is_empty());

    w.store
        .write(|tx| tx.set_seat_state("p:mast", SeatState::Active, None, T0 + 2))
        .unwrap();
    feed::keep_lanes_fed(&mut w.store, "p", T0 + 3).unwrap();
    let items = balance_items(&mut w.store);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Open);
}

#[test]
fn an_idle_lane_next_to_a_pile_of_work_gives_one_balance_item() {
    let mut w = imbalance_world();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    let items = balance_items(&mut w.store);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Open);
    assert_eq!(items[0].thread, "balance");
    assert_eq!(items[0].recipient, "p:captain");
    assert_eq!(
        items[0].body,
        "mast has nothing left; deck has 3 tasks (120m) left: move some (lane ownership allowing) or plan more for mast."
    );

    // The next tick does not repeat the fact.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(balance_items(&mut w.store).len(), 1, "no duplicate");

    // The imbalance ends: the idle lane gets work.
    w.store
        .write(|tx| lane_task(tx, "T-9", "mast", 9, TaskState::Ready, Some(20), vec![]))
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();
    let items = balance_items(&mut w.store);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Resolved, "the item closed");
}

#[test]
fn a_lane_one_task_from_dry_warns_once() {
    let mut w = world(AgentStatus::Working, 0);
    // The lane works on its last task; the one behind it waits for a
    // dependency that is not done.
    startable_elsewhere(&mut w);
    w.store
        .write(|tx| {
            lane_task(tx, "T-1", "heat", 1, TaskState::Running, Some(30), vec![])?;
            lane_task(
                tx,
                "T-2",
                "heat",
                2,
                TaskState::Ready,
                Some(30),
                vec!["T-3"],
            )?;
            lane_task(tx, "T-3", "wire", 3, TaskState::Running, Some(30), vec![])?;
            // The lane works on T-1.
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();

    let lane_items = items(&mut w.store, "heat");
    assert_eq!(lane_items.len(), 1, "{lane_items:?}");
    assert_eq!(lane_items[0].state, MsgState::Open);
    assert_eq!(
        lane_items[0].body,
        "lane heat has nothing to start after T-1 (1 queued behind deps: T-2)"
    );

    // The next tick does not repeat the warning: one item per state.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    assert_eq!(items(&mut w.store, "heat").len(), 1, "no duplicate");
}

/// F2: the warning about the work in flight is not the idle state. When the
/// lane hands its last task in, it stands idle — a fresh, worse fact, with
/// its own item.
#[test]
fn a_lane_that_hands_its_last_task_in_says_it_is_idle() {
    let mut w = world(AgentStatus::Working, 0);
    startable_elsewhere(&mut w);
    w.store
        .write(|tx| {
            lane_task(tx, "T-1", "heat", 1, TaskState::Running, Some(30), vec![])?;
            lane_task(
                tx,
                "T-2",
                "heat",
                2,
                TaskState::Ready,
                Some(30),
                vec!["T-3"],
            )?;
            lane_task(tx, "T-3", "wire", 3, TaskState::Running, Some(30), vec![])?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(
        items(&mut w.store, "heat").len(),
        1,
        "the warning about the work in flight"
    );

    // The lane hands the task in: the review runs, the lane has nothing left.
    w.store
        .write(|tx| {
            let mut attempt = tx.get_attempt("p", "T-1", 1)?.expect("the attempt");
            attempt.state = AttemptState::Submitted;
            tx.update_attempt(&attempt)?;
            let mut task = tx.get_task("p", "T-1")?.expect("the task");
            task.state = TaskState::Verifying;
            tx.update_task(&task)
        })
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    let lane_items = items(&mut w.store, "heat");
    assert_eq!(lane_items.len(), 1, "{lane_items:?}");
    assert_eq!(
        lane_items[0].state,
        MsgState::Open,
        "the idle state updates the existing item"
    );
    assert_eq!(lane_items[0].body, "lane heat is IDLE: no next task to dispatch (its agent may still be finishing a turn); plan a task for it or move one to it");

    // And the next tick is silent: the idle lane is not told again.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 15).unwrap();
    assert_eq!(items(&mut w.store, "heat").len(), 1, "no duplicate");
}

/// F1: a balance item the captain resolved is not told again while the same
/// imbalance lasts; a later imbalance is told again.
#[test]
fn a_resolved_balance_item_is_not_told_again() {
    let mut w = imbalance_world();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    assert_eq!(
        balance_items(&mut w.store).len(),
        1,
        "{:?}",
        balance_items(&mut w.store)
    );

    // The captain resolves the item.
    let id = balance_items(&mut w.store)[0].id.clone();
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                &id,
                MsgState::Resolved,
                Some("seen"),
                None,
                None,
                T0 + 2,
            )
        })
        .unwrap();

    // The next tick does not tell the same fact again.
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 5).unwrap();
    let items = balance_items(&mut w.store);
    assert_eq!(items.len(), 1, "no new item is opened: {items:?}");
    assert_eq!(
        items[0].state,
        MsgState::Resolved,
        "the one item stays resolved"
    );

    // The imbalance ends: the idle lane gets work.
    w.store
        .write(|tx| lane_task(tx, "T-9", "mast", 9, TaskState::Ready, Some(20), vec![]))
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 10).unwrap();

    // And it is back: the fact is told again.
    w.store
        .write(|tx| {
            let mut task = tx.get_task("p", "T-9")?.expect("the task");
            task.state = TaskState::Done;
            tx.update_task(&task)
        })
        .unwrap();
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 15).unwrap();
    let items = balance_items(&mut w.store);
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(
        items[1].state,
        MsgState::Open,
        "the imbalance is told again"
    );
}

// ---- the quiet project: no lane has work left to start (T-232)

/// A project of two lanes: `heat` works on its last task, `deck` has nothing.
/// With `ready`, one more task of `heat` can be started.
fn two_lane_world(ready: bool) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "deck", SeatRole::Lane, DECK_PANE, "opencode", T0)?;
            lane_task(tx, "T-1", "heat", 1, TaskState::Running, Some(30), vec![])?;
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
            })?;
            tx.set_attempt_time("p", "T-1", 1, AttemptStep::Started, T0)?;
            if ready {
                lane_task(tx, "T-2", "heat", 2, TaskState::Ready, Some(30), vec![])?;
            }
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Working);
    let mut cfg = EngineCfg {
        tick_s: 4,
        cooldown_s: 30,
        doorbell_check_budget: 3,
        ..Default::default()
    };
    for kind in ["codex", "opencode"] {
        cfg.profiles.insert(kind.to_string(), profile(kind));
    }
    World { store, herdr, cfg }
}

/// T-232: with nothing startable anywhere no item opens for any lane, and the
/// IDLE item an earlier tick left open closes with the plain fact.
#[test]
fn no_item_is_opened_and_the_idle_item_closes_when_no_lane_has_work_left_to_start() {
    let mut w = two_lane_world(false);
    // What an earlier tick left open for the idle lane.
    w.store
        .write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "lane:deck".into(),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: MsgKind::System,
                urgent: false,
                body: "lane deck is IDLE: no next task to dispatch (its agent may still be finishing a turn); plan a task for it or move one to it".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0,
            })?;
            Ok(())
        })
        .unwrap();

    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 1).unwrap();

    assert_eq!(
        changes,
        vec![feed::FeedChange {
            lane: "deck".into(),
            action: "closed"
        }],
        "{changes:?}"
    );
    assert!(
        items(&mut w.store, "heat").is_empty(),
        "no item for the working lane"
    );
    let deck = items(&mut w.store, "deck");
    assert_eq!(deck.len(), 1, "{deck:?}");
    assert_eq!(deck[0].state, MsgState::Resolved);
    assert_eq!(
        deck[0].result.as_deref(),
        Some("no lane has work left to start")
    );
    assert!(balance_items(&mut w.store).is_empty(), "no imbalance item");
    for seat in ["p:heat", "p:deck"] {
        let severity = w
            .store
            .read(|tx| tx.get_meta(&format!("feed-severity:{seat}")))
            .unwrap();
        assert_eq!(severity.as_deref(), Some("0"), "{seat}");
    }

    // The next tick stays quiet too.
    assert!(feed::keep_lanes_fed(&mut w.store, "p", T0 + 5)
        .unwrap()
        .is_empty());
}

/// T-232: one startable task anywhere and the lanes are told as today.
#[test]
fn one_startable_task_keeps_the_lanes_talking() {
    let mut w = two_lane_world(true);

    let changes = feed::keep_lanes_fed(&mut w.store, "p", T0 + 1).unwrap();

    let heat = items(&mut w.store, "heat");
    assert_eq!(heat.len(), 1, "{heat:?}");
    assert_eq!(heat[0].state, MsgState::Open);
    assert_eq!(heat[0].body, "lane heat has 1 ready task left");
    let deck = items(&mut w.store, "deck");
    assert_eq!(deck.len(), 1, "{deck:?}");
    assert_eq!(deck[0].state, MsgState::Open);
    assert_eq!(deck[0].body, "lane deck is IDLE: no next task to dispatch (its agent may still be finishing a turn); plan a task for it or move one to it");
    assert_eq!(changes.len(), 2, "{changes:?}");
}
