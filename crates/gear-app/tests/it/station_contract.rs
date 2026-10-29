//! T-49: persisted stations from shared voyage facts, closed by plan decisions.
use gear_app::{draft, home, mail, plan, station, Env};
use gear_app::identity::Fingerprint;
use gear_model::{AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, AttemptRow, AttemptStep, MessageRow, NewMessage, Store};

const T0: i64 = 1_800_000_000;
const PANE: &str = "w1:p1";

fn captain() -> (Env, Fingerprint) {
    (Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some(PANE.into()), admin: false },
    Fingerprint { pane_id: PANE.into(), kind: Some("codex".into()), session_ref: None })
}

fn apply(store: &mut Store, decisions: &str, rid: &str, now: i64) -> gear_model::GearResult<plan::ApplyResult> {
    let base = store.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Decisions\n{decisions}\n");
    let parsed = draft::parse(&text).unwrap();
    let (env, fp) = captain();
    plan::apply(store, &env, Some(&fp), None, &parsed, &text, rid, now)
}

fn seed(store: &mut Store, eta: &str, count: usize) {
    store.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, PANE, "codex", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        Ok(())
    }).unwrap();
    let mut text = format!("<!-- gear-plan project=p base=0 -->\n# Milestone M2: course\ndone: ship it\nbar: stable\neta: {eta}\n# Queue\n");
    for n in 1..=count {
        let estimate = match n { 4 => 30, 5 => 60, _ => 10 };
        text.push_str(&format!("## NEW [must] lane=heat milestone=M2\ngoal: task {n}\naccept: works\nestimate: {estimate}\ndeps: -\n"));
    }
    let (env, fp) = captain();
    plan::apply(store, &env, Some(&fp), None, &draft::parse(&text).unwrap(), &text, "seed", T0).unwrap();
}

fn world(eta: &str, count: usize) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store, eta, count);
    store
}

fn finish(store: &mut Store, n: i64) {
    store.write(|tx| {
        let id = format!("T-{n}");
        let mut task = tx.get_task("p", &id)?.unwrap();
        task.state = TaskState::Done;
        tx.update_task(&task)?;
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(), task: id.clone(), n: 1, seat: "p:heat".into(), generation: 1,
            contract_version: 1, adopted_version: 1, pending_update: None,
            state: AttemptState::Accepted, candidate: None, report: None, safe_point: None,
            started: T0 + (n-1)*1200, updated: T0+n*1200,
        })?;
        tx.set_attempt_time("p", &id, 1, AttemptStep::Started, T0+(n-1)*1200)?;
        tx.set_attempt_time("p", &id, 1, AttemptStep::Settled, T0+n*1200)
    }).unwrap();
}

fn check(store: &mut Store, seconds: i64) -> Vec<String> {
    store.write(|tx| station::checkpoints(tx, "p", T0 + seconds)).unwrap()
}

fn item(store: &mut Store, id: &str) -> MessageRow {
    store.read(|tx| tx.get_message("p", id)).unwrap().unwrap()
}

#[test]
fn eta_quarters_open_once_and_keep_their_snapshots_open() {
    let mut store = world("4h", 5);
    assert!(check(&mut store, 3599).is_empty());
    let first = check(&mut store, 3600);
    assert_eq!(first.len(), 1);
    let message = item(&mut store, &first[0]);
    assert!(message.body.contains("25% eta"), "{}", message.body);
    assert_eq!(message.body.lines().count(), 2);
    // T-336: the voyage's goal line first, the text of today second.
    assert_eq!(message.body.lines().next(), Some("Your goal, voyage M2: done: ship it · bar: stable · 5 tasks left in the voyage. Judge every decision against it."), "{}", message.body);
    assert!(message.body.lines().nth(1).unwrap().starts_with("Station M2 ["), "{}", message.body);
    assert_eq!(message.state, MsgState::Open);
    assert!(check(&mut store, 3601).is_empty());
    assert!(check(&mut store, 7199).is_empty());
    let second = check(&mut store, 7200);
    assert_eq!(second.len(), 1);
    assert!(item(&mut store, &second[0]).body.contains("50% eta"));
    let third = check(&mut store, 10800);
    assert_eq!(third.len(), 1);
    assert!(item(&mut store, &third[0]).body.contains("75% eta"));
    // The course station (row B10) rings every 60 min of voyage time.
    let fourth = check(&mut store, 14400);
    assert_eq!(fourth.len(), 1);
    assert!(item(&mut store, &fourth[0]).body.contains("240 min"));
    assert_eq!(item(&mut store, &first[0]), message, "new stations do not mutate/close earlier snapshots");
}

#[test]
fn three_done_uses_exactly_the_shared_pace_projection_and_slowest_lane() {
    let mut store = world("8h", 5);
    for n in 1..=3 { finish(&mut store, n); }
    let ids = check(&mut store, 3600);
    assert_eq!(ids.len(), 1);
    let message = item(&mut store, &ids[0]);
    let report = store.read(|tx| gear_board::voyage::reports(tx, "p", T0+3600)).unwrap().remove(0);
    assert_eq!(report.pace(), Some(2.0));
    assert_eq!(report.remaining_estimate_min, Some(90));
    assert_eq!(report.projected_min, Some(60 + 90*2));
    assert_eq!(message.body, format!("Your goal, voyage M2: done: ship it · bar: stable · 5 tasks left in the voyage. Judge every decision against it.\nStation M2 [60 min, 3 done] · elapsed 60m / eta 8h · time left 420m · pace 2.00x · projected finish 240m from departure · tasks left 2 (90m) · lanes heat 180m · slowest lane heat 180m"));
    assert!(check(&mut store, 3601).is_empty());
    let (env, fp) = captain();
    let page = home::build(&mut store, &env, Some(&fp), T0+3601).unwrap();
    assert!(page.text.contains("next: gear plan apply gear/voyages/M2/plan.md"));
    assert!(page.text.contains(&format!("station {}: keep course|time-box|cut|move", ids[0])));
}

#[test]
fn simultaneous_and_missed_checkpoints_coalesce_into_one_line() {
    let mut store = world("4h", 8);
    for n in 1..=6 { finish(&mut store, n); }
    let ids = check(&mut store, 10801);
    assert_eq!(ids.len(), 1);
    let message = item(&mut store, &ids[0]);
    assert!(message.body.contains("25% eta, 50% eta, 75% eta, 3–6 done"), "{}", message.body);
    assert_eq!(message.body.lines().count(), 2);
    assert!(message.body.lines().next().unwrap().starts_with("Your goal, voyage M2: "), "{}", message.body);
    assert!(check(&mut store, 10802).is_empty());
}

#[test]
fn each_third_completion_is_a_station_including_the_final_task() {
    let mut store = world("8h", 6);
    assert!(check(&mut store, 0).is_empty()); // watch it while it is active
    for n in 1..=3 { finish(&mut store, n); }
    assert_eq!(check(&mut store, 3600).len(), 1);
    for n in 4..=6 { finish(&mut store, n); }
    let ids = check(&mut store, 7200);
    // The final station, and (L-78) the urgent "ready to land" item.
    assert_eq!(ids.len(), 2, "{ids:?}");
    let bodies: Vec<String> = ids.iter().map(|id| item(&mut store, id).body).collect();
    assert!(bodies.iter().any(|b| b.contains("[6 done]")), "{bodies:?}");
    let land = ids.iter().find(|id| item(&mut store, id).body.contains("ready to land")).unwrap().clone();
    assert!(item(&mut store, &land).urgent);
    // T-171: the landing item points at the three steps of the landing
    // (design §7.5): the report first, the user before the lanes.
    let land_body = item(&mut store, &land).body.clone();
    // T-336: the goal line first, the landing text second.
    assert!(land_body.starts_with("Your goal, voyage M2: done: ship it · bar: stable · "), "{land_body}");
    assert!(land_body.lines().nth(1).unwrap().starts_with("Voyage M2 is ready to land: all 6 tasks are done"), "{land_body}");
    assert!(land_body.contains("Run `gear report voyage`"), "{land_body}");
    assert!(land_body.contains("Review it with the user first"), "{land_body}");
    assert!(land_body.contains("Only then ask about reclaiming the lanes"), "{land_body}");
    assert!(land_body.contains("station <id>: land"), "{land_body}");
    apply(&mut store, &format!("- station {land}: land"), "land", T0 + 7300).unwrap();
    assert_eq!(item(&mut store, &land).state, MsgState::Resolved);
    assert!(check(&mut store, 14400).is_empty(), "landed voyages do not keep producing time stations");
    let mut historical = world("8h", 3);
    for n in 1..=3 { finish(&mut historical, n); }
    assert!(check(&mut historical, 20000).is_empty(), "no retroactive spam for old finished voyages");
}

fn reply(store: &mut Store, ids: Vec<String>, mode: mail::ReplyMode, rid: &str) -> gear_model::GearResult<mail::ReplyResult> {
    let (env, fp) = captain();
    mail::reply(store, &env, Some(&fp), &mail::ReplyArgs {
        ids, mode, text: "keep course".into(), wait_on: None, wait_until: Some(T0+10000),
        downgrade: false, request_id: rid.into(),
    }, T0+3601)
}

#[test]
fn only_a_plan_decision_naming_the_item_can_close_it() {
    let mut store = world("4h", 5);
    let id = check(&mut store, 3600).remove(0);
    for (mode, rid) in [(mail::ReplyMode::Resolve,"resolve"),(mail::ReplyMode::Wait,"wait")] {
        let error = reply(&mut store, vec![id.clone()], mode, rid).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat);
        assert!(error.detail.contains("# Decisions"));
        assert_eq!(item(&mut store, &id).state, MsgState::Open);
    }
    reply(&mut store, vec![id.clone()], mail::ReplyMode::Note, "note").unwrap();
    assert_eq!(item(&mut store, &id).state, MsgState::Open);
    apply(&mut store, "- keep course", "unrelated", T0+3602).unwrap();
    assert_eq!(item(&mut store, &id).state, MsgState::Open);
    let second = check(&mut store, 7200).remove(0);
    let decision = format!("- station {id}: time-box keep only the core");
    apply(&mut store, &decision, "decision", T0+7201).unwrap();
    let message = item(&mut store, &id);
    assert_eq!(message.state, MsgState::Resolved);
    assert_eq!(message.result.as_deref(), Some(decision.trim_start_matches("- ")));
    assert_eq!(message.resolved, Some(T0+7201));
    assert_eq!(item(&mut store, &second).state, MsgState::Open, "names exactly one item");
    apply(&mut store, &decision, "retained-decision", T0+7202).unwrap();
    assert_eq!(item(&mut store, &id), message, "retaining a decision in later plans is safe");
}

#[test]
fn invalid_station_decisions_and_mixed_replies_are_atomic() {
    let mut store = world("4h", 5);
    let id = check(&mut store, 3600).remove(0);
    let base = store.read(|tx| tx.plan_version("p")).unwrap();
    for (text, rid) in [
        (format!("- station {id}: done"), "bad-action"),
        (format!("- station {id}: keep course\n- station M-999: cut"), "bad-id"),
        (format!("- station {id}: cutting"), "bad-prefix"),
    ] {
        assert_eq!(apply(&mut store, &text, rid, T0+3601).unwrap_err().code, ErrorCode::BadFormat);
        assert_eq!(store.read(|tx| tx.plan_version("p")).unwrap(), base);
        assert_eq!(item(&mut store, &id).state, MsgState::Open);
    }
    let ordinary = store.write(|tx| Ok(tx.insert_message(&NewMessage {
        meta: None,
        project: "p".into(), thread: "ordinary".into(), task: None, attempt: None,
        sender: "p:heat".into(), sender_gen: 1, recipient: "p:captain".into(),
        kind: MsgKind::Question, urgent: false, body: "question".into(), reply_to: None,
        needs_ack: false, state: MsgState::Open, created: T0+3600,
    })?.id)).unwrap();
    assert!(reply(&mut store, vec![ordinary.clone(), id], mail::ReplyMode::Resolve, "mixed").is_err());
    assert_eq!(item(&mut store, &ordinary).state, MsgState::Open);
    assert!(apply(&mut store, &format!("- station {ordinary}: cut"), "not-station", T0+3601).is_err());
}

#[test]
fn unknown_eta_keeps_task_stations_and_unknown_measurements_honest() {
    // T-387: `apply` refuses a prose eta, so the seed is a duration and the
    // prose is written the way a row older than T-387 carries it.
    let mut store = world("4h", 5);
    store.write(|tx| {
        let mut voyage = tx.get_voyage("p", "M2")?.unwrap();
        voyage.eta = Some("soon".into());
        tx.upsert_voyage(&voyage)
    }).unwrap();
    // An unknown eta rings no eta station; the course station (row B10) does
    // not need one: the voyage time is known.
    let ids = check(&mut store, 3600);
    assert_eq!(ids.len(), 1);
    assert!(!item(&mut store, &ids[0]).body.contains("% eta"), "{}", item(&mut store, &ids[0]).body);
    // Legacy completions may have no attempt timestamps.
    store.write(|tx| {
        for n in 1..=3 {
            let mut task = tx.get_task("p", &format!("T-{n}"))?.unwrap();
            task.state = TaskState::Done;
            tx.update_task(&task)?;
        }
        Ok(())
    }).unwrap();
    let ids = check(&mut store, 3601);
    assert_eq!(ids.len(), 1);
    let body = item(&mut store, &ids[0]).body;
    assert!(body.contains("pace - · projected finish -"), "{body}");
    assert!(body.contains("3 done"));
}

/// Design §10.7 row B10: besides the 25/50/75% stations, a course station
/// every 60 min of voyage time. Its text gives elapsed / eta, the time left,
/// the per-lane projection and the lane that sets it, and the tasks left; it
/// is urgent when the projected finish is past the eta; it is answered in
/// `# Decisions` like any station.
#[test]
fn a_course_station_every_hour_answers_like_any_station() {
    let mut store = world("12h", 5);
    for n in 1..=2 { finish(&mut store, n); }
    assert!(check(&mut store, 3599).is_empty(), "no station before the first hour");
    let first = check(&mut store, 3600);
    assert_eq!(first.len(), 1, "one station at 60 min");
    let message = item(&mut store, &first[0]);
    let body = message.body.clone();
    assert!(body.contains("60 min"), "{body}");
    assert!(body.contains("elapsed 60m / eta 12h"), "{body}");
    assert!(body.contains("time left 660m"), "{body}");
    assert!(body.contains("lanes heat 200m"), "{body}");
    assert!(body.contains("slowest lane heat 200m"), "{body}");
    assert!(body.contains("tasks left 3 (100m)"), "{body}");
    assert!(!message.urgent, "the projection (240m) is inside the eta (12h)");
    assert!(check(&mut store, 3601).is_empty(), "one station per hour");
    let second = check(&mut store, 7200);
    assert_eq!(second.len(), 1, "one station at 120 min");
    assert!(item(&mut store, &second[0]).body.contains("120 min"));
    // Answered in `# Decisions` like any station.
    let decision = format!("- station {}: time-box keep the core", first[0]);
    apply(&mut store, &decision, "course", T0 + 7201).unwrap();
    assert_eq!(item(&mut store, &first[0]).state, MsgState::Resolved);
    assert_eq!(item(&mut store, &second[0]).state, MsgState::Open, "names exactly one item");
}

#[test]
fn a_course_station_is_urgent_when_the_projection_is_past_the_eta() {
    let mut store = world("1h", 5);
    for n in 1..=3 { finish(&mut store, n); }
    let ids = check(&mut store, 3600);
    assert_eq!(ids.len(), 1);
    let message = item(&mut store, &ids[0]);
    assert!(message.body.contains("projected finish 240m from departure"), "{}", message.body);
    assert!(message.urgent, "the projected finish (240m) is past the eta (1h)");
}

/// T-414: the eta markers do not stop at 75%. A voyage still sailing past its
/// eta keeps getting one station per crossed quarter, each naming the quarter
/// and how far past the eta the voyage is.
#[test]
fn a_voyage_past_its_eta_keeps_getting_quarter_stations() {
    let mut store = world("8h", 5);
    let at_eta = check(&mut store, 9 * 3600);
    assert_eq!(at_eta.len(), 1, "{at_eta:?}");
    let message = item(&mut store, &at_eta[0]);
    assert!(message.body.contains("100% eta"), "{}", message.body);
    assert!(message.urgent, "a crossed 100% quarter is urgent");
    assert!(check(&mut store, 9 * 3600).is_empty(), "one station per quarter");
    assert!(check(&mut store, 9 * 3600 + 1).is_empty(), "no second station for the same quarter");
    let later = check(&mut store, 10 * 3600);
    assert_eq!(later.len(), 1, "{later:?}");
    let body = item(&mut store, &later[0]).body;
    assert!(body.contains("125% eta (past eta by 120 min)"), "{body}");
    assert!(item(&mut store, &later[0]).urgent, "a crossed 100%+ quarter is urgent");
    assert!(body.contains("Past the eta: speed up (rebalance lanes, cut waiting, fix rework fast); the eta does not move."), "{body}");
    assert!(check(&mut store, 10 * 3600).is_empty(), "one station per quarter");
}

#[test]
fn a_landed_voyage_gets_no_quarter_station() {
    let mut store = world("8h", 5);
    assert!(check(&mut store, 0).is_empty()); // watch it while it is active
    for n in 1..=5 { finish(&mut store, n); }
    let ids = check(&mut store, 10 * 3600);
    assert!(ids.iter().any(|id| item(&mut store, id).body.contains("ready to land")), "{ids:?}");
    for id in &ids {
        let body = item(&mut store, id).body;
        assert!(!body.contains("% eta"), "{body}");
    }
    assert!(check(&mut store, 11 * 3600).is_empty());
}

#[test]
fn the_first_three_quarters_are_unchanged() {
    let mut store = world("4h", 5);
    for (seconds, quarter) in [(3600, "25% eta"), (7200, "50% eta"), (10800, "75% eta")] {
        let ids = check(&mut store, seconds);
        assert_eq!(ids.len(), 1, "{ids:?}");
        let message = item(&mut store, &ids[0]);
        assert!(message.body.contains(quarter), "{}", message.body);
        assert!(!message.body.contains("past eta by"), "{}", message.body);
        assert!(!message.body.contains("Past the eta:"), "{}", message.body);
    }
}
