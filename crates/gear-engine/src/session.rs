//! The session of a round: the conversation the round runs in (design §3.2-§3.4).
//!
//! A round is the work of one attempt on one seat. Before its packet goes
//! out, the engine picks the **session** the round runs in: the conversation
//! it continues, the one it starts, or the one it resumes (design §3.3).
//!
//! This module is part 1 of two:
//!
//! - the rules of §3.3 as a pure function of the facts ([`choose`]): the
//!   same facts always give the same action;
//! - the round row of §4.1 ([`prepare_lane`], [`prepare_review`]): it is
//!   created once and kept over the ticks, and the action it chose is
//!   recorded;
//! - the two actions that need nothing from Herdr — `keep` and `fresh` — are
//!   run here. The other three (`new`, `compact`, `resume`) are chosen here
//!   and run in part 2: until then they wait, one tick at a time.

use gear_herdr::HerdrAct;
use gear_model::{AttemptState, GearResult, SeatState, Ts};
use gear_store::{NewRound, RoundKey, RoundRow, SeatRow, Store};

use crate::EngineCfg;

/// What the session step of a round did (design §13.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prep {
    /// The round's session is the one it needs: the packet can go.
    Ready,
    /// Not yet: the step runs one step per tick; try again next tick.
    Wait,
}

/// The kind of round — the role of its row (design §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// A lane's first attempt at a task.
    #[default]
    Dev,
    /// A lane's rework (attempt n ≥ 2).
    Rework,
    /// A reviewer's round.
    Review,
    /// An early direction check on a lane's attempt.
    Direction,
}

impl Role {
    /// The name of the role in the store (`dev`, `rework`, `review`).
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Dev => "dev",
            Role::Rework => "rework",
            Role::Review => "review",
            Role::Direction => "direction",
        }
    }
}

/// What a round does with its session (design §3.3-§3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing: the round goes on in the session the seat has.
    Keep,
    /// The agent was just started: its session is new already.
    Fresh,
    /// A new conversation (`new_session`), then the packet.
    New,
    /// A compaction, then the packet.
    Compact,
    /// Start the agent again on that session.
    Resume(String),
}

impl Action {
    /// The name the attempt records (design §13.1: `attempts.session_action`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Keep => "keep",
            Action::Fresh => "fresh",
            Action::New => "new",
            Action::Compact => "compact",
            Action::Resume(_) => "resume",
        }
    }

    /// Whether the action is done: `keep` and `fresh` need nothing from
    /// Herdr — the packet may go.
    pub fn is_ready(&self) -> bool {
        matches!(self, Action::Keep | Action::Fresh)
    }
}

/// The facts the session rules read (design §3.3).
///
/// The rules are a pure function of them: the same facts always give the
/// same action. Every field is a fact of the store or of Herdr — never a
/// guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The seat the round runs on.
    pub seat: String,
    /// The state of that seat: a reviewer that waits is `Standby`, a free one
    /// is `Released` (V1).
    pub seat_state: SeatState,
    /// The kind of round: a lane round or a review round.
    pub role: Role,
    /// The attempt of the round: n ≥ 2 is a rework (L2-L4).
    pub attempt: i64,
    /// The task the round follows (`followup-of: F`), if any (L5-L6).
    pub followup_of: Option<String>,
    /// The seat that ran the earlier work: the rework's attempt n−1, the
    /// follow-up's F, or the seat of the earlier review round of this task.
    pub earlier_seat: Option<String>,
    /// The CLI that earlier work ran on.
    pub earlier_cli: Option<String>,
    /// The session of that earlier work.
    pub earlier_session: Option<String>,
    /// The session the seat runs on now.
    pub current_session: Option<String>,
    /// The CLI the seat runs on now.
    pub current_cli: Option<String>,
    /// Whether the seat has a live agent (L1: a seat without one has its
    /// agent started plainly, and the action becomes `fresh`).
    pub agent_live: bool,
    /// The session of the round that is already open on this seat (S1: the
    /// agent died or was restarted while its round was open).
    pub open_round_session: Option<String>,
    /// The sessions other panes' live agents run (S3): a conversation one of
    /// them holds is never resumed here.
    pub sessions_elsewhere: Vec<String>,
}

impl Facts {
    /// Whether the earlier work ran on this seat with the same CLI (L2, L3,
    /// L5, L6, V1).
    fn ran_here_with_the_same_cli(&self) -> bool {
        self.earlier_seat.as_deref() == Some(self.seat.as_str())
            && self.earlier_cli.is_some()
            && self.earlier_cli == self.current_cli
    }

    /// Whether the session of the earlier work is the session the seat runs
    /// on now (L2, L5, V1).
    fn same_session(&self) -> bool {
        self.earlier_session.is_some() && self.earlier_session == self.current_session
    }
}

/// The action the round takes on its session (design §3.3): a pure function
/// of the facts.
///
/// The rules are read top to bottom, and the first match wins:
///
/// - **S1** — the round is open and its agent died or was restarted: the
///   round resumes its session, in the same round;
/// - **L2-L7** — a lane round: a rework (n ≥ 2), else a follow-up, else a
///   new task;
/// - **V1-V2** — a review round: the earlier review of this task on this
///   seat, else a new session.
///
/// **S3** sits on top of all of them: whichever rule picked a `resume` of a
/// session another pane's live agent runs, the round starts a new
/// conversation instead.
///
/// **L1** is not a rule of its own: it says what happens when the seat has
/// no live agent — the agent is started plainly (with the resume args when
/// the rule says `resume`) and the action becomes `fresh`.
pub fn choose(facts: &Facts) -> Action {
    // S1: the round is open and the agent died or was restarted.
    let action = match s1(facts) {
        Some(action) => action,
        None => match facts.role {
            Role::Review | Role::Direction => review_action(facts),
            Role::Dev | Role::Rework => lane_action(facts),
        },
    };
    // S3: a conversation another pane's live agent runs is not resumed — that
    // agent answers "This conversation is open in another app" and the seat
    // sits locked. The seat starts a new conversation instead; nothing else is
    // touched, no pane is closed.
    let action = without_a_held_resume(facts, action);
    // L1: a seat without a live agent has its agent started plainly, and the
    // session it starts with is a fresh one.
    if !facts.agent_live && !matches!(action, Action::Resume(_)) {
        return Action::Fresh;
    }
    action
}

/// S3 (design §3.3, T-423): a conversation another pane's live agent holds is
/// never resumed. Every path that picks an action passes through here — the
/// rules of `choose`, and the S1 recovery of an open round that calls `s1`
/// directly (`prepare_lane`, `prepare_review`).
fn without_a_held_resume(facts: &Facts, action: Action) -> Action {
    match action {
        Action::Resume(session) if facts.sessions_elsewhere.contains(&session) => Action::Fresh,
        action => action,
    }
}

/// S1 (design §3.3): the agent died or was restarted while its round was
/// open → the round resumes its session, in the same round (no new round
/// row).
fn s1(facts: &Facts) -> Option<Action> {
    let session = facts.open_round_session.as_deref()?;
    // The agent is alive and runs on the session of the round: the round goes
    // on as it is.
    if facts.agent_live && facts.current_session.as_deref() == Some(session) {
        return None;
    }
    Some(Action::Resume(session.to_string()))
}

/// The lane rules L2-L7 (design §3.3).
fn lane_action(facts: &Facts) -> Action {
    // L2-L4: a rework (n ≥ 2) continues the session of attempt n−1.
    if facts.attempt >= 2 {
        return if facts.ran_here_with_the_same_cli() {
            if facts.same_session() {
                Action::Keep
            } else {
                resume(facts)
            }
        } else {
            Action::New
        };
    }
    // L5-L6: a follow-up continues the session of the task it follows.
    if facts.followup_of.is_some() && facts.ran_here_with_the_same_cli() {
        return if facts.same_session() {
            Action::Keep
        } else {
            resume(facts)
        };
    }
    // L7: an unrelated task starts a new conversation.
    Action::New
}

/// The review rules V1-V2 (design §3.3).
fn review_action(facts: &Facts) -> Action {
    // V1: an earlier review round of this task ran on this seat with the same
    // CLI, and the seat waits (`Standby`) or is free (`Released`).
    let v1 = matches!(facts.seat_state, SeatState::Standby | SeatState::Released)
        && facts.ran_here_with_the_same_cli();
    if v1 {
        return match facts.earlier_session.as_deref() {
            Some(session)
                if facts.agent_live && facts.current_session.as_deref() == Some(session) =>
            {
                Action::Keep
            }
            Some(session) => Action::Resume(session.to_string()),
            // The earlier round has no session to continue.
            None => Action::New,
        };
    }
    // V2: any other case: a new conversation.
    Action::New
}

/// The action of a rule that says `resume`: the agent starts again on the
/// session of the earlier work. Without a recorded session, start a new one;
/// L1 converts this to fresh when the agent is absent.
fn resume(facts: &Facts) -> Action {
    match facts.earlier_session.as_deref() {
        Some(session) => Action::Resume(session.to_string()),
        None => Action::New,
    }
}

/// The session restored by S1 remains authoritative: the round's session
/// history can also contain the transient session observed before recovery.
pub(crate) fn round_session(
    tx: &gear_store::Tx<'_>,
    round: &gear_store::RoundRow,
) -> GearResult<Option<String>> {
    let identity = format!(
        "{}:{}:{}:{}",
        round.project, round.task, round.attempt, round.role
    );
    let generation = tx.get_seat(&round.seat)?.map(|seat| seat.generation);
    let cross_cli = tx
        .get_meta(&format!("engine.session_cross_cli.{}", round.seat))?
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .is_some_and(|s| s["round"] == identity && s["generation"].as_i64() == generation);
    if cross_cli {
        return match generation {
            Some(generation) => Ok(tx
                .get_binding(&round.seat, generation)?
                .and_then(|b| b.session_ref)),
            None => Ok(None),
        };
    }
    let resumed = tx
        .get_meta(&format!("engine.session_step.{}", round.seat))?
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(|s| {
            s["round"] == identity
                && s["generation"].as_i64() == generation
                && s["action"] == "resume"
                && s["step"] == "done"
                && s["result"] == "resume"
        })
        .and_then(|s| s["target"].as_str().map(str::to_string));
    Ok(resumed.or_else(|| round.sessions.last().cloned()))
}

/// The session step of a **lane** round (design §3.2 steps 2-3, §13.4).
///
/// The round row is created on the first call and kept over the ticks
/// (§4.1: a seat never has two open rounds), the action of §3.3 is chosen
/// and recorded on the attempt (`attempts.session_action`), and the action
/// that needs nothing from Herdr is run: `keep` and `fresh` are done, so the
/// packet may go ([`Prep::Ready`]). The other three wait for part 2.
pub fn prepare_lane(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    now: Ts,
) -> GearResult<Prep> {
    let project = seat.project.clone();
    let role = if attempt >= 2 {
        Role::Rework
    } else {
        Role::Dev
    };
    let key = round_key(&project, task, attempt, role);

    let mut facts = lane_facts(store, herdr, &project, seat, task, attempt)?;
    let round = round_of(store, &key)?;
    let fresh_key = format!("engine.session_fresh.{}", seat.id);
    let fresh_generation =
        store.read(|tx| tx.get_meta(&fresh_key))?.as_deref() == Some(&seat.generation.to_string());
    let fresh = round.is_none() && fresh_generation;
    if fresh {
        // Onboarding has already started this agent, but its first unrelated
        // round is fresh; a related recorded session still resumes.
        facts.agent_live = false;
        facts.current_session = None;
    }
    facts.open_round_session = match &round {
        Some(round) => store.read(|tx| round_session(tx, round))?,
        None => None,
    };

    let rebind_fresh = round.is_none()
        && store
            .read(|tx| tx.get_meta(&crate::rebind::fresh_key(&seat.id)))?
            .as_deref()
            == Some(&seat.generation.to_string());
    let mut action = match &round {
        // A new round: the rules of §3.3 decide.
        None => choose(&facts),
        // The round is open: S1 decides, else the round goes on with the
        // action it has. S3 holds for S1 too (T-423, F1 of the review).
        Some(_) => without_a_held_resume(
            &facts,
            match s1(&facts) {
                Some(action) => action,
                None => recorded_action(store, &project, task, attempt)?.unwrap_or(Action::Keep),
            },
        ),
    };
    if rebind_fresh {
        action = Action::Fresh;
    }
    if round.is_none()
        && facts.followup_of.is_some()
        && attempt == 1
        && matches!(action, Action::Keep)
        && context_large(store, cfg, seat, facts.current_session.as_deref())?
    {
        action = Action::Compact;
    }

    store.write(|tx| {
        if round.is_none() {
            create_round(tx, &key, seat, &facts, now)?;
        }
        // Resuming an existing round also uses this generation's fresh
        // process; it must not be reused for a later unrelated round.
        if fresh_generation {
            tx.set_meta(&fresh_key, "")?;
        }
        if rebind_fresh {
            tx.set_meta(&crate::rebind::fresh_key(&seat.id), "")?;
        }
        if round.is_none() {
            tx.set_attempt_action(&project, task, attempt, action.as_str())?;
        }
        Ok(())
    })?;

    let compact_after = facts.followup_of.is_some()
        && attempt == 1
        && matches!(action, Action::Resume(_))
        && context_large(store, cfg, seat, facts.earlier_session.as_deref())?;
    let compact_key = crate::quiet::compact_key(&seat.id);
    let pending = store
        .read(|tx| tx.get_meta(&compact_key))?
        .unwrap_or_default();
    if pending != "running" {
        let prep = crate::session_runner::run(
            store,
            herdr,
            cfg,
            seat,
            task,
            attempt,
            role,
            &action,
            compact_after,
            now,
        )?;
        if prep != Prep::Ready || pending != "pending" {
            return Ok(prep);
        }
        // The previous round action is complete. Start a separate durable
        // compaction before allowing this delivery, including across restarts.
        store.write(|tx| {
            tx.set_meta(&format!("engine.session_step.{}", seat.id), "")?;
            tx.set_meta(&compact_key, "running")
        })?;
    }
    let prep = crate::session_runner::run(
        store,
        herdr,
        cfg,
        seat,
        task,
        attempt,
        role,
        &Action::Compact,
        false,
        now,
    )?;
    if prep == Prep::Ready {
        store.write(|tx| tx.set_meta(&compact_key, ""))?;
    }
    Ok(prep)
}

/// The session step of a **review** round (design §3.2 steps 2-3, §13.4):
/// the same as [`prepare_lane`], with the rules of V1-V2 and the round row
/// of the review.
///
/// The reviewer has no attempt of its own: its action is recorded in the
/// meta store, under a key of its own (the attempt row of a task belongs to
/// the lane that wrote it).
pub fn prepare_review(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    now: Ts,
) -> GearResult<Prep> {
    prepare_review_role(store, herdr, cfg, seat, task, attempt, Role::Review, now)
}

pub(crate) fn prepare_review_role(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &EngineCfg,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
    role: Role,
    now: Ts,
) -> GearResult<Prep> {
    let project = seat.project.clone();
    let key = round_key(&project, task, attempt, role);

    let mut facts = review_facts(store, herdr, &project, seat, task, attempt)?;
    facts.role = role;
    let round = round_of(store, &key)?;
    facts.open_round_session = round
        .as_ref()
        .and_then(|round| round.sessions.last().cloned());

    let action = match &round {
        None => choose(&facts),
        // The round is open: S1 decides, else the recorded action. S3 holds for
        // S1 too (T-423, F1 of the review).
        Some(_) => without_a_held_resume(
            &facts,
            match s1(&facts) {
                Some(action) => action,
                None => {
                    recorded_review_action(store, &project, task, attempt)?.unwrap_or(Action::Keep)
                }
            },
        ),
    };

    store.write(|tx| {
        if round.is_none() {
            create_round(tx, &key, seat, &facts, now)?;
            tx.set_meta(
                &review_action_key(&project, task, attempt),
                &review_action_json(&action),
            )?;
        }
        Ok(())
    })?;

    crate::session_runner::run(
        store, herdr, cfg, seat, task, attempt, role, &action, false, now,
    )
}

fn context_large(
    store: &mut Store,
    cfg: &EngineCfg,
    seat: &SeatRow,
    session: Option<&str>,
) -> GearResult<bool> {
    let (project, model, binding) = store.read(|tx| {
        Ok((
            tx.get_project(&seat.project)?,
            tx.get_meta(&crate::model::model_meta_key(&seat.id))?,
            tx.get_binding(&seat.id, seat.generation)?,
        ))
    })?;
    let Some(config) = project.and_then(|p| gear_app::config::parse(&p.config_toml).ok()) else {
        return Ok(true);
    };
    let entries = match seat.role {
        gear_model::SeatRole::Lane => config
            .lanes
            .iter()
            .find(|lane| lane.name == seat.name)
            .map(|lane| gear_app::config::lane_chain(&config, lane))
            .unwrap_or(&config.models.lanes),
        gear_model::SeatRole::Temp => config.models.review.as_slice(),
        gear_model::SeatRole::Captain => config.models.captain.as_slice(),
    };
    let entry = match model.as_deref() {
        Some(model) => entries.iter().find(|entry| entry.model() == model),
        None => entries.first(),
    };
    let Some(window) = entry.and_then(|e| e.context_window_k()).filter(|w| *w > 0) else {
        return Ok(true);
    };
    let profiles = gear_app::config::profile_table(&cfg.profiles, &config)?;
    let usage = binding
        .as_ref()
        .and_then(|binding| {
            profiles
                .get(&seat.profile)
                .or_else(|| profiles.values().find(|p| p.kind == binding.kind))
        })
        .and_then(|p| p.usage_log.as_deref())
        .and_then(gear_usage::UsageLog::from_name)
        .and_then(|log| session.and_then(|session| gear_usage::last_context(log, session)));
    Ok(usage.is_none_or(|tokens| (tokens as u128) * 2 >= (window as u128) * 1000))
}

/// The facts of a lane round (design §3.3).
fn lane_facts(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
) -> GearResult<Facts> {
    // The earlier work: the attempt before this one (a rework), or the last
    // accepted attempt of the task it follows (a follow-up).
    let (followup_of, earlier_seat, earlier_cli, earlier_session) = store.read(|tx| {
        let followup_of = tx
            .get_task(project, task)?
            .and_then(|row| row.followup_of.clone());
        let earlier = if attempt >= 2 {
            tx.get_attempt(project, task, attempt - 1)?
        } else if let Some(followup) = followup_of.as_deref() {
            tx.attempts_of_task(project, followup)?
                .into_iter()
                .rfind(|row| row.state == AttemptState::Accepted)
        } else {
            None
        };
        Ok((
            followup_of,
            earlier.as_ref().map(|row| row.seat.clone()),
            earlier.as_ref().and_then(|row| row.session_cli.clone()),
            earlier.as_ref().and_then(|row| row.session_ref.clone()),
        ))
    })?;

    let (current_session, current_cli, agent_live) = seat_session(store, herdr, seat)?;
    Ok(Facts {
        seat: seat.id.clone(),
        seat_state: seat.state,
        role: if attempt >= 2 {
            Role::Rework
        } else {
            Role::Dev
        },
        attempt,
        followup_of,
        earlier_seat,
        earlier_cli,
        earlier_session,
        current_session,
        current_cli,
        agent_live,
        open_round_session: None,
        sessions_elsewhere: sessions_elsewhere(store, herdr, seat),
    })
}

/// The facts of a review round (design §3.3, V1-V2).
fn review_facts(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &SeatRow,
    task: &str,
    attempt: i64,
) -> GearResult<Facts> {
    // The earlier review round of this task on this seat (V1).
    let (earlier_seat, earlier_cli, earlier_session) = store.read(|tx| {
        let round = tx
            .rounds_of_task(project, task)?
            .into_iter()
            .rev()
            .find(|round| round.role == Role::Review.as_str() && round.seat == seat.id);
        Ok(match round {
            Some(round) => (
                Some(seat.id.clone()),
                Some(round.cli.clone()),
                round.sessions.last().cloned(),
            ),
            None => (None, None, None),
        })
    })?;

    let (current_session, current_cli, agent_live) = seat_session(store, herdr, seat)?;
    Ok(Facts {
        seat: seat.id.clone(),
        seat_state: seat.state,
        role: Role::Review,
        attempt,
        followup_of: None,
        earlier_seat,
        earlier_cli,
        earlier_session,
        current_session,
        current_cli,
        agent_live,
        open_round_session: None,
        sessions_elsewhere: sessions_elsewhere(store, herdr, seat),
    })
}

/// The sessions the live agents of the **other** panes run (S3).
///
/// A conversation another pane's live agent holds is refused on resume
/// ("This conversation is open in another app") and the seat sits locked, so
/// the seat starts a new conversation instead. A read that fails is not
/// evidence of another pane: the facts then carry no such session, and the
/// rules resume as they did before.
fn sessions_elsewhere(store: &mut Store, herdr: &mut dyn HerdrAct, seat: &SeatRow) -> Vec<String> {
    let own = store
        .read(|tx| tx.get_binding(&seat.id, seat.generation))
        .ok()
        .flatten()
        .map(|binding| binding.pane_id);
    let Ok(agents) = herdr.agents() else {
        return Vec::new();
    };
    agents
        .iter()
        .filter(|agent| Some(agent.pane_id.as_str()) != own.as_deref())
        .filter_map(|agent| agent.agent_session.as_ref())
        .map(|session| session.value.clone())
        .filter(|session| !session.is_empty())
        .collect()
}

/// The session the seat runs on now, the CLI it runs on, and whether its
/// agent is alive.
///
/// A pane that cannot be read is not an absence: the agent counts as alive
/// (design/08).
fn seat_session(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &SeatRow,
) -> GearResult<(Option<String>, Option<String>, bool)> {
    let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
    let Some(binding) = binding else {
        return Ok((None, None, false));
    };
    let session = binding
        .session_ref
        .clone()
        .filter(|session| !session.is_empty());
    let cli = if binding.kind.is_empty() {
        None
    } else {
        Some(binding.kind.clone())
    };
    let alive = match herdr.agent_get(&binding.pane_id) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => true,
    };
    Ok((session, cli, alive))
}

/// The key of a round (the store's identity of one round).
fn round_key(project: &str, task: &str, attempt: i64, role: Role) -> RoundKey {
    RoundKey {
        project: project.to_string(),
        task: task.to_string(),
        attempt,
        role: role.as_str().to_string(),
    }
}

/// The round of that key, if it exists.
fn round_of(store: &mut Store, key: &RoundKey) -> GearResult<Option<RoundRow>> {
    store.read(|tx| {
        Ok(tx
            .rounds_of_task(&key.project, &key.task)?
            .into_iter()
            .find(|round| round.attempt == key.attempt && round.role == key.role))
    })
}

/// Create the round row of a round (design §4.1, §13.4).
///
/// The seat's own open round ends when this one starts: a seat never has
/// two open rounds.
fn create_round(
    tx: &gear_store::Tx<'_>,
    key: &RoundKey,
    seat: &SeatRow,
    facts: &Facts,
    now: Ts,
) -> GearResult<()> {
    if let Some(open) = tx.open_round_of_seat(&seat.id)? {
        let same = open.attempt == key.attempt && open.role == key.role && open.task == key.task;
        if !same {
            tx.close_round(
                &RoundKey {
                    project: open.project.clone(),
                    task: open.task.clone(),
                    attempt: open.attempt,
                    role: open.role.clone(),
                },
                now,
            )?;
        }
    }
    tx.insert_round(&NewRound {
        key: key.clone(),
        seat: seat.id.clone(),
        generation: seat.generation,
        cli: facts.current_cli.clone().unwrap_or_default(),
        model: tx.get_meta(&crate::model::model_meta_key(&seat.id))?,
        start_at: now,
    })
}

/// The action the attempt recorded (`attempts.session_action`).
fn recorded_action(
    store: &mut Store,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<Option<Action>> {
    let action = store.read(|tx| {
        Ok(tx
            .get_attempt(project, task, attempt)?
            .and_then(|row| row.session_action))
    })?;
    Ok(action.as_deref().and_then(parse_action))
}

/// The action a review round recorded (the meta store: a reviewer has no
/// attempt of its own).
fn recorded_review_action(
    store: &mut Store,
    project: &str,
    task: &str,
    attempt: i64,
) -> GearResult<Option<Action>> {
    let action = store.read(|tx| tx.get_meta(&review_action_key(project, task, attempt)))?;
    Ok(action.as_deref().and_then(|value| {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
            let name = json.get("action")?.as_str()?;
            if name == "resume" {
                return Some(Action::Resume(
                    json.get("session_ref")?.as_str()?.to_string(),
                ));
            }
            parse_action(name)
        } else {
            parse_action(value)
        }
    }))
}

fn review_action_json(action: &Action) -> String {
    let session = match action {
        Action::Resume(session) => Some(session.as_str()),
        _ => None,
    };
    serde_json::json!({ "action": action.as_str(), "session_ref": session }).to_string()
}

/// The key of a review round's action.
fn review_action_key(project: &str, task: &str, attempt: i64) -> String {
    format!("engine.review_action.{project}.{task}.{attempt}")
}

/// The action of a name (`keep`, `fresh`, `new`, `compact`, `resume`).
fn parse_action(name: &str) -> Option<Action> {
    match name {
        "keep" => Some(Action::Keep),
        "fresh" => Some(Action::Fresh),
        "new" => Some(Action::New),
        "compact" => Some(Action::Compact),
        "resume" => Some(Action::Resume(String::new())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_store::testing;

    const T0: i64 = 1_800_000_000;

    #[test]
    fn loop_guard_compacts_before_the_next_delivery_is_ready() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        herdr.set_agent_session("w1:p1", "round-session");
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        store
            .write(|tx| {
                tx.set_binding_session_ref(&seat.id, 1, "round-session")?;
                let key = gear_store::RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: seat.id.clone(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T0,
                })?;
                tx.add_round_session(&key, "round-session")?;
                tx.set_attempt_action("p", "T-1", 1, "keep")?;
                tx.set_meta(&crate::quiet::compact_key(&seat.id), "pending")?;
                tx.insert_op(&gear_store::NewOp {
                    project: "p".into(),
                    seat: seat.id.clone(),
                    generation: 1,
                    kind: gear_model::OpKind::Deliver,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({"text":"packet"}),
                    created: T0,
                })?;
                Ok(())
            })
            .unwrap();
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            prepare_lane(store, herdr, &cfg(), &seat, "T-1", 1, now).unwrap()
        };
        assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
        assert!(herdr.calls.iter().any(
            |c| matches!(c, gear_herdr::fake::Call::PaneType {text,..} if text == "/compact")
        ));
        assert!(
            crate::ops::execute_intended(&mut store, &mut herdr, "p", T0 + 1)
                .unwrap()
                .is_empty(),
            "the already-intended packet must wait"
        );
        assert_eq!(
            store
                .read(|tx| tx.ops_in_state("p", gear_model::OpState::Intended))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(run(&mut store, &mut herdr, T0 + 2), Prep::Wait);
        assert!(herdr.calls.iter().any(
            |c| matches!(c, gear_herdr::fake::Call::SendKeys {keys,..} if keys == &["enter"])
        ));
        herdr.set_status("w1:p1", gear_herdr::AgentStatus::Done);
        assert_eq!(run(&mut store, &mut herdr, T0 + 3), Prep::Ready);
        assert_eq!(
            store
                .read(|tx| tx.get_meta(&crate::quiet::compact_key(&seat.id)))
                .unwrap()
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn completed_step_resumes_the_round_after_an_idle_replacement() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0).unwrap(),
            Prep::Wait
        );
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 1).unwrap(),
            Prep::Wait
        );
        store
            .write(|tx| {
                tx.set_binding_session_ref(&seat.id, 1, "round-session")?;
                tx.add_round_session(&key, "round-session")
            })
            .unwrap();
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 6).unwrap(),
            Prep::Ready
        );
        store
            .write(|tx| tx.set_binding_session_ref(&seat.id, 1, "replacement"))
            .unwrap();
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 10).unwrap(),
            Prep::Wait
        );
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 11).unwrap(),
            Prep::Wait
        );
        herdr.exit_agent("w1:p1");
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 12).unwrap(),
            Prep::Wait
        );
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 13).unwrap(),
            Prep::Wait
        );
        assert!(herdr.calls.iter().any(|call| matches!(call, gear_herdr::fake::Call::AgentStart { args, .. } if args.contains(&"round-session".to_string()))));
        store
            .write(|tx| tx.set_binding_session_ref(&seat.id, 1, "round-session"))
            .unwrap();
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 14).unwrap(),
            Prep::Ready
        );
        let calls = herdr.calls.len();
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 15).unwrap(),
            Prep::Ready
        );
        assert_eq!(herdr.calls.len(), calls);
    }

    /// The persisted session step of a seat; the T-434 tests read the step
    /// the runner wrote.
    fn session_state(store: &mut Store, seat: &str) -> serde_json::Value {
        let key = format!("engine.session_step.{seat}");
        let raw = store.read(|tx| tx.get_meta(&key)).unwrap().unwrap();
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()
    }

    #[test]
    fn new_session_is_typed_and_entered_in_one_step() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let action = Action::New;
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &cfg(),
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
        // T-434: the text and its Enter leave in the same call, so the `enter`
        // step is never visited and nothing stays typed in the box.
        assert_eq!(session_state(&mut store, "p:heat")["step"], "wait_action");
        let typed = herdr.calls.iter().position(|call| matches!(call, gear_herdr::fake::Call::PaneType { text, .. } if text == "/new")).expect("the new session is typed");
        let entered = herdr.calls.iter().position(|call| matches!(call, gear_herdr::fake::Call::SendKeys { keys, .. } if keys == &["enter"])).expect("the Enter is sent at once");
        assert!(typed < entered);
        // The agent looks busy right after the text: the Enter already went.
        herdr.set_status("w1:p1", gear_herdr::AgentStatus::Working);
        assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
        assert_eq!(herdr.calls.iter().filter(|call| matches!(call, gear_herdr::fake::Call::PaneType { text, .. } if text == "/new")).count(), 1, "the command is not typed twice");
        herdr.set_status("w1:p1", gear_herdr::AgentStatus::Idle);
        assert_eq!(run(&mut store, &mut herdr, T0 + 4), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 5), Prep::Ready);
        let attempt = store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap();
        assert_eq!(attempt.session_action.as_deref(), Some("new-unconfirmed"));
    }

    #[test]
    fn a_codex_new_conversation_answers_the_worktree_dialog_first() {
        // T-457: Codex 0.165's `/new` opens "Where should the new
        // conversation run?" and Herdr still reports the agent idle. The
        // answer (Enter, the preselected current worktree) goes before the
        // step records the new session, so no delivery lands in the dialog.
        const CODEX_NEW_SCREEN: &str = "\
  Where should the new conversation run?
  1. Use current Git worktree
     Keep using the current working directory
  2. Create new Git worktree
     Create a separate checkout of this repository in another directory
";
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let action = Action::New;
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &cfg(),
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
        assert_eq!(session_state(&mut store, "p:heat")["step"], "wait_action");
        let enters = |herdr: &FakeHerdr| {
            herdr
                .calls
                .iter()
                .filter(|call| {
                    matches!(call, gear_herdr::fake::Call::SendKeys { keys, .. } if keys == &["enter"])
                })
                .count()
        };
        let submitted = enters(&herdr);
        // The dialog opens with the agent still idle: the answer goes, and
        // the step records nothing yet.
        herdr.set_screen("w1:p1", CODEX_NEW_SCREEN);
        assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
        assert_eq!(enters(&herdr), submitted + 1, "the dialog is answered");
        assert_eq!(session_state(&mut store, "p:heat")["step"], "wait_action");
        // The dialog is gone and the conversation is idle: the new session is
        // recorded on the usual codex timing.
        herdr.set_screen(
            "w1:p1",
            "  > \n  gpt-6.1-sol high · Context 0% used · weekly 94% left",
        );
        assert_eq!(run(&mut store, &mut herdr, T0 + 2), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 6), Prep::Ready);
        let attempt = store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap();
        assert_eq!(attempt.session_action.as_deref(), Some("new-unconfirmed"));
    }

    #[test]
    fn a_refused_enter_stays_at_enter_and_goes_on_a_later_call_while_working() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        store
            .write(|tx| {
                tx.set_meta("engine.session_step.p:heat", &serde_json::json!({
            "round":"p:T-1:1:dev", "generation":1, "action":"new", "step":"enter", "started":T0,
            "t0":T0, "step_at":T0, "old_session_ref":"old-session", "target":null, "offset":0,
            "saw_working":false, "compact_after":false, "initial_done":false
        }).to_string())
            })
            .unwrap();
        let action = Action::New;
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &cfg(),
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse {
            code: "refused".into(),
        });
        assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
        assert_eq!(
            session_state(&mut store, "p:heat")["step"],
            "enter",
            "a refused Enter waits for a later call"
        );
        // The text is in the box already: a busy agent does not hold the Enter.
        herdr.set_status("w1:p1", gear_herdr::AgentStatus::Working);
        assert_eq!(run(&mut store, &mut herdr, T0 + 2), Prep::Wait);
        let state = session_state(&mut store, "p:heat");
        assert_eq!(state["step"], "wait_action");
        assert_eq!(state["t0"], T0 + 2);
        assert!(herdr.calls.iter().any(|call| matches!(call, gear_herdr::fake::Call::SendKeys { keys, .. } if keys == &["enter"])));
    }

    #[test]
    fn working_and_unknown_agents_receive_no_session_commands() {
        for status in [
            gear_herdr::AgentStatus::Working,
            gear_herdr::AgentStatus::Unknown,
        ] {
            let mut store = world();
            let mut herdr = FakeHerdr::new();
            herdr.add_agent("w1:p1", "codex", status);
            let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
            let prep = crate::session_runner::run(
                &mut store,
                &mut herdr,
                &cfg(),
                &seat,
                "T-1",
                1,
                Role::Dev,
                &Action::New,
                false,
                T0,
            )
            .unwrap();
            assert_eq!(prep, Prep::Wait);
            assert!(!herdr.calls.iter().any(|call| matches!(
                call,
                gear_herdr::fake::Call::PaneType { .. } | gear_herdr::fake::Call::SendKeys { .. }
            )));
        }
    }

    fn pi_world() -> (Store, FakeHerdr, SeatRow, EngineCfg) {
        let mut store = world();
        store
            .write(|tx| {
                testing::seed_active_seat(
                    tx,
                    "p",
                    "pi-lane",
                    gear_model::SeatRole::Lane,
                    "pi:p1",
                    "pi",
                    T0,
                )?;
                tx.set_binding_session_ref("p:pi-lane", 1, "old-session")
            })
            .unwrap();
        let seat = store.read(|tx| tx.get_seat("p:pi-lane")).unwrap().unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("pi:p1", "pi", gear_herdr::AgentStatus::Idle);
        let mut config = cfg();
        config
            .profiles
            .insert("pi".into(), gear_herdr::builtin("pi").unwrap());
        (store, herdr, seat, config)
    }

    #[test]
    fn pi_new_waits_for_changed_session_or_thirty_second_fallback() {
        for changed in [false, true] {
            let (mut store, mut herdr, seat, config) = pi_world();
            store
                .write(|tx| {
                    testing::seed_active_seat(
                        tx,
                        "p",
                        "captain",
                        gear_model::SeatRole::Captain,
                        "cap:p1",
                        "codex",
                        T0,
                    )?;
                    Ok(())
                })
                .unwrap();
            let action = Action::New;
            let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
                crate::session_runner::run(
                    store,
                    herdr,
                    &config,
                    &seat,
                    "T-1",
                    1,
                    Role::Dev,
                    &action,
                    false,
                    now,
                )
                .unwrap()
            };
            assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
            assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
            if changed {
                store
                    .write(|tx| tx.set_binding_session_ref(&seat.id, 1, "new-session"))
                    .unwrap();
            }
            assert_eq!(run(&mut store, &mut herdr, T0 + 4), Prep::Wait);
            assert_eq!(
                run(&mut store, &mut herdr, T0 + if changed { 5 } else { 31 }),
                Prep::Ready
            );
            let action = store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .session_action
                .unwrap();
            assert_eq!(action, if changed { "new" } else { "new-unconfirmed" });
            let messages = store
                .read(|tx| tx.thread_messages("p", "session:p:pi-lane"))
                .unwrap();
            assert_eq!(messages.len(), if changed { 0 } else { 1 });
        }
    }

    #[test]
    fn pi_session_resume_starts_with_role_rules() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, mut herdr, seat, mut config) = pi_world();
        config.db_path = dir.path().join("gear.db").to_string_lossy().into();
        herdr.exit_agent("pi:p1");
        store
            .write(|tx| {
                tx.set_meta(&format!("engine.session_step.{}", seat.id), &serde_json::json!({
            "round":"p:T-1:1:dev", "generation":1, "action":"resume", "step":"start_resume",
            "step_at":T0, "target":"old-session"
        }).to_string())
            })
            .unwrap();
        let action = Action::Resume("old-session".into());
        assert_eq!(
            crate::session_runner::run(
                &mut store,
                &mut herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                T0 + 1
            )
            .unwrap(),
            Prep::Wait
        );
        let args = herdr
            .calls
            .iter()
            .find_map(|c| {
                if let gear_herdr::fake::Call::AgentStart { args, .. } = c {
                    Some(args)
                } else {
                    None
                }
            })
            .unwrap();
        let path = dir.path().join("rules/lane.md");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--append-system-prompt" && w[1] == path.to_string_lossy()),
            "{args:?}"
        );
        assert!(args.windows(2).any(|w| w == ["--session", "old-session"]));
    }

    #[test]
    fn compact_uses_log_or_done_and_falls_back_after_timeout() {
        for mode in ["log", "assistant", "done", "timeout"] {
            let (mut store, mut herdr, seat, config) = pi_world();
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("session.jsonl");
            std::fs::write(&file, "").unwrap();
            store
                .write(|tx| tx.set_binding_session_ref(&seat.id, 1, file.to_str().unwrap()))
                .unwrap();
            let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
                crate::session_runner::run(
                    store,
                    herdr,
                    &config,
                    &seat,
                    "T-1",
                    1,
                    Role::Dev,
                    &Action::Compact,
                    false,
                    now,
                )
                .unwrap()
            };
            assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
            assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
            herdr.set_status("pi:p1", gear_herdr::AgentStatus::Working);
            assert_eq!(run(&mut store, &mut herdr, T0 + 2), Prep::Wait);
            if mode == "log" {
                std::fs::write(&file, "{\"type\":\"compaction\"}\n").unwrap();
            }
            if mode == "assistant" {
                std::fs::write(&file, format!("{{\"type\":\"message\",\"timestamp\":{},\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":1}}}}}}\n", T0 + 2)).unwrap();
            }
            herdr.set_status(
                "pi:p1",
                if mode == "done" {
                    gear_herdr::AgentStatus::Done
                } else {
                    gear_herdr::AgentStatus::Idle
                },
            );
            assert_eq!(
                run(
                    &mut store,
                    &mut herdr,
                    T0 + if mode == "timeout" { 301 } else { 3 }
                ),
                Prep::Ready
            );
            let action = store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .session_action
                .unwrap();
            assert_eq!(
                action,
                if mode == "timeout" {
                    "compact-fallback"
                } else {
                    "compact"
                }
            );
        }
    }

    #[test]
    fn pi_compact_without_work_finishes_as_keep_after_fifteen_seconds() {
        let (mut store, mut herdr, seat, config) = pi_world();
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &Action::Compact,
                false,
                now,
            )
            .unwrap()
        };
        assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 14), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 15), Prep::Ready);
        assert_eq!(
            store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .session_action
                .as_deref(),
            Some("keep")
        );
    }

    #[test]
    fn context_size_uses_the_configured_window_and_last_usage() {
        let (mut store, _herdr, seat, config) = pi_world();
        let text = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 1
[captain]
pane = "p:captain"
kind = "pi"
profile = "pi"
[[lanes]]
name = "pi-lane"
duty = "test"
owns = []
kind = "pi"
profile = "pi"
models = [{ model = "test-model", context_window_k = 100 }]
[models]
lanes = [{ model = "test-model", context_window_k = 10 }]
"#;
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = text.into();
                tx.update_project(&project)?;
                tx.set_meta(&crate::model::model_meta_key(&seat.id), "test-model")
            })
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let write = |input| {
            std::fs::write(&path, format!("{{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":{input},\"cacheRead\":10000,\"cacheWrite\":5000}}}}}}\n")).unwrap()
        };
        write(34999);
        assert!(!context_large(&mut store, &config, &seat, path.to_str()).unwrap());
        write(35000);
        assert!(context_large(&mut store, &config, &seat, path.to_str()).unwrap());
        assert!(context_large(&mut store, &config, &seat, None).unwrap());
    }

    #[test]
    fn resume_restarts_with_session_and_falls_back_if_refused() {
        for fallback in [false, true] {
            let (mut store, mut herdr, seat, config) = pi_world();
            let action = Action::Resume("wanted-session".into());
            let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
                crate::session_runner::run(
                    store,
                    herdr,
                    &config,
                    &seat,
                    "T-1",
                    1,
                    Role::Dev,
                    &action,
                    false,
                    now,
                )
                .unwrap()
            };
            assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
            assert_eq!(run(&mut store, &mut herdr, T0 + 1), Prep::Wait);
            herdr.exit_agent("pi:p1");
            assert_eq!(run(&mut store, &mut herdr, T0 + 2), Prep::Wait);
            if fallback {
                herdr.refuse_starts = Some(("failed".into(), 1));
            }
            assert_eq!(run(&mut store, &mut herdr, T0 + 3), Prep::Wait);
            if fallback {
                assert_eq!(run(&mut store, &mut herdr, T0 + 4), Prep::Wait);
            }
            assert_eq!(run(&mut store, &mut herdr, T0 + 5), Prep::Ready);
            let args: Vec<Vec<String>> = herdr
                .calls
                .iter()
                .filter_map(|call| match call {
                    gear_herdr::fake::Call::AgentStart { args, .. } => Some(args.clone()),
                    _ => None,
                })
                .collect();
            assert!(args[0].contains(&"wanted-session".to_string()));
            if fallback {
                assert!(!args[1].contains(&"wanted-session".to_string()));
            }
            let actual = store
                .read(|tx| tx.get_attempt("p", "T-1", 1))
                .unwrap()
                .unwrap()
                .session_action
                .unwrap();
            assert_eq!(
                actual,
                if fallback {
                    "resume-fallback"
                } else {
                    "resume"
                }
            );
            if fallback {
                let starts_before = herdr
                    .calls
                    .iter()
                    .filter(|call| matches!(call, gear_herdr::fake::Call::AgentStart { .. }))
                    .count();
                assert_eq!(run(&mut store, &mut herdr, T0 + 6), Prep::Ready);
                assert_eq!(
                    herdr
                        .calls
                        .iter()
                        .filter(|call| matches!(call, gear_herdr::fake::Call::AgentStart { .. }))
                        .count(),
                    starts_before
                );
            }
        }
    }

    #[test]
    fn resume_fallback_types_the_exit_command_and_enter_in_one_step() {
        let (mut store, mut herdr, seat, config) = pi_world();
        let action = Action::Resume("wanted".into());
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        let types = |herdr: &FakeHerdr| {
            herdr.calls.iter().filter(|call| matches!(call, gear_herdr::fake::Call::PaneType { text, .. } if text == "/quit")).count()
        };
        assert_eq!(run(&mut store, &mut herdr, T0), Prep::Wait);
        assert_eq!(
            session_state(&mut store, "p:pi-lane")["step"],
            "wait_exit",
            "the initial step submits the exit command too"
        );
        assert_eq!(types(&herdr), 1);
        // The agent does not exit: the fallback types `/quit` and submits it in
        // the same call, so it cannot sit unsent while the agent looks busy.
        assert_eq!(run(&mut store, &mut herdr, T0 + 31), Prep::Wait);
        assert_eq!(
            session_state(&mut store, "p:pi-lane")["step"],
            "fallback_exit"
        );
        assert_eq!(run(&mut store, &mut herdr, T0 + 32), Prep::Wait);
        assert_eq!(
            session_state(&mut store, "p:pi-lane")["step"],
            "fallback_wait_exit"
        );
        assert_eq!(types(&herdr), 2);
        let n = herdr.calls.len();
        assert!(
            matches!(&herdr.calls[n - 2], gear_herdr::fake::Call::PaneType { text, .. } if text == "/quit"),
            "{:?}",
            &herdr.calls[n - 2..]
        );
        assert!(
            matches!(&herdr.calls[n - 1], gear_herdr::fake::Call::SendKeys { keys, .. } if keys == &["enter"]),
            "{:?}",
            &herdr.calls[n - 2..]
        );
    }

    #[test]
    fn ineffective_exit_is_bounded_and_visible() {
        let (mut store, mut herdr, seat, config) = pi_world();
        store
            .write(|tx| {
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "cap:p1",
                    "codex",
                    T0,
                )?;
                Ok(())
            })
            .unwrap();
        let action = Action::Resume("wanted".into());
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        for n in [0, 1, 31, 32, 33, 63] {
            assert_eq!(run(&mut store, &mut herdr, T0 + n), Prep::Wait);
        }
        let seat = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        assert!(seat
            .state_reason
            .as_deref()
            .unwrap()
            .contains("fallback_wait_exit"));
    }

    #[test]
    fn retry_restarts_failed_session_step_for_the_new_generation() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        store
            .write(|tx| {
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: seat.id.clone(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T0 - 1,
                })?;
                tx.add_round_session(&key, "wanted")
            })
            .unwrap();
        for n in [0, 1, 31, 32, 33, 63] {
            assert_eq!(
                prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + n).unwrap(),
                Prep::Wait
            );
        }
        assert_eq!(
            store
                .read(|tx| tx.get_seat(&seat.id))
                .unwrap()
                .unwrap()
                .state,
            SeatState::NeedsCheck
        );
        let calls = herdr.calls.len();
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 64).unwrap(),
            Prep::Wait
        );
        assert_eq!(
            herdr.calls.len(),
            calls,
            "failure remains sticky in its generation"
        );
        store
            .write(|tx| {
                tx.set_seat_state(
                    &seat.id,
                    SeatState::Queued,
                    Some("retried by the captain"),
                    T0 + 65,
                )?;
                let generation =
                    gear_app::seats::begin_onboard(tx, &seat.id, "w1:p2", "codex", T0 + 66)?;
                gear_app::seats::finish_onboard(tx, &seat.id, generation, Some("wanted"), T0 + 67)
            })
            .unwrap();
        herdr.exit_agent("w1:p1");
        herdr.add_agent("w1:p2", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(seat.generation, 2);
        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 68).unwrap(),
            Prep::Ready
        );
        let state = store
            .read(|tx| tx.get_meta("engine.session_step.p:heat"))
            .unwrap()
            .unwrap();
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(state["generation"], 2);
        assert_eq!(state["step"], "done");
    }

    #[test]
    fn fallback_starts_plain_if_the_agent_exits_before_deadline() {
        let (mut store, mut herdr, seat, config) = pi_world();
        let action = Action::Resume("wanted".into());
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        for n in [0, 1, 31, 32, 33] {
            assert_eq!(run(&mut store, &mut herdr, T0 + n), Prep::Wait);
        }
        herdr.exit_agent("pi:p1");
        assert_eq!(run(&mut store, &mut herdr, T0 + 63), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 64), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 65), Prep::Ready);
        assert!(herdr.calls.iter().any(|call| matches!(call, gear_herdr::fake::Call::AgentStart { args, .. } if !args.contains(&"wanted".to_string()))));
    }

    #[test]
    fn fallback_agent_that_never_becomes_ready_is_bounded_without_typing() {
        let (mut store, mut herdr, seat, config) = pi_world();
        store
            .write(|tx| {
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "cap:p1",
                    "codex",
                    T0,
                )?;
                Ok(())
            })
            .unwrap();
        herdr.exit_agent("pi:p1");
        herdr.refuse_starts = Some(("resume rejected".into(), 1));
        let action = Action::Resume("wanted".into());
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                false,
                now,
            )
            .unwrap()
        };
        for n in [0, 1, 2] {
            assert_eq!(run(&mut store, &mut herdr, T0 + n), Prep::Wait);
        }
        herdr.set_status("pi:p1", gear_herdr::AgentStatus::Working);
        let before = herdr.calls.len();
        assert_eq!(run(&mut store, &mut herdr, T0 + 62), Prep::Wait);
        let seat = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        assert!(seat
            .state_reason
            .as_deref()
            .unwrap()
            .contains("wait_fallback"));
        assert!(!herdr.calls[before..].iter().any(|c| matches!(
            c,
            gear_herdr::fake::Call::PaneType { .. }
                | gear_herdr::fake::Call::SendKeys { .. }
                | gear_herdr::fake::Call::AgentStart { .. }
        )));
    }

    #[test]
    fn exhausted_temp_review_recovery_reaches_needs_check_without_touching_agent() {
        let (mut store, mut herdr, _, config) = pi_world();
        store.write(|tx| {
            testing::seed_active_seat(tx, "p", "review-9", gear_model::SeatRole::Temp, "pi:review", "pi", T0)?;
            tx.set_seat_state("p:review-9", SeatState::Starting, None, T0)?;
            tx.set_meta("engine.session_step.p:review-9", &serde_json::json!({"round":"p:T-1:1:review", "generation":1, "action":"resume", "step":"fallback_wait_exit", "step_at":T0, "target":"wanted"}).to_string())
        }).unwrap();
        herdr.add_agent("pi:review", "pi", gear_herdr::AgentStatus::Working);
        let seat = store.read(|tx| tx.get_seat("p:review-9")).unwrap().unwrap();
        assert_eq!(
            crate::session_runner::run(
                &mut store,
                &mut herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Review,
                &Action::Resume("wanted".into()),
                false,
                T0 + 30
            )
            .unwrap(),
            Prep::Wait
        );
        assert_eq!(
            store
                .read(|tx| tx.get_seat(&seat.id))
                .unwrap()
                .unwrap()
                .state,
            SeatState::NeedsCheck
        );
        assert!(!herdr.calls.iter().any(|c| matches!(
            c,
            gear_herdr::fake::Call::PaneType { .. } | gear_herdr::fake::Call::SendKeys { .. }
        )));
    }

    #[test]
    fn unobservable_fallback_is_still_bounded() {
        let (mut store, mut herdr, seat, config) = pi_world();
        store.write(|tx| tx.set_meta("engine.session_step.p:pi-lane", &serde_json::json!({"round":"p:T-1:1:dev", "generation":1, "action":"resume", "step":"wait_fallback", "step_at":T0, "target":"wanted"}).to_string())).unwrap();
        herdr.observe_fails = true;
        assert_eq!(
            crate::session_runner::run(
                &mut store,
                &mut herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &Action::Resume("wanted".into()),
                false,
                T0 + 60
            )
            .unwrap(),
            Prep::Wait
        );
        let seat = store.read(|tx| tx.get_seat(&seat.id)).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        assert!(seat
            .state_reason
            .as_deref()
            .unwrap()
            .contains("wait_fallback"));
    }

    #[test]
    fn a_large_followup_compacts_after_resuming() {
        let (mut store, mut herdr, seat, config) = pi_world();
        let action = Action::Resume("wanted-session".into());
        let run = |store: &mut Store, herdr: &mut FakeHerdr, now| {
            crate::session_runner::run(
                store,
                herdr,
                &config,
                &seat,
                "T-1",
                1,
                Role::Dev,
                &action,
                true,
                now,
            )
            .unwrap()
        };
        run(&mut store, &mut herdr, T0);
        run(&mut store, &mut herdr, T0 + 1);
        herdr.exit_agent("pi:p1");
        run(&mut store, &mut herdr, T0 + 2);
        run(&mut store, &mut herdr, T0 + 3);
        assert_eq!(run(&mut store, &mut herdr, T0 + 4), Prep::Wait);
        assert_eq!(run(&mut store, &mut herdr, T0 + 5), Prep::Wait);
        assert!(herdr.calls.iter().any(|call| matches!(call, gear_herdr::fake::Call::PaneType { text, .. } if text == "/compact")));
        assert!(!herdr
            .calls
            .iter()
            .any(|call| matches!(call, gear_herdr::fake::Call::Prompt { .. })));
    }

    /// The facts of a lane round, with the defaults of a seat that works.
    fn lane_facts() -> Facts {
        Facts {
            seat: "p:heat".into(),
            seat_state: SeatState::Active,
            role: Role::Dev,
            attempt: 1,
            followup_of: None,
            earlier_seat: None,
            earlier_cli: None,
            earlier_session: None,
            current_session: Some("session-a".into()),
            current_cli: Some("codex".into()),
            agent_live: true,
            open_round_session: None,
            sessions_elsewhere: Vec::new(),
        }
    }

    /// L2: a rework that ran on this seat, on the same CLI, in the session
    /// the seat runs on now → keep.
    #[test]
    fn l2_a_rework_on_the_same_session_keeps_it() {
        let mut facts = lane_facts();
        facts.role = Role::Rework;
        facts.attempt = 2;
        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-a".into());

        assert_eq!(choose(&facts), Action::Keep);
    }

    /// L3: the same rework, but the seat runs on another session now → resume
    /// the session of attempt n−1.
    #[test]
    fn l3_a_rework_whose_session_moved_resumes_it() {
        let mut facts = lane_facts();
        facts.role = Role::Rework;
        facts.attempt = 2;
        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-old".into());
        facts.current_session = Some("session-a".into());

        assert_eq!(choose(&facts), Action::Resume("session-old".into()));
    }

    /// L4: a rework that ran on another seat (or on another CLI) → a new
    /// conversation.
    #[test]
    fn l4_a_rework_on_another_seat_starts_a_new_session() {
        let mut facts = lane_facts();
        facts.role = Role::Rework;
        facts.attempt = 2;
        facts.earlier_seat = Some("p:other".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-a".into());

        assert_eq!(choose(&facts), Action::New);

        // The same seat, another CLI.
        let mut facts = facts.clone();
        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_cli = Some("opencode".into());
        assert_eq!(choose(&facts), Action::New);

        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = None;
        assert_eq!(choose(&facts), Action::New);
        facts.agent_live = false;
        assert_eq!(choose(&facts), Action::Fresh);
    }

    /// L5: a follow-up whose earlier attempt ran on this seat, on the same
    /// CLI, in the session the seat runs on now → keep.
    #[test]
    fn l5_a_follow_up_on_the_same_session_keeps_it() {
        let mut facts = lane_facts();
        facts.followup_of = Some("T-7".into());
        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-a".into());

        assert_eq!(choose(&facts), Action::Keep);
    }

    /// L6: the same follow-up, but the seat runs on another session → resume
    /// the session of the task it follows.
    #[test]
    fn l6_a_follow_up_whose_session_moved_resumes_it() {
        let mut facts = lane_facts();
        facts.followup_of = Some("T-7".into());
        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-old".into());
        facts.current_session = Some("session-a".into());

        assert_eq!(choose(&facts), Action::Resume("session-old".into()));
    }

    /// L7: an unrelated task → a new conversation.
    #[test]
    fn l7_an_unrelated_task_starts_a_new_session() {
        let facts = lane_facts();
        assert_eq!(choose(&facts), Action::New);

        // A follow-up whose earlier attempt ran on another seat: L7 as well.
        let mut facts = facts;
        facts.followup_of = Some("T-7".into());
        facts.earlier_seat = Some("p:other".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-a".into());
        assert_eq!(choose(&facts), Action::New);

        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_session = None;
        assert_eq!(choose(&facts), Action::New);
        facts.agent_live = false;
        assert_eq!(choose(&facts), Action::Fresh);
    }

    /// L1: a seat without a live agent has its agent started plainly, and the
    /// action becomes `fresh`.
    #[test]
    fn l1_a_seat_without_a_live_agent_starts_fresh() {
        let mut facts = lane_facts();
        facts.agent_live = false;
        assert_eq!(choose(&facts), Action::Fresh);

        // A rule that says `resume` keeps its resume args.
        let mut facts = facts;
        facts.attempt = 2;
        facts.role = Role::Rework;
        facts.earlier_seat = Some("p:heat".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-old".into());
        assert_eq!(choose(&facts), Action::Resume("session-old".into()));
    }

    /// V1: a re-review on the seat that judged the task before, on the same
    /// session → keep; on another session → resume it.
    #[test]
    fn v1_a_re_review_keeps_or_resumes_the_session_of_the_earlier_round() {
        let mut facts = Facts {
            seat: "p:review-1".into(),
            seat_state: SeatState::Standby,
            role: Role::Review,
            attempt: 1,
            followup_of: None,
            earlier_seat: Some("p:review-1".into()),
            earlier_cli: Some("codex".into()),
            earlier_session: Some("session-r".into()),
            current_session: Some("session-r".into()),
            current_cli: Some("codex".into()),
            agent_live: true,
            open_round_session: None,
            sessions_elsewhere: Vec::new(),
        };
        assert_eq!(choose(&facts), Action::Keep);

        facts.agent_live = false;
        assert_eq!(choose(&facts), Action::Resume("session-r".into()));
        facts.agent_live = true;
        facts.current_session = Some("session-other".into());
        assert_eq!(choose(&facts), Action::Resume("session-r".into()));
    }

    /// V2: a review of another task (or on another seat) → a new conversation.
    #[test]
    fn v2_a_review_of_another_task_starts_a_new_session() {
        let mut facts = Facts {
            seat: "p:review-2".into(),
            seat_state: SeatState::Standby,
            role: Role::Review,
            attempt: 1,
            followup_of: None,
            earlier_seat: None,
            earlier_cli: None,
            earlier_session: None,
            current_session: Some("session-r".into()),
            current_cli: Some("codex".into()),
            agent_live: true,
            open_round_session: None,
            sessions_elsewhere: Vec::new(),
        };
        assert_eq!(choose(&facts), Action::New);

        // The earlier round of this task ran on another seat.
        facts.earlier_seat = Some("p:review-1".into());
        facts.earlier_cli = Some("codex".into());
        facts.earlier_session = Some("session-r".into());
        assert_eq!(choose(&facts), Action::New);
    }

    /// S1: the agent died while its round was open → the round resumes its
    /// session, in the same round.
    #[test]
    fn s1_a_dead_agent_in_an_open_round_resumes_the_round_session() {
        let mut facts = lane_facts();
        facts.open_round_session = Some("session-a".into());
        facts.agent_live = false;
        assert_eq!(choose(&facts), Action::Resume("session-a".into()));

        // The agent was restarted: it runs on another session now.
        let mut facts = lane_facts();
        facts.open_round_session = Some("session-old".into());
        facts.current_session = Some("session-new".into());
        assert_eq!(choose(&facts), Action::Resume("session-old".into()));

        // The round goes on: the agent is alive on the round's session.
        let mut facts = lane_facts();
        facts.open_round_session = Some("session-a".into());
        assert_ne!(choose(&facts), Action::Resume(String::new()));
    }

    /// S3: a review round whose earlier session another pane's live agent
    /// holds starts a new conversation; the same round without such an agent
    /// resumes it.
    #[test]
    fn s3_a_review_of_a_session_held_elsewhere_starts_fresh() {
        let mut facts = Facts {
            seat: "p:review-1".into(),
            seat_state: SeatState::Standby,
            role: Role::Review,
            attempt: 1,
            followup_of: None,
            earlier_seat: Some("p:review-1".into()),
            earlier_cli: Some("codex".into()),
            earlier_session: Some("session-r".into()),
            current_session: Some("session-other".into()),
            current_cli: Some("codex".into()),
            agent_live: true,
            open_round_session: None,
            sessions_elsewhere: Vec::new(),
        };
        assert_eq!(choose(&facts), Action::Resume("session-r".into()));

        facts.sessions_elsewhere = vec!["session-r".into()];
        assert_eq!(
            choose(&facts),
            Action::Fresh,
            "another pane's agent holds it"
        );

        // Another session held elsewhere is not this round's concern.
        facts.sessions_elsewhere = vec!["session-z".into()];
        assert_eq!(choose(&facts), Action::Resume("session-r".into()));
    }

    /// S3: an S1 resume of a session another pane's live agent holds starts a
    /// new conversation (the original symptom: the seat sat locked).
    #[test]
    fn s3_an_s1_resume_of_a_session_held_elsewhere_starts_fresh() {
        // The agent of the open round died, and another pane's live agent runs
        // the round's session.
        let mut facts = lane_facts();
        facts.open_round_session = Some("session-a".into());
        facts.agent_live = false;
        facts.sessions_elsewhere = vec!["session-a".into()];
        assert_eq!(choose(&facts), Action::Fresh);

        // The agent was restarted on another session, and the round's session
        // is held elsewhere.
        let mut facts = lane_facts();
        facts.open_round_session = Some("session-old".into());
        facts.current_session = Some("session-new".into());
        facts.sessions_elsewhere = vec!["session-old".into()];
        assert_eq!(choose(&facts), Action::Fresh);

        // With no such holder, S1 resumes as it did.
        facts.sessions_elsewhere = Vec::new();
        assert_eq!(choose(&facts), Action::Resume("session-old".into()));
    }

    /// F1 of the T-423 review (S3): the recovery of an open round calls `s1`
    /// directly, so the guard holds there too — a lane round whose session
    /// another pane's live agent holds starts a new conversation.
    #[test]
    fn s3_an_open_lane_round_held_elsewhere_is_not_resumed() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
        };
        store
            .write(|tx| {
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: seat.id.clone(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T0,
                })?;
                tx.add_round_session(&key, "round-session")?;
                // The seat was restarted on another session: S1 would resume
                // the round's.
                tx.set_binding_session_ref(&seat.id, 1, "replacement")
            })
            .unwrap();
        herdr.exit_agent("w1:p1");
        // Another pane's live agent runs the round's session.
        herdr.add_agent("w9:p1", "codex", gear_herdr::AgentStatus::Idle);
        herdr.set_agent_session("w9:p1", "round-session");

        assert_eq!(
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 10).unwrap(),
            Prep::Ready,
            "the seat starts a new conversation"
        );

        let state = store
            .read(|tx| tx.get_meta("engine.session_step.p:heat"))
            .unwrap()
            .unwrap();
        let state: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(
            state["action"], "fresh",
            "S1 does not resume a held session: {state}"
        );
        assert!(
            !herdr.calls.iter().any(|call| matches!(call, gear_herdr::fake::Call::AgentStart { args, .. } if args.contains(&"round-session".to_string()))),
            "the held session is not started: {:?}",
            herdr.calls
        );
    }

    /// F1 of the T-423 review (S3): an open **review** round in the same
    /// recovery starts a new conversation too.
    #[test]
    fn s3_an_open_review_round_held_elsewhere_is_not_resumed() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        let key = gear_store::RoundKey {
            project: "p".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "review".into(),
        };
        store
            .write(|tx| {
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: seat.id.clone(),
                    generation: seat.generation,
                    cli: "codex".into(),
                    model: None,
                    start_at: T0,
                })?;
                tx.add_round_session(&key, "review-held")?;
                tx.set_binding_session_ref(&seat.id, seat.generation, "review-other")
            })
            .unwrap();
        herdr.exit_agent("w2:p2");
        herdr.add_agent("w9:p1", "codex", gear_herdr::AgentStatus::Idle);
        herdr.set_agent_session("w9:p1", "review-held");

        assert_eq!(
            prepare_review(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 10).unwrap(),
            Prep::Ready,
            "the review starts a new conversation"
        );
        let action = store
            .read(|tx| tx.get_meta(&review_action_key("p", "T-1", 1)))
            .unwrap()
            .unwrap();
        let action: serde_json::Value = serde_json::from_str(&action).unwrap();
        assert_eq!(
            action["action"], "fresh",
            "a held review session is not resumed: {action}"
        );
    }

    /// S3: both facts builders read the sessions of the other panes' live
    /// agents from Herdr, never the seat's own.
    #[test]
    fn s3_the_facts_name_the_sessions_the_other_panes_hold() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w3:p3", "codex", gear_herdr::AgentStatus::Idle);
        herdr.add_agent("w4:p4", "codex", gear_herdr::AgentStatus::Idle);
        herdr.set_agent_session("w1:p1", "lane-own");
        herdr.set_agent_session("w2:p2", "review-own");
        herdr.set_agent_session("w3:p3", "held-a");
        herdr.set_agent_session("w4:p4", "held-b");

        let lane = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        let facts = super::lane_facts(&mut store, &mut herdr, "p", &lane, "T-1", 1).unwrap();
        assert_eq!(
            facts.sessions_elsewhere,
            vec!["review-own".to_string(), "held-a".into(), "held-b".into()]
        );

        let review = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        let facts = super::review_facts(&mut store, &mut herdr, "p", &review, "T-1", 1).unwrap();
        assert_eq!(
            facts.sessions_elsewhere,
            vec!["lane-own".to_string(), "held-a".into(), "held-b".into()]
        );
    }

    /// The round row is created once: two calls over the same round make one
    /// row, and the action is the same both times.
    #[test]
    fn the_round_row_is_created_once_over_two_calls() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();

        let first = prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 10).unwrap();
        let second =
            prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 12).unwrap();

        assert_eq!(first, Prep::Wait, "the seat has no session yet: a new one");
        assert_eq!(second, Prep::Wait, "the round goes on with its action");
        let rounds = store.read(|tx| tx.rounds_of_task("p", "T-1")).unwrap();
        assert_eq!(rounds.len(), 1, "one round row: {rounds:?}");
        assert_eq!(rounds[0].role, "dev");
        assert_eq!(rounds[0].seat, "p:heat");
        let action = store
            .read(|tx| tx.get_attempt("p", "T-1", 1))
            .unwrap()
            .unwrap()
            .session_action;
        assert_eq!(action.as_deref(), Some("new"), "the action is recorded");
    }

    /// A rework's round row carries the role `rework`.
    #[test]
    fn a_rework_round_is_a_rework() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        store
            .write(|tx| {
                // The first attempt is handed in; the rework starts.
                let mut attempt = tx.get_attempt("p", "T-1", 1)?.unwrap();
                attempt.state = AttemptState::Submitted;
                attempt.candidate = Some("abc1234".into());
                tx.update_attempt(&attempt)?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)
            })
            .unwrap();

        prepare_lane(&mut store, &mut herdr, &cfg(), &seat, "T-1", 2, T0 + 10).unwrap();

        let rounds = store.read(|tx| tx.rounds_of_task("p", "T-1")).unwrap();
        assert_eq!(rounds.len(), 1, "{rounds:?}");
        assert_eq!(rounds[0].role, "rework", "the role of the row");
        assert_eq!(rounds[0].attempt, 2);
    }

    /// A review round: its action is recorded, and the round row is a review.
    #[test]
    fn a_review_round_is_recorded_on_its_own_key() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w2:p2", "codex", gear_herdr::AgentStatus::Idle);
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();

        let prep =
            prepare_review(&mut store, &mut herdr, &cfg(), &seat, "T-1", 1, T0 + 10).unwrap();

        assert_eq!(prep, Prep::Wait, "a review of a new task starts a session");
        let rounds = store.read(|tx| tx.rounds_of_task("p", "T-1")).unwrap();
        assert_eq!(rounds.len(), 1, "{rounds:?}");
        assert_eq!(rounds[0].role, "review");
        let action = store
            .read(|tx| tx.get_meta(&review_action_key("p", "T-1", 1)))
            .unwrap();
        let action: serde_json::Value = serde_json::from_str(action.as_deref().unwrap()).unwrap();
        assert_eq!(action["action"], "new", "the action is recorded");
    }

    /// An engine that knows the codex profile.
    fn cfg() -> EngineCfg {
        let mut cfg = EngineCfg {
            db_path: crate::testtmp::temp("gear-engine-tests")
                .join("gear.db")
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        cfg.profiles
            .insert("codex".to_string(), gear_herdr::builtin("codex").unwrap());
        cfg
    }

    /// A project with a lane, a reviewer and a task whose attempt is handed
    /// in.
    fn world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                tx.insert_project(&gear_store::ProjectRow {
                    id: "p".into(),
                    herdr_session: "sess".into(),
                    config_toml: String::new(),
                    config_version: 1,
                    quota_lanes: 1,
                    quota_temps: 2,
                    quota_includes_captain: false,
                    plan_version: 0,
                    created: T0,
                })?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "heat",
                    gear_model::SeatRole::Lane,
                    "w1:p1",
                    "codex",
                    T0,
                )?;
                testing::seed_temp_seat(tx, "p", "review-1", "T-1", "w2:p2", "codex", T0)?;
                tx.insert_task(&gear_store::TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: gear_model::TaskState::Ready,
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
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                Ok(())
            })
            .unwrap();
        store
    }
}
