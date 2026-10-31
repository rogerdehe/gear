//! T-71: no seat is rung with an earlier job's or an earlier voyage's
//! messages, and a new agent gets its task before any doorbell.
//!
//! The trial found both: a fresh reviewer first read "6 item(s) waiting for
//! you (1 urgent)" and only then its packet (on T-61 both went into its pane
//! in one second and the packet was swallowed), and a re-registered lane spent
//! its first turn on the previous voyage's messages. A warm temp seat keeps
//! its id across jobs, so the seat's *generation* — not its id — says which
//! messages are its own.

use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState};
use gear_store::{
    testing, BindingRow, BindingStatus, ContractRow, NewMessage, NewOp, SeatRow, Store, TaskRow,
};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const REV_PANE: &str = "w9:p1";
/// When the seat's current generation started: the new job (or the
/// re-registration) opened the binding of that generation.
const JOB_STARTED: i64 = T0 + 100;

fn profile(kind: &str) -> gear_herdr::Profile {
    gear_herdr::Profile {
        kind: kind.into(),
        new_session: String::new(),
        compact: String::new(),
        usage_log: None,
        error_markers: Vec::new(),
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

fn world() -> World {
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
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
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

impl World {
    /// Move a seat to its next generation at `JOB_STARTED`: the binding of the
    /// new generation is opened then, exactly as `begin_onboard` does it.
    fn next_generation(&mut self, seat: &SeatRow, pane: &str) {
        self.store
            .write(|tx| {
                tx.set_binding_status(&seat.id, seat.generation, BindingStatus::Void)?;
                tx.set_seat_generation(&seat.id, seat.generation + 1)?;
                tx.insert_binding(&BindingRow {
                    seat: seat.id.clone(),
                    generation: seat.generation + 1,
                    session: "sess".into(),
                    pane_id: pane.into(),
                    kind: seat.profile.clone(),
                    session_ref: None,
                    status: BindingStatus::Active,
                    handover_id: None,
                    created: JOB_STARTED,
                })
            })
            .unwrap();
    }

    /// A reply from the captain (`messages_to_since` carries replies and
    /// instructions).
    fn reply(&mut self, to: &str, task: &str, body: &str, at: i64) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_message(&NewMessage {
                        meta: None,
                        project: "p".into(),
                        thread: format!("{task}:1"),
                        task: Some(task.into()),
                        attempt: Some(1),
                        sender: "p:captain".into(),
                        sender_gen: 1,
                        recipient: to.into(),
                        kind: MsgKind::Reply,
                        urgent: false,
                        body: body.into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Info,
                        created: at,
                    })?
                    .id)
            })
            .unwrap()
    }

    /// An open question for `to`: an action item, not a reply.
    fn question(&mut self, to: &str, body: &str, at: i64) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_message(&NewMessage {
                        meta: None,
                        project: "p".into(),
                        thread: format!("q:{}", at),
                        task: None,
                        attempt: None,
                        sender: "p:captain".into(),
                        sender_gen: 1,
                        recipient: to.into(),
                        kind: MsgKind::Question,
                        urgent: false,
                        body: body.into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Open,
                        created: at,
                    })?
                    .id)
            })
            .unwrap()
    }

    /// An open item for `to`, from `sender`, in its own thread: the batch
    /// limits group threads by sender, so a test of the batch needs senders.
    fn item(
        &mut self,
        from: &str,
        to: &str,
        thread: &str,
        body: &str,
        urgent: bool,
        at: i64,
    ) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_message(&NewMessage {
                        meta: None,
                        project: "p".into(),
                        thread: thread.into(),
                        task: None,
                        attempt: None,
                        sender: from.into(),
                        sender_gen: 0,
                        recipient: to.into(),
                        kind: MsgKind::System,
                        urgent,
                        body: body.into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Open,
                        created: at,
                    })?
                    .id)
            })
            .unwrap()
    }

    /// An open item the engine wrote for `to`, in its own thread (T-254: the
    /// engine's own items, the ones the captain was rung for).
    fn engine_item(&mut self, to: &str, thread: &str, body: &str, at: i64) -> String {
        self.item("engine", to, thread, body, false, at)
    }

    /// The review packet of T-7, due for the reviewer's pane.
    fn packet(&mut self, seat: &SeatRow) -> String {
        self.store
            .write(|tx| {
                Ok(tx
                    .insert_op(&NewOp {
                        project: "p".into(),
                        seat: seat.id.clone(),
                        generation: seat.generation,
                        kind: OpKind::Deliver,
                        task: Some("T-7".into()),
                        attempt: Some(1),
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({ "text": "Review T-7 attempt 1" }),
                        created: JOB_STARTED,
                    })?
                    .id)
            })
            .unwrap()
    }

    /// The packet was sent and taken: the delivery op is settled.
    fn confirm_packet(&mut self, id: &str) {
        self.store
            .write(|tx| {
                tx.set_op_state("p", id, OpState::Executing, None, JOB_STARTED + 1)?;
                tx.set_op_state("p", id, OpState::Submitted, None, JOB_STARTED + 1)?;
                tx.set_op_state("p", id, OpState::Confirmed, Some("taken"), JOB_STARTED + 1)
            })
            .unwrap();
    }
}

/// A warm reviewer seat on its second job: `review-1`, generation 2 (started
/// at `JOB_STARTED`), bound to the task it reviews now, its pane up and idle.
fn warm_reviewer(w: &mut World) -> SeatRow {
    let seat = w
        .store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-7".into(),
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
                project: "p".into(),
                task: "T-7".into(),
                version: 1,
                goal: "g".into(),
                accept: "a".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            // The attempt the reviewer reads: handed in, its candidate pushed.
            gear_app::tasks::start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
            let mut attempt = tx.attempts_of_task("p", "T-7")?.pop().unwrap();
            attempt.state = gear_model::AttemptState::Submitted;
            attempt.candidate = Some("abc1234".into());
            tx.update_attempt(&attempt)?;
            let mut task = tx.get_task("p", "T-7")?.unwrap();
            task.state = TaskState::Verifying;
            tx.update_task(&task)?;
            let seat = testing::seed_active_seat(
                tx,
                "p",
                "review-1",
                SeatRole::Temp,
                REV_PANE,
                "codex",
                T0,
            )?;
            tx.set_seat_task(&seat.id, "T-7")?;
            Ok(seat)
        })
        .unwrap();
    w.next_generation(&seat, REV_PANE);
    w.herdr.add_agent(REV_PANE, "codex", AgentStatus::Idle);
    w.herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    w.store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap()
}

/// A lane re-registered for this voyage: generation 2, started at
/// `JOB_STARTED`, its pane up and idle.
fn reregistered_lane(w: &mut World) -> SeatRow {
    let seat = w
        .store
        .write(|tx| {
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)
        })
        .unwrap();
    w.next_generation(&seat, LANE_PANE);
    w.herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    w.store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap()
}

#[test]
fn a_warm_reviewer_gets_its_packet_before_any_doorbell() {
    let mut w = world();
    let seat = warm_reviewer(&mut w);
    // An earlier job's answer, older than this generation.
    w.reply(&seat.id, "T-5", "judge v2", T0 + 10);
    // This job's packet is due in the same tick.
    w.packet(&seat);

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 10).unwrap();

    let said = w.herdr.prompts_to(REV_PANE);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("Review T-7"),
        "the packet comes first: {said:?}"
    );
    assert!(
        !said[0].contains("GEAR:"),
        "and no doorbell with it: {said:?}"
    );
}

#[test]
fn an_earlier_jobs_answer_does_not_ring_the_new_job() {
    let mut w = world();
    let seat = warm_reviewer(&mut w);
    // The reply the trial saw: an answer to the seat's earlier job, never
    // shown to this generation. And an open question of that job too.
    w.reply(&seat.id, "T-5", "judge v2", T0 + 10);
    w.question(&seat.id, "M2: judge v2", T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 10).unwrap();

    assert!(
        w.herdr.prompts_to(REV_PANE).is_empty(),
        "no doorbell for another job's answer: {:?}",
        w.herdr.prompts_to(REV_PANE)
    );
}

#[test]
fn a_reply_to_the_current_job_rings_after_the_packet() {
    let mut w = world();
    let seat = warm_reviewer(&mut w);
    let packet = w.packet(&seat);
    w.confirm_packet(&packet);
    // The captain's answer to this job, after the packet.
    w.reply(&seat.id, "T-7", "judge v2 on this one", JOB_STARTED + 5);

    // Within the reviewer's nudge cooldown: this tick is about the doorbell.
    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 20).unwrap();

    let said = w.herdr.prompts_to(REV_PANE);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("1 item(s) waiting for you"), "{said:?}");
}

#[test]
fn a_reregistered_lane_starts_with_its_task_not_a_doorbell() {
    let mut w = world();
    let seat = reregistered_lane(&mut w);
    // What the previous voyage left open for this lane's name.
    w.question(&seat.id, "M2: which lane takes the runtime?", T0 + 10);
    // And this voyage's first task, ready.
    w.store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
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
            })
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 10).unwrap();

    let said = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("New task T-1"),
        "the task comes first: {said:?}"
    );
    assert!(
        !said[0].contains("GEAR:"),
        "and no doorbell with it: {said:?}"
    );
}

#[test]
fn an_earlier_voyages_question_does_not_ring_a_reregistered_lane() {
    let mut w = world();
    let seat = reregistered_lane(&mut w);
    // Open since the previous voyage; nothing is ready to deliver this tick.
    w.question(&seat.id, "M2: which lane takes the runtime?", T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 10).unwrap();

    assert!(
        w.herdr.prompts_to(LANE_PANE).is_empty(),
        "no doorbell for an earlier voyage's question: {:?}",
        w.herdr.prompts_to(LANE_PANE)
    );
}

#[test]
fn a_tick_with_a_doorbell_and_a_delivery_for_one_seat_sends_only_the_delivery() {
    let mut w = world();
    let seat = reregistered_lane(&mut w);
    // A message of this generation (it may ring) and a task (a delivery).
    w.question(&seat.id, "here is today's briefing", JOB_STARTED + 5);
    w.store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
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
            })
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 10).unwrap();

    let said = w.herdr.prompts_to(LANE_PANE);
    assert_eq!(said.len(), 1, "only the delivery: {said:?}");
    assert!(said[0].contains("New task T-1"), "{said:?}");
}

#[test]
fn the_captains_open_items_still_ring() {
    let mut w = world();
    // The engine's own item for the captain, written this generation.
    w.question("p:captain", "merge of T-5 failed: conflict", T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 20).unwrap();

    let said = w.herdr.prompts_to(CAP_PANE);
    assert!(
        said.iter().any(|text| text.contains("GEAR:")),
        "the captain is rung: {said:?}"
    );
}

#[test]
fn a_late_answer_about_the_previous_task_does_not_reach_the_new_job() {
    let mut w = world();
    let seat = warm_reviewer(&mut w);
    // The captain answers late: about the task the seat reviewed *before*,
    // but written after this job started. The generation alone would let it
    // through (F1 of the T-71 review).
    w.reply(&seat.id, "T-5", "judge v2", JOB_STARTED + 20);

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 30).unwrap();

    assert!(
        w.herdr.prompts_to(REV_PANE).is_empty(),
        "no doorbell for the previous task: {:?}",
        w.herdr.prompts_to(REV_PANE)
    );
    let home = home_of(&mut w, &seat, REV_PANE, "codex", JOB_STARTED + 30);
    assert!(
        !home.text.contains("judge v2"),
        "and it is not shown: {}",
        home.text
    );
}

/// `gear` for a seat, as the CLI builds it.
fn home_of(
    w: &mut World,
    seat: &SeatRow,
    pane: &str,
    kind: &str,
    now: i64,
) -> gear_app::home::Home {
    let env = gear_app::Env {
        ctx: Some(gear_model::GearCtx {
            project: "p".into(),
            seat: seat.name.clone(),
            generation: seat.generation,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    };
    let fp = gear_app::identity::Fingerprint {
        pane_id: pane.into(),
        kind: Some(kind.into()),
        session_ref: None,
    };
    gear_app::home::build(&mut w.store, &env, Some(&fp), now).unwrap()
}

#[test]
fn an_open_item_from_before_a_new_captain_generation_still_rings() {
    let mut w = world();
    // A new captain generation (a handover), and an item that was already
    // open before it: the captain's inbox is the engine's business, so the
    // item must ring (F2 of the T-71 review).
    let captain = w
        .store
        .read(|tx| tx.get_seat("p:captain"))
        .unwrap()
        .unwrap();
    w.next_generation(&captain, CAP_PANE);
    w.question("p:captain", "merge of T-5 failed: conflict", T0 + 10);

    tick(&mut w.store, &mut w.herdr, &w.cfg, JOB_STARTED + 30).unwrap();

    let said = w.herdr.prompts_to(CAP_PANE);
    assert!(
        said.iter().any(|text| text.contains("GEAR:")),
        "the captain is rung for its open item: {said:?}"
    );
}

#[test]
fn a_new_reviewers_gear_shows_no_message_from_an_earlier_job() {
    let mut w = world();
    let seat = warm_reviewer(&mut w);
    // An earlier job's answer and this job's answer.
    w.reply(&seat.id, "T-5", "judge v2", T0 + 10);
    w.reply(&seat.id, "T-7", "judge this one", JOB_STARTED + 50);

    let home = home_of(&mut w, &seat, REV_PANE, "codex", JOB_STARTED + 60);

    assert!(home.text.contains("judge this one"), "{}", home.text);
    assert!(
        !home.text.contains("judge v2"),
        "another job's answer is not this job's instruction: {}",
        home.text
    );
}

// ---------------------------------------------------------------- T-254

/// The message ids a doorbell text names, in order.
fn named_by(text: &str) -> Vec<String> {
    text.split_once(". Run")
        .unwrap()
        .0
        .split_whitespace()
        .filter(|word| word.starts_with("M-"))
        .map(str::to_string)
        .collect()
}

/// The doorbells rung at `pane`, in the order they were typed.
fn doorbells_to<'a>(w: &'a World, pane: &str) -> Vec<&'a str> {
    w.herdr
        .prompts_to(pane)
        .into_iter()
        .filter(|text| text.contains("item(s) waiting for you"))
        .collect()
}

/// Three open engine threads from one sender: with `per_sender` 2 the batch
/// keeps two of them, so the captain must be rung for those two only.
#[test]
fn the_captains_doorbell_names_only_the_items_its_gear_shows() {
    let mut w = world();
    let first = w.engine_item(
        "p:captain",
        "merge:1",
        "T-305: merge of T-305 failed",
        T0 + 10,
    );
    let second = w.engine_item(
        "p:captain",
        "merge:2",
        "T-310: merge of T-310 failed",
        T0 + 20,
    );
    let third = w.engine_item(
        "p:captain",
        "merge:3",
        "T-341: merge of T-341 failed",
        T0 + 30,
    );

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 40).unwrap();

    let said = doorbells_to(&w, CAP_PANE);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(
        named_by(&said[0]),
        vec![first.clone(), second.clone()],
        "{}",
        said[0]
    );

    // And that is what its own `gear` shows: the same two items, not the third.
    let captain = w
        .store
        .read(|tx| tx.get_seat("p:captain"))
        .unwrap()
        .unwrap();
    let home = home_of(&mut w, &captain, CAP_PANE, "codex", T0 + 40);
    assert!(
        home.text.contains(&first) && home.text.contains(&second),
        "{}",
        home.text
    );
    assert!(!home.text.contains(&third), "{}", home.text);
}

/// Once one of the two is resolved, the third enters the batch — and the
/// doorbell names it.
#[test]
fn a_resolved_item_lets_the_next_one_into_the_captains_doorbell() {
    let mut w = world();
    let first = w.engine_item(
        "p:captain",
        "merge:1",
        "T-305: merge of T-305 failed",
        T0 + 10,
    );
    let second = w.engine_item(
        "p:captain",
        "merge:2",
        "T-310: merge of T-310 failed",
        T0 + 20,
    );
    let third = w.engine_item(
        "p:captain",
        "merge:3",
        "T-341: merge of T-341 failed",
        T0 + 30,
    );

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 40).unwrap();
    let said = doorbells_to(&w, CAP_PANE);
    assert_eq!(
        named_by(&said[0]),
        vec![first.clone(), second.clone()],
        "{}",
        said[0]
    );

    // The captain answers the first: the item is resolved, and the doorbell
    // that named it was taken (so its slot and cooldown are over).
    w.store
        .write(|tx| {
            tx.update_message_state(
                "p",
                &first,
                MsgState::Resolved,
                Some("merged"),
                None,
                None,
                T0 + 50,
            )?;
            for op in tx.ops_of_seat_kind("p:captain", OpKind::Doorbell)? {
                tx.set_op_state("p", &op.id, OpState::Confirmed, Some("taken"), T0 + 50)?;
            }
            Ok(())
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 100).unwrap();

    let said = doorbells_to(&w, CAP_PANE);
    assert_eq!(said.len(), 2, "a second doorbell: {said:?}");
    assert_eq!(
        named_by(&said[1]),
        vec![second.clone(), third.clone()],
        "{}",
        said[1]
    );
}

// ---------------------------------------------------------------- T-319

/// The threads `gear` prints under INBOX, each with its `[urgent]` mark and the
/// message ids it lists.
fn inbox_threads(home: &str) -> Vec<(bool, Vec<String>)> {
    let mut threads = Vec::new();
    let mut inside = false;
    for line in home.lines() {
        if line.starts_with("INBOX (") {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with("    ") {
            // The thread's own bodies and its `+n more` line.
            continue;
        }
        if !line.starts_with("  ") {
            break;
        }
        let urgent = line.contains("[urgent]");
        let ids = line
            .split_whitespace()
            .filter(|word| word.starts_with("M-"))
            .map(str::to_string)
            .collect();
        threads.push((urgent, ids));
    }
    threads
}

/// The `(<k> urgent)` count of a doorbell text, zero when it carries none.
fn urgent_in_doorbell(text: &str) -> usize {
    let Some(start) = text.find(" (") else {
        return 0;
    };
    text[start + 2..]
        .split(" urgent)")
        .next()
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0)
}

/// Trial B04-01: the captain's doorbell named an open urgent item its own
/// `gear` did not list. T-254 fixed the cause (the doorbell keeps only the ids
/// in the home page's batch); this is the case that broke, with more items than
/// one batch holds: eight open items in eight threads from three senders, three
/// of them urgent. With `per_sender` 2 the second urgent thread of one sender
/// falls out of the batch, and an urgent item must not be rung for either.
#[test]
fn an_urgent_item_past_the_batch_is_not_rung_and_enters_it_later() {
    let mut w = world();
    // Sender a: one normal thread and two urgent ones. The oldest normal thread
    // takes the reserved slot, so `per_sender` 2 keeps one of the urgent ones
    // and pushes the other past the batch.
    w.item(
        "p:lane-a",
        "p:captain",
        "merge:a1",
        "T-301: merge of T-301 failed",
        false,
        T0 + 10,
    );
    w.item(
        "p:lane-a",
        "p:captain",
        "merge:a2",
        "T-302: merge of T-302 failed",
        true,
        T0 + 20,
    );
    let past = w.item(
        "p:lane-a",
        "p:captain",
        "merge:a3",
        "T-303: merge of T-303 failed",
        true,
        T0 + 30,
    );
    w.item(
        "p:lane-b",
        "p:captain",
        "merge:b1",
        "T-304: merge of T-304 failed",
        false,
        T0 + 40,
    );
    w.item(
        "p:lane-b",
        "p:captain",
        "merge:b2",
        "T-305: merge of T-305 failed",
        true,
        T0 + 50,
    );
    let normal_past = w.item(
        "p:lane-b",
        "p:captain",
        "merge:b3",
        "T-306: merge of T-306 failed",
        false,
        T0 + 60,
    );
    w.item(
        "p:lane-c",
        "p:captain",
        "merge:c1",
        "T-307: merge of T-307 failed",
        false,
        T0 + 70,
    );
    w.item(
        "p:lane-c",
        "p:captain",
        "merge:c2",
        "T-308: merge of T-308 failed",
        false,
        T0 + 80,
    );

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 100).unwrap();

    let said: Vec<String> = doorbells_to(&w, CAP_PANE)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    let named = named_by(&said[0]);
    assert_eq!(named.len(), 6, "one batch holds six threads: {}", said[0]);
    assert!(
        !named.contains(&past),
        "the urgent item past the batch is not rung: {}",
        said[0]
    );

    // The home page lists the same items: every id the doorbell names is under
    // INBOX, and the doorbell's urgent count is the urgent threads the INBOX
    // marks among them.
    let captain = w
        .store
        .read(|tx| tx.get_seat("p:captain"))
        .unwrap()
        .unwrap();
    let home = home_of(&mut w, &captain, CAP_PANE, "codex", T0 + 100);
    let threads = inbox_threads(&home.text);
    let listed: Vec<&String> = threads.iter().flat_map(|(_, ids)| ids.iter()).collect();
    assert_eq!(threads.len(), 6, "{}", home.text);
    for id in &named {
        assert!(
            listed.contains(&id),
            "{id} is named but not in the INBOX: {}",
            home.text
        );
    }
    let urgent_listed = threads.iter().filter(|(urgent, _)| *urgent).count();
    assert_eq!(
        urgent_in_doorbell(&said[0]),
        urgent_listed,
        "the doorbell's urgent count is the INBOX's: {} | {}",
        said[0],
        home.text
    );

    // Once the captain answers the six, the ones past the batch come into it,
    // and the next doorbell names them — the same items the home page lists.
    w.store
        .write(|tx| {
            for id in &named {
                tx.update_message_state(
                    "p",
                    id,
                    MsgState::Resolved,
                    Some("handled"),
                    None,
                    None,
                    T0 + 200,
                )?;
            }
            for op in tx.ops_of_seat_kind("p:captain", OpKind::Doorbell)? {
                tx.set_op_state("p", &op.id, OpState::Confirmed, Some("taken"), T0 + 200)?;
            }
            Ok(())
        })
        .unwrap();

    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 300).unwrap();

    let said: Vec<String> = doorbells_to(&w, CAP_PANE)
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(said.len(), 2, "a second doorbell: {said:?}");
    let next = named_by(&said[1]);
    assert_eq!(next.len(), 2, "the two past the batch: {}", said[1]);
    assert!(
        next.contains(&past) && next.contains(&normal_past),
        "the items that came into the batch are named: {}",
        said[1]
    );

    let home = home_of(&mut w, &captain, CAP_PANE, "codex", T0 + 300);
    let listed: Vec<String> = inbox_threads(&home.text)
        .into_iter()
        .flat_map(|(_, ids)| ids)
        .collect();
    for id in &next {
        assert!(
            listed.contains(id),
            "{id} is named but not in the INBOX: {}",
            home.text
        );
    }
    assert_eq!(
        urgent_in_doorbell(&said[1]),
        1,
        "one of the two is urgent: {}",
        said[1]
    );
}
