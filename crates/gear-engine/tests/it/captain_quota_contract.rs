use gear_engine::{tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow, ViewRecord};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";

fn screen(percent: u32) -> String {
    format!("conversation\n────────────────────────\n❯\n────────────────────────\n  Opus 5.5 (1M context) · medium /repo (dev)  ·  ctx 63%  ·  5h {percent}%\n  ⏵⏵ bypass permissions on (shift+tab to cycle)\n")
}

fn world() -> (Store, FakeHerdr, EngineCfg) {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"
[quota]
lanes = 1
temps = 1
[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"
[[lanes]]
name = "heat"
duty = "work"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
[models]
quota_reset_h = 1
"#
            .into();
            tx.update_project(&project)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "claude", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "opencode", T0)?;
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
                 issue: None,updated: T0,
            })?;
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-1".into(),
                version: 1,
                goal: "finish".into(),
                accept: "done".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "claude", AgentStatus::Idle);
    herdr.add_agent(LANE, "opencode", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.review_git = std::sync::Arc::new(crate::FakeReviewGit);
    for kind in ["claude", "opencode", "codex"] {
        let mut profile = gear_herdr::builtin(kind).unwrap();
        profile.confirm_rounds = 1;
        if kind == "opencode" { profile.new_session.clear(); }
        cfg.profiles.insert(kind.into(), profile);
    }
    (store, herdr, cfg)
}

fn view(store: &mut Store, now: i64) {
    store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: "p:captain".into(),
                generation: 1,
                ts: now,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: None,
                shown_attempt: None,
                shown_contract: None,
            })
        })
        .unwrap();
}

#[test]
fn the_captain_limit_pauses_new_tasks_until_reset_and_a_new_view() {
    let (mut store, mut herdr, cfg) = world();
    herdr.set_screen(CAP, &screen(100));
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    assert!(
        herdr.prompts_to(LANE).is_empty(),
        "a captain limit must stop new tasks"
    );
    // A view during the outage cannot acknowledge a future reset.
    view(&mut store, T0 + 20);
    tick(&mut store, &mut herdr, &cfg, T0 + 30).unwrap();
    assert!(herdr.prompts_to(CAP).is_empty());
    let reset = T0 + 10 + 3600;
    view(&mut store, reset); // a view in the same second, before the wake intent
    tick(&mut store, &mut herdr, &cfg, reset).unwrap();
    assert_eq!(herdr.prompts_to(CAP).len(), 1);
    assert!(herdr.prompts_to(CAP)[0].contains("quota is back"));
    tick(&mut store, &mut herdr, &cfg, reset + 10).unwrap();
    assert_eq!(herdr.prompts_to(CAP).len(), 1, "the wake-up is sent once");
    assert!(
        herdr.prompts_to(LANE).is_empty(),
        "wait for the captain's next view"
    );
    view(&mut store, reset + 20);
    tick(&mut store, &mut herdr, &cfg, reset + 30).unwrap();
    assert_eq!(
        herdr.prompts_to(LANE).len(),
        1,
        "new work resumes after the view"
    );
}

#[test]
fn quota_words_and_quoted_bars_in_the_conversation_do_nothing() {
    let (mut store, mut herdr, cfg) = world();
    let conversation = format!("quota exhausted\n{}\n{}", screen(100), screen(4));
    herdr.set_screen(CAP, &conversation);
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    assert_eq!(herdr.prompts_to(LANE).len(), 1);
}

#[test]
fn current_work_can_continue_and_hand_in_but_the_next_task_waits() {
    let (mut store, mut herdr, cfg) = world();
    store
        .write(|tx| {
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            let mut next = tx.get_task("p", "T-1")?.unwrap();
            next.id = "T-2".into();
            next.rank = 2;
            next.state = TaskState::Ready;
            tx.insert_task(&next)
        })
        .unwrap();
    herdr.set_screen(CAP, &screen(100));
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    // T-150 (design §10.3): the attempt already started — the lane got its
    // packet and checked in — so the engine sends no second one.
    assert!(
        herdr.prompts_to(LANE).is_empty(),
        "no second packet: {:?}",
        herdr.prompts_to(LANE)
    );
    store
        .write(|tx| {
            tx.insert_view(&ViewRecord {
                project: "p".into(),
                seat: "p:heat".into(),
                generation: 1,
                ts: T0 + 11,
                shown_msgs: vec![],
                shown_ops: vec![],
                shown_task: Some("T-1".into()),
                shown_attempt: Some(1),
                shown_contract: Some(1),
            })?;
            gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "report", T0 + 20)
        })
        .unwrap();
    tick(&mut store, &mut herdr, &cfg, T0 + 30).unwrap();
    assert_eq!(herdr.prompts_to(LANE).len(), 0, "the captain is on quota: the next task waits");
    assert_eq!(
        store
            .read(|tx| tx.get_task("p", "T-2"))
            .unwrap()
            .unwrap()
            .state,
        TaskState::Ready
    );
}

#[test]
fn a_handed_in_task_still_gets_its_reviewer() {
    let (mut store, mut herdr, cfg) = world();
    store
        .write(|tx| {
            let mut project = tx.get_project("p")?.unwrap();
            project
                .config_toml
                .push_str("\n[temps]\nkind = \"codex\"\nprofile = \"codex\"\n");
            tx.update_project(&project)?;
            gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
            gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
            gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "report", T0 + 1)
        })
        .unwrap();
    herdr.set_screen(CAP, &screen(100));
    let report = tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    let binding = store
        .read(|tx| tx.get_binding("p:review-1", 1))
        .unwrap()
        .unwrap();
    assert_eq!(report.reviewers, vec!["p:review-1"]);
    assert!(herdr
        .prompts_to(&binding.pane_id)
        .iter()
        .any(|text| text.starts_with("Review T-1")));
    assert!(store.read(|tx| tx.captain_quota_paused("p")).unwrap());
}

#[test]
fn a_working_captain_is_read_but_not_prompted_until_idle() {
    let (mut store, mut herdr, cfg) = world();
    herdr.set_status(CAP, AgentStatus::Working);
    herdr.set_screen(CAP, &screen(100));
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    assert!(herdr.prompts_to(LANE).is_empty());
    tick(&mut store, &mut herdr, &cfg, T0 + 3610).unwrap();
    assert!(herdr.prompts_to(CAP).is_empty());
    herdr.set_status(CAP, AgentStatus::Idle);
    tick(&mut store, &mut herdr, &cfg, T0 + 3620).unwrap();
    assert_eq!(herdr.prompts_to(CAP).len(), 1);
}

#[test]
fn an_uncertain_wake_is_not_resent_after_recovery() {
    let (mut store, mut herdr, cfg) = world();
    herdr.set_screen(CAP, &screen(100));
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    herdr.inject_next = Some(gear_herdr::fake::Inject::Uncertain);
    tick(&mut store, &mut herdr, &cfg, T0 + 3610).unwrap();
    gear_engine::ops::recover_on_start(&mut store, T0 + 3611).unwrap();
    tick(&mut store, &mut herdr, &cfg.clone(), T0 + 3620).unwrap();
    assert_eq!(herdr.prompts_to(CAP).len(), 1);
    view(&mut store, T0 + 3621);
    tick(&mut store, &mut herdr, &cfg, T0 + 3630).unwrap();
    assert_eq!(herdr.prompts_to(LANE).len(), 1);
}

#[test]
fn a_previously_queued_assignment_waits_for_the_captain_to_return() {
    let (mut store, mut herdr, cfg) = world();
    store
        .write(|tx| {
            let lane = tx.get_seat("p:heat")?.unwrap();
            gear_engine::dispatch::intend_dispatch(
                tx,
                "p",
                &[(
                    lane,
                    gear_engine::dispatch::Action::Assign { task: "T-1".into() },
                )],
                T0 + 1,
            )?;
            Ok(())
        })
        .unwrap();
    herdr.set_screen(CAP, &screen(100));
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    assert!(herdr.prompts_to(LANE).is_empty());
    tick(&mut store, &mut herdr, &cfg, T0 + 3610).unwrap();
    view(&mut store, T0 + 3611);
    tick(&mut store, &mut herdr, &cfg, T0 + 3620).unwrap();
    assert_eq!(herdr.prompts_to(LANE).len(), 1);
}

#[test]
fn the_bar_reset_time_wins_over_the_fallback_window() {
    let (mut store, mut herdr, cfg) = world();
    let reset = T0 + 600;
    let stamp = reset as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    assert!(!unsafe { libc::localtime_r(&stamp, local.as_mut_ptr()) }.is_null());
    let local = unsafe { local.assume_init() };
    let bar = screen(100).replace(
        "5h 100%",
        &format!("5h 100% ↻{:02}:{:02}", local.tm_hour, local.tm_min),
    );
    herdr.set_screen(CAP, &bar);
    tick(&mut store, &mut herdr, &cfg, T0 + 10).unwrap();
    assert_eq!(
        store
            .read(|tx| tx.captain_quota("p"))
            .unwrap()
            .unwrap()
            .reset_at,
        reset
    );
    tick(&mut store, &mut herdr, &cfg, reset).unwrap();
    assert_eq!(herdr.prompts_to(CAP).len(), 1);
}
