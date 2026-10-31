//! Contract (T-115): a reviewer whose Codex limit is spent is moved by the
//! engine to the next model of its own chain (`[models].review`,
//! `on_quota.review`), and the review job goes with it.
//!
//! The trial's B02-03 shape for the reviewer: a Codex reviewer whose weekly
//! limit is spent used to look idle. The marker makes it seen as on quota; the
//! policy moves it to `opencode-go/deepseek-v4-pro` — another CLI, so the
//! engine closes the Codex agent, starts OpenCode in the same pane and hands
//! the **same** task and attempt to the new agent as a full review packet.

use std::collections::BTreeSet;

use gear_app::tasks::start_attempt;
use gear_engine::observe::{Presence, SeatObs};
use gear_engine::quota::apply_quota_policy;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::{AgentStatus, ScreenFlag};
use gear_model::{AttemptState, MsgState, OpKind, OpState, SeatRole, SeatState, TaskState};
use gear_store::{testing, ContractRow, SeatRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const REVIEW_PANE: &str = "w7:p1";
const LANE_PANE: &str = "w1:p2";
const CAPTAIN_PANE: &str = "w9:p1";

/// The screen the installed codex binary prints when the weekly limit is spent
/// (`You’ve hit your usage limit.`, curly apostrophe; codex-cli 0.164.0).
const SPENT_LIMIT: &str = "You’ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/settings/usage to purchase more credits";
/// The status bar OpenCode shows once it runs the fallback review model.
const OPENCODE_BAR: &str = "Build auto · DeepSeek V4 Pro (New) OpenCode Go · high";

fn config(on_quota_review: &str) -> String {
    format!(
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 2

[captain]
pane = "w9:p1"
kind = "claude"
profile = "claude"

[temps]
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = [
  {{ model = "gpt-6.1-sol", cli = "codex" }},
  {{ model = "opencode-go/deepseek-v4-pro", cli = "opencode" }},
]
on_quota = {{ lanes = "fallback", review = "{on_quota_review}" }}
quota_reset_h = 5

[models.switch."opencode-go/deepseek-v4-pro"]
picker = "DeepSeek V4 Pro (New) OpenCode Go"
bar = "DeepSeek V4 Pro (New) OpenCode Go"
"#
    )
}

/// A project whose task T-7 was handed in by lane `heat` and whose reviewer
/// `r-1` (a Codex temp seat) holds the job.
fn world(on_quota_review: &str) -> (Store, SeatRow) {
    let mut store = Store::open_in_memory().unwrap();
    let seat = store
        .write(|tx| {
            let project = testing::seed_project(tx, "p", "sess", T0)?;
            let mut project = project;
            project.config_toml = config(on_quota_review);
            tx.update_project(&project)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAPTAIN_PANE,
                "claude",
                T0,
            )?;
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
                goal: "the goal".into(),
                accept: "the accept".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
            let mut attempt = tx.attempts_of_task("p", "T-7")?.pop().unwrap();
            attempt.state = AttemptState::Submitted;
            attempt.candidate = Some("abc1234".into());
            tx.update_attempt(&attempt)?;
            let mut task = tx.get_task("p", "T-7")?.unwrap();
            task.state = TaskState::Verifying;
            tx.update_task(&task)?;
            let seat = testing::seed_temp_seat(tx, "p", "r-1", "T-7", REVIEW_PANE, "codex", T0)?;
            // The packet it was given, delivered.
            let op = tx.insert_op(&gear_store::NewOp {
                project: "p".into(),
                seat: seat.id.clone(),
                generation: 1,
                kind: OpKind::Deliver,
                task: Some("T-7".into()),
                attempt: Some(1),
                ref_msg: None,
                plan_version: None,
                payload: serde_json::json!({ "text": "Review T-7 attempt 1." }),
                created: T0,
            })?;
            tx.set_op_state("p", &op.id, OpState::Confirmed, Some("shown"), T0 + 1)?;
            Ok(seat)
        })
        .unwrap();
    (store, seat)
}

fn cfg() -> EngineCfg {
    let mut cfg = EngineCfg {
        db_path: crate::temp("gear-engine-tests")
            .join("gear.db")
            .to_string_lossy()
            .into_owned(),
        ..Default::default()
    };
    for kind in ["opencode", "claude", "codex"] {
        cfg.profiles
            .insert(kind.into(), gear_herdr::builtin(kind).unwrap());
    }
    cfg
}

/// The reviewer's observation this tick: on the quota screen, idle.
fn observed(seat: &SeatRow, quota: bool, idle: bool) -> Vec<SeatObs> {
    let mut flags = BTreeSet::new();
    if quota {
        flags.insert(ScreenFlag::Quota);
    }
    vec![SeatObs {
        seat: seat.clone(),
        presence: Presence::Present,
        status: Some(if idle {
            AgentStatus::Idle
        } else {
            AgentStatus::Working
        }),
        flags,
        idle,
        screen: None,
        session: None,
    }]
}

fn reviewer(store: &mut Store) -> SeatRow {
    store.read(|tx| tx.get_seat("p:r-1")).unwrap().unwrap()
}

fn tick(
    store: &mut Store,
    herdr: &mut FakeHerdr,
    quota: bool,
    now: i64,
) -> gear_engine::quota::QuotaReport {
    let seat = reviewer(store);
    let obs = observed(&seat, quota, true);
    apply_quota_policy(store, herdr, &cfg(), &cfg().profiles, "p", &obs, now).unwrap()
}

fn herdr() -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(REVIEW_PANE, "codex", AgentStatus::Idle);
    herdr.set_screen(REVIEW_PANE, SPENT_LIMIT);
    // The fallback agent comes up on the chain's model: the startup gate sees
    // the status bar and lets the job through.
    herdr.start_screen = Some(OPENCODE_BAR.into());
    herdr
}

fn exits(herdr: &FakeHerdr) -> Vec<Vec<String>> {
    herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::SendKeys { keys, .. } => Some(keys.clone()),
            _ => None,
        })
        .collect()
}

/// The commands typed into a pane (the exit of a CLI that takes a command
/// instead of a key, M4 §12 item 4).
fn exit_prompts(herdr: &FakeHerdr) -> Vec<String> {
    herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::Prompt { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn starts(herdr: &FakeHerdr) -> Vec<(String, String, Vec<String>)> {
    herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::AgentStart {
                kind, pane, args, ..
            } => Some((kind.clone(), pane.clone(), args.clone())),
            _ => None,
        })
        .collect()
}

/// The whole move: Codex stop → the exit keys → OpenCode in the same pane on
/// the fallback review model → the packet for the same attempt → a verdict
/// that settles it.
#[test]
fn a_spent_codex_reviewer_is_restarted_on_the_review_fallback() {
    let (mut store, _seat) = world("fallback");
    let mut herdr = herdr();

    // One quota screen is not enough for the codex profile (`confirm_rounds`).
    let report = tick(&mut store, &mut herdr, true, T0 + 10);
    assert!(report.events.is_empty(), "{:?}", report.events);
    assert!(
        exits(&herdr).is_empty(),
        "nothing is closed on the first tick"
    );

    // The limit is confirmed: the Codex agent is told to exit.
    let report = tick(&mut store, &mut herdr, true, T0 + 20);
    assert!(report.events.is_empty(), "{:?}", report.events);
    let exit_command = gear_herdr::builtin("codex").unwrap().exit_command;
    // M4 §12 item 4: codex exits with the typed `/quit`, not with keys.
    assert_eq!(
        exit_prompts(&herdr),
        vec![exit_command.unwrap_or_default()],
        "{:?}",
        herdr.calls
    );
    assert!(exits(&herdr).is_empty(), "no exit keys for codex any more");
    assert!(
        starts(&herdr).is_empty(),
        "nothing starts before the pane is a shell"
    );
    assert!(
        gear_engine::restart::pending(&mut store, "p:r-1")
            .unwrap()
            .is_some(),
        "the restart is durable"
    );
    // The seat leaves the doorbell's reach before its agent goes.
    assert_eq!(reviewer(&mut store).state, SeatState::Missing);

    // The agent leaves: OpenCode starts in the same pane on the fallback model
    // and the reviewer gets its packet again.
    herdr.exit_agent(REVIEW_PANE);
    let report = tick(&mut store, &mut herdr, false, T0 + 30);

    let started = starts(&herdr);
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0].0, "opencode", "the CLI of the fallback entry");
    assert_eq!(started[0].1, REVIEW_PANE, "the same pane");
    assert_eq!(
        started[0].2,
        vec!["--auto".to_string()],
        "the model is carried in the environment, not as a flag: {started:?}"
    );

    let seat = reviewer(&mut store);
    assert_eq!(seat.state, SeatState::Active, "{:?}", seat.state_reason);
    assert_eq!(seat.generation, 2, "the fallback is a new generation");
    assert_eq!(seat.profile, "opencode");
    assert_eq!(
        seat.task_id.as_deref(),
        Some("T-7"),
        "the seat keeps its job"
    );
    let binding = store
        .read(|tx| tx.get_binding("p:r-1", 2))
        .unwrap()
        .unwrap();
    assert_eq!(binding.kind, "opencode");
    assert_eq!(binding.pane_id, REVIEW_PANE);
    assert_eq!(
        binding.status,
        gear_store::BindingStatus::Active,
        "the new agent is up before the packet goes"
    );
    assert_eq!(report.events.len(), 1, "{:?}", report.events);
    assert!(
        report.events[0].contains("restarted on opencode"),
        "{:?}",
        report.events
    );

    // The job travels with the seat: a full review packet for the same task
    // and attempt, not the lane's "Continue T-n".
    let packet = store
        .read(|tx| tx.last_op("p:r-1", OpKind::Deliver))
        .unwrap()
        .expect("the packet is delivered again");
    assert_eq!(packet.task.as_deref(), Some("T-7"));
    assert_eq!(packet.attempt, Some(1));
    assert_eq!(packet.generation, 2);
    let text = packet.payload["text"].as_str().unwrap_or_default();
    // T-465: the restarted reviewer's packet starts with the handover of the
    // agent that was switched, then the review packet.
    assert!(
        text.starts_with("## Handover — you take over another agent's work"),
        "{text}"
    );
    assert!(text.contains("Review T-7 attempt 1."), "{text}");
    assert!(
        text.contains("abc1234"),
        "the candidate rides along: {text}"
    );
    assert!(
        text.contains("gear send --kind review --task T-7 --attempt 1"),
        "{text}"
    );
    assert!(
        !text.contains("Continue T-7"),
        "not a lane's continue line: {text}"
    );

    // The captain hears about the move in an information line.
    let told: Vec<_> = store
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 40))
        .unwrap();
    assert!(
        told.is_empty(),
        "a successful fallback does not ring: {told:?}"
    );
    let info = store.read(|tx| tx.thread_messages("p", "r-1:1")).unwrap();
    assert_eq!(info.len(), 1, "{info:?}");
    assert!(
        info[0].state == MsgState::Info && !info[0].urgent,
        "{info:?}"
    );
    assert!(info[0].body.contains("restarted on opencode"), "{info:?}");

    // And the verdict settles the same attempt on the new agent.
    store
        .write(|tx| {
            gear_app::review::submit_review(
                tx,
                &gear_app::review::Review {
                    project: "p".into(),
                    task: "T-7".into(),
                    attempt: 1,
                    outcome: gear_app::review::Outcome::Pass,
                    findings: "ok".into(),
                    blocking: String::new(),
                    by_seat: "p:r-1".into(),
                },
                T0 + 50,
            )
        })
        .unwrap();
    assert!(
        store
            .read(|tx| tx.get_review_verdict("p", "T-7", 1))
            .unwrap()
            .is_some(),
        "the restarted reviewer's verdict lands on the attempt it judged"
    );
}

/// `on_quota.review = "wait"`: a reviewer cannot be parked (its role has no
/// Parked state), so the seat is flagged once and the captain is rung — the
/// review of T-7 waits for a human instead of vanishing.
#[test]
fn a_reviewer_on_wait_is_flagged_once_with_its_task() {
    let (mut store, _seat) = world("wait");
    let mut herdr = herdr();

    tick(&mut store, &mut herdr, true, T0 + 10);
    let report = tick(&mut store, &mut herdr, true, T0 + 20);

    assert_eq!(report.events.len(), 1, "{:?}", report.events);
    assert_eq!(reviewer(&mut store).state, SeatState::NeedsCheck);
    assert!(
        exits(&herdr).is_empty(),
        "nothing is closed: {:?}",
        herdr.calls
    );
    assert!(starts(&herdr).is_empty());
    let open = store
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 30))
        .unwrap();
    assert_eq!(open.len(), 1, "one urgent item: {open:?}");
    assert!(open[0].urgent, "{open:?}");
    assert!(
        open[0].body.contains("T-7"),
        "it names the review: {open:?}"
    );

    // A later tick says nothing new.
    tick(&mut store, &mut herdr, true, T0 + 40);
    let open = store
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 50))
        .unwrap();
    assert_eq!(open.len(), 1, "told once: {open:?}");
}

/// A stale limit line on a **working** reviewer is not a quota turn: it is
/// never restarted on screen text alone (the lane's M-266 rule).
#[test]
fn a_working_reviewer_with_a_stale_limit_line_is_left_alone() {
    let (mut store, _seat) = world("fallback");
    let mut herdr = herdr();
    let seat = reviewer(&mut store);

    for tick_no in 0..5 {
        let obs = observed(&seat, true, false);
        apply_quota_policy(
            &mut store,
            &mut herdr,
            &cfg(),
            &cfg().profiles,
            "p",
            &obs,
            T0 + 10 + tick_no,
        )
        .unwrap();
    }

    assert_eq!(reviewer(&mut store).state, SeatState::Active);
    assert_eq!(reviewer(&mut store).generation, 1);
    assert!(exits(&herdr).is_empty(), "no exit keys: {:?}", herdr.calls);
    assert!(starts(&herdr).is_empty());
}
