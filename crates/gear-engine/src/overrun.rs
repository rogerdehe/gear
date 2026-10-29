//! Round clocks and the overrun question (design §5.1–§5.2).
use gear_herdr::HerdrAct;
use gear_model::{AttemptState, GearResult, MsgKind, MsgState, SeatState, TaskState, Ts};
use gear_store::{NewMessage, RoundKey, RoundRow, Store, Tx};
use serde_json::{json, Value};
use crate::EngineCfg;

fn key(round: &RoundRow) -> String { format!("engine.overrun.{}.{}.{}.{}", round.project, round.task, round.attempt, round.role) }
fn round_key(round: &RoundRow) -> RoundKey { RoundKey { project: round.project.clone(), task: round.task.clone(), attempt: round.attempt, role: round.role.clone() } }
fn text(task: &str, attempt: i64, contract: i64, budget: i64, active: i64) -> String {
    format!("GEAR: {task} has reached its estimate ({budget} min; {} min so far).\nAnswer with one command, then keep working:\ngear send --kind progress --task {task} --attempt {attempt} --contract {contract} --left <minutes> --blocker \"<what stops you, or none>\" \"<done so far; what is left>\"", active / 60)
}

/// Union of the seat's pauses in a time window: overlapping quota and wait
/// pauses count only once.
fn paused(tx: &Tx<'_>, round: &RoundRow, start: Ts, end: Ts) -> GearResult<i64> {
    let mut intervals: Vec<(Ts, Ts)> = tx.pauses_of_project(&round.project)?.into_iter()
        .filter(|p| p.seat == round.seat)
        .map(|p| (p.start_at.max(start), p.end_at.unwrap_or(end).min(end)))
        .filter(|(a, b)| b > a).collect();
    intervals.sort_unstable();
    let mut total = 0;
    let mut current: Option<(Ts, Ts)> = None;
    for (a, b) in intervals {
        match current {
            Some((x, y)) if a <= y => current = Some((x, y.max(b))),
            Some((x, y)) => { total += y - x; current = Some((a, b)); },
            None => current = Some((a, b)),
        }
    }
    if let Some((a, b)) = current { total += b - a; }
    Ok(total)
}
fn active_at(tx: &Tx<'_>, round: &RoundRow, start: Ts, end: Ts) -> GearResult<i64> {
    Ok((end - start - paused(tx, round, start, end)?).max(0))
}

fn answers(tx: &Tx<'_>, round: &RoundRow) -> GearResult<Vec<gear_store::MessageRow>> {
    let mut messages = tx.progress_of_attempt(&round.project, &round.task, round.attempt)?;
    messages.retain(|m| m.sender == round.seat && m.meta.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()).and_then(|v| v.get("left").and_then(Value::as_i64)).is_some());
    messages.sort_by_key(|m| (m.created, m.id.trim_start_matches("M-").parse::<u64>().unwrap_or(0)));
    Ok(messages)
}

fn item(tx: &Tx<'_>, round: &RoundRow, body: &str, urgent: bool, now: Ts) -> GearResult<()> {
    let thread = format!("overrun:{}:{}", round.task, round.role);
    let Some(captain) = tx.captain_of(&round.project)? else { return Ok(()) };
    let existing = tx.thread_messages(&round.project, &thread)?.into_iter().find(|m| m.recipient == captain.id && m.kind == MsgKind::System && m.attempt == Some(round.attempt) && m.sender == round.seat);
    if let Some(existing) = existing {
        tx.update_message_body(&round.project, &existing.id, body)?;
        tx.set_message_urgent(&round.project, &existing.id, urgent)?;
        if existing.state == MsgState::Resolved { tx.update_message_state(&round.project, &existing.id, MsgState::Open, None, None, None, now)?; }
    } else {
        tx.insert_message(&NewMessage { project: round.project.clone(), thread, task: Some(round.task.clone()), attempt: Some(round.attempt), sender: round.seat.clone(), sender_gen: round.generation, recipient: captain.id, kind: MsgKind::System, urgent, body: body.into(), reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None })?;
    }
    Ok(())
}

fn process(tx: &Tx<'_>, round: &RoundRow, now: Ts) -> GearResult<bool> {
    let Some(attempt) = tx.get_attempt(&round.project, &round.task, round.attempt)? else { return Ok(false) };
    let Some(task) = tx.get_task(&round.project, &round.task)? else { return Ok(false) };
    let times = tx.attempt_times(&round.project, &round.task, round.attempt)?;
    let review = round.role == "review";
    let start = if review { Some(round.start_at) } else { times.started };
    let Some(start) = start else { return Ok(false) };
    let verdict = if review { tx.get_review_verdict(&round.project, &round.task, round.attempt)?.map(|v| v.created) } else { None };
    // A review round ends when its attempt is settled by anyone (T-381): a
    // captain's verdict writes neither a review verdict nor an `end_at`, so the
    // round would stay live for ever, its active time would grow and its
    // "no answer" item would come back every 15 minutes. A review round is thus
    // live only while its attempt is still handed in and its task is still with
    // the reviewer; once it is not, the round stops when it was settled.
    let settled = review && (attempt.state != AttemptState::Submitted || task.state != TaskState::Verifying);
    let stop = if review {
        match verdict {
            Some(at) => Some(at),
            None if settled => Some(times.settled.unwrap_or(now)),
            None => None,
        }
    } else { times.handed_in };
    let end = stop.unwrap_or(now).min(round.end_at.unwrap_or(now)).min(now).max(start);
    let pause = paused(tx, round, start, end)?;
    let total_pause = pause.max(round.paused_s);
    if total_pause > round.paused_s { tx.add_round_pause(&round_key(round), total_pause - round.paused_s)?; }
    let active = (end - start - total_pause).max(0);
    tx.set_round_active(&round_key(round), active)?;
    let live = if review { verdict.is_none() && round.end_at.is_none() && !settled } else { matches!(attempt.state, AttemptState::Started | AttemptState::Stopped) && stop.is_none() };
    let Some(seat) = tx.get_seat(&round.seat)? else { return Ok(false) };
    let budget = tx.round_budget_min(&round.project, &round.task, round.attempt, &round.role, round.model.as_deref())?;
    let state_key = key(round);
    let mut state = tx.get_meta(&state_key)?.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_else(|| json!({"next_active":budget*60,"asked_at":null,"asked_active":0,"answer_id":null,"waiting":false,"no_answer":false}));
    let previous_budget = state.get("budget").and_then(Value::as_i64).unwrap_or(budget);
    if budget > previous_budget {
        let next = state["next_active"].as_i64().unwrap_or(0).max(budget * 60);
        state["next_active"] = json!(next);
    }
    state["budget"] = json!(budget);
    let asked_at = state.get("asked_at").and_then(Value::as_i64).filter(|_| !settled);
    let mut changed = false;
    if let Some(asked) = asked_at {
        let last_id = state["answer_id"].as_str().and_then(|id| id.trim_start_matches("M-").parse::<u64>().ok()).unwrap_or(0);
        if let Some(answer) = answers(tx, round)?.into_iter().rev().find(|m| m.created >= asked && m.created >= start && m.created <= end && m.id.trim_start_matches("M-").parse::<u64>().unwrap_or(0) > last_id) {
            let meta: Value = serde_json::from_str(answer.meta.as_deref().unwrap_or("{}")).unwrap_or_default();
            let left = meta.get("left").and_then(Value::as_i64).unwrap_or(0).max(0);
            let blocker = meta.get("blocker").and_then(Value::as_str).unwrap_or("none");
            let answered_active = active_at(tx, round, start, answer.created.min(now).max(start))?;
            let body = format!("{}: budget {} min; active {} min; left {} min; blocker: {}\n{}", round.task, budget, active / 60, left, blocker, answer.body);
            item(tx, round, &body, !blocker.trim().eq_ignore_ascii_case("none"), now)?;
            state["answer_id"] = json!(answer.id);
            state["next_active"] = json!((answered_active + left.max(10) * 60).max(budget * 60));
            state["waiting"] = json!(false); state["no_answer"] = json!(false);
            for question in tx.thread_messages(&round.project, &format!("overrun-question:{}:{}:{}", round.task, round.attempt, round.role))? {
                if matches!(question.state, MsgState::Open | MsgState::Waiting) {
                    tx.update_message_state(&round.project, &question.id, MsgState::Resolved, Some("progress received"), None, None, now)?;
                }
            }
            crate::stall::resolve(tx, &round.project, &round.seat, now, None)?;
            changed = true;
        }
    }
    if !live {
        for question in tx.thread_messages(&round.project, &format!("overrun-question:{}:{}:{}", round.task, round.attempt, round.role))? {
            if matches!(question.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(&round.project, &question.id, MsgState::Resolved, Some("round finished"), None, None, now)?;
            }
        }
        // The round is over because its attempt was settled: its open overrun
        // item is answered by that settlement and is not asked again (T-381).
        if settled {
            for old in tx.thread_messages(&round.project, &format!("overrun:{}:{}", round.task, round.role))? {
                if old.attempt == Some(round.attempt) && matches!(old.state, MsgState::Open | MsgState::Waiting) {
                    tx.update_message_state(&round.project, &old.id, MsgState::Resolved, Some("the attempt was settled"), None, None, now)?;
                    changed = true;
                }
            }
        }
        tx.set_meta(&state_key, &state.to_string())?;
        return Ok(changed)
    }
    let waiting = state["waiting"].as_bool().unwrap_or(false);
    let asked_active = state["asked_active"].as_i64().unwrap_or(0);
    if waiting && task.wait_on.is_none() && !state["no_answer"].as_bool().unwrap_or(false) && active >= asked_active + 600 {
        item(tx, round, &format!("{}: budget {} min; active {} min; no answer", round.task, budget, active / 60), true, now)?;
        state["no_answer"] = json!(true); state["waiting"] = json!(false); state["next_active"] = json!(active + 900);
        changed = true;
    }
    let next = state["next_active"].as_i64().unwrap_or(budget * 60);
    let paused_now = tx.pauses_of_project(&round.project)?.iter().any(|p| p.seat == round.seat && p.start_at <= now && p.end_at.is_none());
    if !state["waiting"].as_bool().unwrap_or(false) && active >= next && !paused_now && task.wait_on.is_none() && !matches!(seat.state, SeatState::Parked | SeatState::Held) {
        let question = text(&round.task, round.attempt, attempt.adopted_version, budget, active);
        for old in tx.thread_messages(&round.project, &format!("overrun-question:{}:{}:{}", round.task, round.attempt, round.role))? {
            if matches!(old.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(&round.project, &old.id, MsgState::Resolved, Some("a new question replaces it"), None, None, now)?;
            }
        }
        tx.insert_message(&NewMessage { project: round.project.clone(), thread: format!("overrun-question:{}:{}:{}", round.task, round.attempt, round.role), task: Some(round.task.clone()), attempt: Some(round.attempt), sender: "engine".into(), sender_gen: 0, recipient: round.seat.clone(), kind: MsgKind::Instruction, urgent: true, body: question, reply_to: None, needs_ack: false, state: MsgState::Open, created: now, meta: None })?;
        state["asked_at"] = json!(now); state["asked_active"] = json!(active); state["waiting"] = json!(true); state["no_answer"] = json!(false);
        changed = true;
    }
    tx.set_meta(&state_key, &state.to_string())?;
    Ok(changed)
}

/// Mirror quota parking into pause rows before calculating any round clock.
fn reconcile_quota_pauses(tx: &Tx<'_>, project: &str) -> GearResult<()> {
    let pauses = tx.pauses_of_project(project)?;
    for seat in tx.seats_of_project(project)? {
        let open = pauses.iter().any(|p| p.seat == seat.id && p.kind == "quota" && p.end_at.is_none());
        if seat.state == SeatState::Parked {
            if !open { tx.open_pause(project, &seat.id, "quota", "quota parked", seat.state_since)?; }
        } else if open {
            tx.close_pause(project, &seat.id, "quota", seat.state_since)?;
        }
    }
    Ok(())
}

pub fn tick(store: &mut Store, herdr: &mut dyn HerdrAct, _cfg: &EngineCfg, now: Ts) -> GearResult<()> {
    let projects = store.read(|tx| tx.project_ids())?;
    for project in projects {
        store.write(|tx| {
            reconcile_quota_pauses(tx, &project)?;
            let mut changed = false;
            for round in tx.rounds_of_project(&project)? { changed |= process(tx, &round, now)?; }
            Ok(changed)
        })?;
        let _ = herdr;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_model::{SeatRole, TaskState};
    use gear_store::{testing, ContractRow, NewRound, TaskRow, AttemptStep};
    const T: i64 = 1_800_000_000;
    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "codex", T)?;
            tx.insert_task(&TaskRow { repo:None, base:None, gate:None, rework_estimate_min:None, project:"p".into(), id:"T-1".into(), rank:1, state:TaskState::Ready, priority:"must".into(), lane:Some("heat".into()), milestone:None, required:false, contract_version:1, deps:vec![], followup_of:None, wait_on:None, wait_cond:None, rework_note:None, estimate_min:Some(10), issue:None, updated:T })?;
            tx.insert_contract(&ContractRow { project:"p".into(), task:"T-1".into(), version:1, goal:"g".into(), accept:"a".into(), evidence_files:vec![], evidence_due_min:None, body:String::new(), created:T, mechanical: None, })?;
            gear_app::tasks::start_attempt(tx,"p","T-1","p:heat",1,T)?;
            gear_app::tasks::on_delivery_confirmed(tx,"p","T-1",1,T)?;
            tx.insert_round(&NewRound { key: RoundKey { project:"p".into(), task:"T-1".into(), attempt:1, role:"dev".into() }, seat:"p:heat".into(), generation:1, cli:"codex".into(), model:None, start_at:T })?;
            Ok(())
        }).unwrap(); s
    }
    fn tick_at(s: &mut Store, at:i64) { tick(s, &mut FakeHerdr::new(), &EngineCfg::default(), T+at).unwrap(); }

    /// The world with attempt 1 handed in (Submitted) and its task Verifying:
    /// what a review round runs on.
    fn submitted() -> Store {
        let mut s = world();
        s.write(|tx| gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: ops/reports/T-1.md", T+1)).unwrap();
        s
    }
    fn messages(s:&mut Store, thread:&str) -> Vec<gear_store::MessageRow> { s.read(|tx| tx.thread_messages("p",thread)).unwrap() }
    fn answer(s:&mut Store, at:i64, left:i64, blocker:&str) {
        s.write(|tx| tx.insert_message(&NewMessage { project:"p".into(), thread:"T-1:1".into(), task:Some("T-1".into()), attempt:Some(1), sender:"p:heat".into(), sender_gen:1, recipient:"p:captain".into(), kind:MsgKind::Progress, urgent:false, body:"work update".into(), reply_to:None, needs_ack:false, state:MsgState::Info, created:T+at, meta:Some(json!({"left":left,"blocker":blocker}).to_string()) }).map(|_| ())).unwrap();
    }
    #[test]
    fn question_answer_and_urgency_are_scheduled_from_answer() {
        let mut s=world();
        tick_at(&mut s,599); assert!(messages(&mut s,"overrun-question:T-1:1:dev").is_empty());
        tick_at(&mut s,600); tick_at(&mut s,600);
        let q=messages(&mut s,"overrun-question:T-1:1:dev");
        assert_eq!(q.len(),1); assert!(q[0].urgent); assert_eq!(q[0].kind,MsgKind::Instruction); assert_eq!(q[0].body,text("T-1",1,1,10,600));
        answer(&mut s,660,15,"waiting for API"); tick_at(&mut s,660);
        let items=messages(&mut s,"overrun:T-1:dev"); assert_eq!(items.len(),1); assert!(items[0].urgent);
        answer(&mut s,700,15,"none"); tick_at(&mut s,700);
        let items=messages(&mut s,"overrun:T-1:dev"); assert_eq!(items.len(),1); assert!(!items[0].urgent);
        tick_at(&mut s,1599); assert_eq!(messages(&mut s,"overrun-question:T-1:1:dev").len(),1);
        tick_at(&mut s,1600); assert_eq!(messages(&mut s,"overrun-question:T-1:1:dev").len(),2);
    }
    #[test]
    fn missing_answer_escalates_after_active_ten_and_repeats_after_fifteen() {
        let mut s=world(); tick_at(&mut s,600);
        s.write(|tx| tx.open_pause("p","p:heat","wait","blocked",T+700)).unwrap();
        tick_at(&mut s,2000); assert!(messages(&mut s,"overrun:T-1:dev").is_empty());
        s.write(|tx| tx.close_pause("p","p:heat","wait",T+2000)).unwrap();
        tick_at(&mut s,2499); assert!(messages(&mut s,"overrun:T-1:dev").is_empty());
        tick_at(&mut s,2500); assert!(messages(&mut s,"overrun:T-1:dev")[0].urgent);
        tick_at(&mut s,3399); assert_eq!(messages(&mut s,"overrun-question:T-1:1:dev").len(),1);
        tick_at(&mut s,3400); assert_eq!(messages(&mut s,"overrun-question:T-1:1:dev").len(),2);
    }
    #[test]
    fn pauses_are_unioned_and_clock_stops_at_handin() {
        let mut s=world();
        s.write(|tx| { tx.open_pause("p","p:heat","wait","wait",T+100)?; tx.open_pause("p","p:heat","quota","quota",T+200)?; tx.close_pause("p","p:heat","wait",T+300)?; tx.close_pause("p","p:heat","quota",T+400)?; tx.set_attempt_time("p","T-1",1,AttemptStep::HandedIn,T+500)?; Ok(()) }).unwrap();
        tick_at(&mut s,2000);
        let round=s.read(|tx| tx.rounds_of_task("p","T-1")).unwrap().remove(0);
        assert_eq!(round.paused_s,300); assert_eq!(round.active_s,200); assert!(messages(&mut s,"overrun-question:T-1:1:dev").is_empty());
    }
    #[test]
    fn review_budget_uses_model_history_and_stops_at_verdict() {
        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w1:p3", "codex", T)?;
            for (n, seconds) in [(2, 1200), (3, 1800), (4, 2400)] {
                let key = RoundKey { project:"p".into(), task:"T-1".into(), attempt:n, role:"review".into() };
                tx.insert_round(&NewRound { key:key.clone(), seat:"p:review-1".into(), generation:1, cli:"codex".into(), model:Some("model-a".into()), start_at:T-5000 })?;
                tx.set_round_active(&key, seconds)?;
                tx.close_round(&key, T-1000+n)?;
            }
            tx.insert_round(&NewRound { key:RoundKey { project:"p".into(), task:"T-1".into(), attempt:1, role:"review".into() }, seat:"p:review-1".into(), generation:1, cli:"codex".into(), model:Some("model-a".into()), start_at:T+100 })?;
            Ok(())
        }).unwrap();
        tick_at(&mut s,2799);
        assert!(messages(&mut s,"overrun-question:T-1:1:review").is_empty());
        tick_at(&mut s,2800);
        assert_eq!(messages(&mut s,"overrun-question:T-1:1:review")[0].body,text("T-1",1,1,45,2700));
        s.write(|tx| tx.insert_review_verdict(&gear_store::ReviewVerdictRow { project:"p".into(), task:"T-1".into(), attempt:1, passed:true, findings:String::new(), by_seat:"p:review-1".into(), created:T+3000 })).unwrap();
        tick_at(&mut s,4000);
        let review=s.read(|tx| tx.rounds_of_task("p","T-1")).unwrap().into_iter().find(|r| r.attempt==1 && r.role=="review").unwrap();
        assert_eq!(review.active_s,2900);
    }

    #[test]
    fn actual_quota_park_pauses_the_clock_and_no_answer_timer() {
        let mut s = world();
        tick_at(&mut s, 600);
        s.write(|tx| tx.set_seat_state("p:heat", SeatState::Parked, Some("quota"), T+700)).unwrap();
        tick_at(&mut s, 2000);
        let round=s.read(|tx| tx.rounds_of_task("p", "T-1")).unwrap().remove(0);
        assert_eq!((round.active_s,round.paused_s),(700,1300));
        assert!(messages(&mut s,"overrun:T-1:dev").is_empty());
        s.write(|tx| tx.set_seat_state("p:heat", SeatState::Active, Some("quota reset"), T+2100)).unwrap();
        tick_at(&mut s, 2200);
        let round=s.read(|tx| tx.rounds_of_task("p", "T-1")).unwrap().remove(0);
        assert_eq!((round.active_s,round.paused_s),(800,1400));
        assert!(messages(&mut s,"overrun:T-1:dev").is_empty());
        tick_at(&mut s,2600);
        assert!(messages(&mut s,"overrun:T-1:dev")[0].urgent);
    }

    #[test]
    fn progress_before_handin_is_processed_on_the_finished_round() {
        let mut s=world(); tick_at(&mut s,600);
        answer(&mut s,601,0,"none");
        s.write(|tx| tx.set_attempt_time("p","T-1",1,AttemptStep::HandedIn,T+602)).unwrap();
        tick_at(&mut s,603); tick_at(&mut s,604);
        let items=messages(&mut s,"overrun:T-1:dev");
        assert_eq!(items.len(),1); assert!(!items[0].urgent); assert!(items[0].body.contains("work update"));
        assert_eq!(messages(&mut s,"overrun-question:T-1:1:dev").len(),1);
    }
    #[test]
    fn progress_after_handin_is_not_an_answer_for_the_closed_round() {
        let mut s=world(); tick_at(&mut s,600);
        s.write(|tx| tx.set_attempt_time("p","T-1",1,AttemptStep::HandedIn,T+601)).unwrap();
        answer(&mut s,602,0,"none"); tick_at(&mut s,603);
        assert!(messages(&mut s,"overrun:T-1:dev").is_empty());
    }
    #[test]
    fn progress_before_review_verdict_is_processed_on_the_finished_round() {
        let mut s=submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx,"p","review-1","T-1","w1:p3","codex",T)?;
            tx.insert_round(&NewRound { key:RoundKey {project:"p".into(),task:"T-1".into(),attempt:1,role:"review".into()},seat:"p:review-1".into(),generation:1,cli:"codex".into(),model:None,start_at:T })
        }).unwrap();
        tick_at(&mut s,1800);
        s.write(|tx| {
            tx.insert_message(&NewMessage {project:"p".into(),thread:"T-1:1".into(),task:Some("T-1".into()),attempt:Some(1),sender:"p:review-1".into(),sender_gen:1,recipient:"p:captain".into(),kind:MsgKind::Progress,urgent:false,body:"review update".into(),reply_to:None,needs_ack:false,state:MsgState::Info,created:T+1801,meta:Some(json!({"left":0,"blocker":"none"}).to_string())})?;
            tx.insert_review_verdict(&gear_store::ReviewVerdictRow {project:"p".into(),task:"T-1".into(),attempt:1,passed:true,findings:String::new(),by_seat:"p:review-1".into(),created:T+1802})
        }).unwrap();
        tick_at(&mut s,1803); tick_at(&mut s,1804);
        let items=messages(&mut s,"overrun:T-1:review");
        assert_eq!(items.len(),1); assert!(!items[0].urgent); assert!(items[0].body.contains("review update"));
        assert_eq!(messages(&mut s,"overrun-question:T-1:1:review").len(),1);
    }

    #[test]
    fn a_settled_attempt_ends_the_review_round_and_resolves_its_item_once() {
        let mut s=submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx,"p","review-1","T-1","w1:p3","codex",T)?;
            tx.insert_round(&NewRound { key:RoundKey {project:"p".into(),task:"T-1".into(),attempt:1,role:"review".into()},seat:"p:review-1".into(),generation:1,cli:"codex".into(),model:None,start_at:T })
        }).unwrap();
        // The round asks at its budget (30 min) and, unanswered, writes its item.
        tick_at(&mut s,1800);
        assert_eq!(messages(&mut s,"overrun-question:T-1:1:review").len(),1);
        tick_at(&mut s,2400);
        let items=messages(&mut s,"overrun:T-1:review");
        assert_eq!(items.len(),1); assert_eq!(items[0].state,MsgState::Open); assert!(items[0].urgent);
        // The captain's fail verdict settles the attempt: no review verdict, no end_at.
        s.write(|tx| gear_app::tasks::settle(tx, &gear_app::tasks::Verdict { project:"p".into(), task:"T-1".into(), attempt:1, contract_version:1, candidate:"abc1234".into(), pass:false, evidence:None, rework:Some("the rows drift right".into()), integrated_by:None, by_seat:"p:captain".into() }, T+2401)).unwrap();
        tick_at(&mut s,2500);
        let items=messages(&mut s,"overrun:T-1:review");
        assert_eq!(items.len(),1);
        assert_eq!(items[0].state,MsgState::Resolved);
        assert_eq!(items[0].result.as_deref(),Some("the attempt was settled"));
        assert_eq!(items[0].resolved,Some(T+2500));
        let round=s.read(|tx| tx.rounds_of_task("p","T-1")).unwrap().into_iter().find(|r| r.attempt==1 && r.role=="review").unwrap();
        assert_eq!(round.active_s,2401);
        // Later ticks ask nothing and write nothing: the item stays resolved once.
        tick_at(&mut s,6000); tick_at(&mut s,9000);
        assert_eq!(messages(&mut s,"overrun-question:T-1:1:review").len(),1);
        let items=messages(&mut s,"overrun:T-1:review");
        assert_eq!(items.len(),1);
        assert_eq!(items[0].state,MsgState::Resolved);
        assert_eq!(items[0].resolved,Some(T+2500));
    }

    #[test]
    fn a_queued_answer_after_settlement_writes_no_overrun_item() {
        let mut s = submitted();
        s.write(|tx| {
            testing::seed_temp_seat(tx,"p","review-1","T-1","w1:p3","codex",T)?;
            tx.insert_round(&NewRound { key:RoundKey {project:"p".into(),task:"T-1".into(),attempt:1,role:"review".into()},seat:"p:review-1".into(),generation:1,cli:"codex".into(),model:None,start_at:T })
        }).unwrap();
        // The round asks at its budget (30 min).
        tick_at(&mut s,1800);
        assert_eq!(messages(&mut s,"overrun-question:T-1:1:review").len(),1);
        // The reviewer's answer is queued at T+1801 and the captain settles the
        // attempt at T+1802, before the next tick can process the answer.
        s.write(|tx| {
            tx.insert_message(&NewMessage {project:"p".into(),thread:"T-1:1".into(),task:Some("T-1".into()),attempt:Some(1),sender:"p:review-1".into(),sender_gen:1,recipient:"p:captain".into(),kind:MsgKind::Progress,urgent:false,body:"review update".into(),reply_to:None,needs_ack:false,state:MsgState::Info,created:T+1801,meta:Some(json!({"left":0,"blocker":"none"}).to_string())})?;
            gear_app::tasks::settle(tx, &gear_app::tasks::Verdict { project:"p".into(), task:"T-1".into(), attempt:1, contract_version:1, candidate:"abc1234".into(), pass:false, evidence:None, rework:Some("the rows drift right".into()), integrated_by:None, by_seat:"p:captain".into() }, T+1802)
        }).unwrap();
        // The settlement already stopped the round: the queued answer writes no
        // item on this tick or any later one (F1 of the T-381 review).
        for at in [1803, 1804, 6000, 9000] {
            tick_at(&mut s, at);
            assert!(messages(&mut s,"overrun:T-1:review").is_empty(), "no item after the settlement, tick {at}");
        }
        // The round's question is closed by the settlement, as before.
        let questions=messages(&mut s,"overrun-question:T-1:1:review");
        assert_eq!(questions.len(),1);
        assert_eq!(questions[0].state,MsgState::Resolved);
    }

}
