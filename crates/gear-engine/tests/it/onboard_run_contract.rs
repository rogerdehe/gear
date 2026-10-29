//! Contract for onboard_queued (T-62): the walking-skeleton onboarding of lanes.
//! Needs T-57 (admin init), T-58 (plan_onboard), T-59 (seat transitions).

use gear_app::{admin, Env};
use gear_engine::onboard::onboard_queued;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::builtin;
use gear_model::{SeatRole, SeatState};
use gear_store::{BindingStatus, SeatRow, Store};

const T0: i64 = 1_800_000_000;
const CONFIG: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#;

fn cfg() -> EngineCfg {
    let mut c = EngineCfg { db_path: std::env::temp_dir().join(format!("gear-engine-tests-{}/gear.db", std::process::id())).to_string_lossy().into_owned(), ..Default::default() };
    c.profiles.insert("opencode".into(), builtin("opencode").unwrap());
    c
}

#[test]
fn queued_lane_gets_worktree_env_and_agent() {
    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(&mut s, &Env { ctx: None, pane_id: None, admin: true }, CONFIG,Some(crate::skill_home()), false, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    let active = onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap();
    assert_eq!(active, vec!["toy:heat".to_string()]);
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!((seat.state, seat.generation), (SeatState::Active, 1));
    let b = s.read(|tx| tx.get_binding("toy:heat", 1)).unwrap().unwrap();
    assert_eq!(b.status, BindingStatus::Active);
    assert!(herdr.worktree_branches().contains(&"toy/heat"));
    assert!(herdr.calls.iter().any(|c| matches!(c, Call::PaneRun { command, .. }
        if command == &format!("export GEAR_CTX=toy:heat:1; export GEAR_DB={}", cfg().db_path))), "{:?}", herdr.calls);
    assert!(herdr.calls.iter().any(|c| matches!(c, Call::AgentStart { name, kind, args, .. }
        if name == "toy-heat" && kind == "opencode" && args == &vec!["--auto".to_string()])), "{:?}", herdr.calls);
    // the captain is bound at init (T-63), not onboarded by the engine
    assert_eq!(s.read(|tx| tx.get_seat("toy:captain")).unwrap().unwrap().state, SeatState::Active);
    // nothing to do on the next run
    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 2).unwrap().is_empty());
}

#[test]
fn a_failed_step_parks_the_seat_in_needs_check() {
    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(&mut s, &Env { ctx: None, pane_id: None, admin: true }, CONFIG,Some(crate::skill_home()), false, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse { code: "worktree_exists".into() });
    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap().is_empty());
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::NeedsCheck);
    assert!(seat.state_reason.unwrap_or_default().contains("worktree"));
}

/// T-60: a lane that cannot start on the chain's model (no `[models.switch]`
/// labels for it) stops — the captain is rung, once.
#[test]
fn a_lane_that_cannot_start_on_its_model_rings_the_captain() {
    let mut s = Store::open_in_memory().unwrap();
    let with_models = CONFIG.to_owned()
        + "\n[models]\nlanes = [\"minimax-cn/MiniMax-M3.1-Flash\", \"deepseek/deepseek-flash\"]\n";
    admin::init_with_home(&mut s, &Env { ctx: None, pane_id: None, admin: true }, &with_models,Some(crate::skill_home()), false, T0).unwrap();
    let mut herdr = FakeHerdr::new();

    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap().is_empty());
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::NeedsCheck, "{:?}", seat.state_reason);

    // The captain is rung: one urgent item naming the seat and the reason.
    let open = s.read(|tx| tx.actionable_for("toy", "toy:captain", T0 + 2)).unwrap();
    assert_eq!(open.len(), 1, "one urgent item: {open:?}");
    assert!(open[0].urgent, "{open:?}");
    assert!(open[0].body.contains("toy:heat"), "it names the seat: {open:?}");
    assert!(open[0].body.contains("models.switch"), "it names the reason: {open:?}");

    // The seat is never retried, so the captain is not rung again.
    assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 3).unwrap().is_empty());
    let open = s.read(|tx| tx.actionable_for("toy", "toy:captain", T0 + 4)).unwrap();
    assert_eq!(open.len(), 1, "the failure was told once: {open:?}");
}

#[test]
fn a_startup_switch_timeout_or_removed_labels_rings_once() {
    for remove_labels in [false, true] {
        let mut s = Store::open_in_memory().unwrap();
        let without_labels = CONFIG.to_owned()
            + "\n[models]\nlanes = [\"deepseek/deepseek-flash\", \"minimax-cn/MiniMax-M3.1-Flash\"]\n";
        let with_labels = without_labels.clone()
            + "\n[models.switch.\"deepseek/deepseek-flash\"]\npicker = \"DeepSeek V4.1 Flash\"\nbar = \"DeepSeek V4.1 Flash\"\n";
        let env = Env { ctx: None, pane_id: None, admin: true };
        admin::init_with_home(&mut s, &env, &with_labels,Some(crate::skill_home()), false, T0).unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.start_screen = Some("Build auto · wrong-model · high".into());
        assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap().is_empty());
        assert!(gear_engine::switch::pending(&mut s, "toy:heat").unwrap().is_some());
        if remove_labels {
            admin::init_with_home(&mut s, &env, &without_labels,Some(crate::skill_home()), false, T0 + 2).unwrap();
        }
        for tick in 0..30 {
            assert!(onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 3 + tick).unwrap().is_empty());
        }
        let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
        assert_eq!(seat.state, SeatState::NeedsCheck);
        assert!(gear_engine::switch::pending(&mut s, "toy:heat").unwrap().is_none());
        let open = s.read(|tx| tx.actionable_for("toy", "toy:captain", T0 + 40)).unwrap();
        assert_eq!(open.len(), 1, "one alert, removed labels={remove_labels}: {open:?}");
        let reason = if remove_labels { "no [models.switch] labels any more" } else { "did not confirm twice" };
        assert!(open[0].urgent && open[0].body.contains("toy:heat") && open[0].body.contains(reason));
    }
}

/// T-6: a lane that a config change adds is a queued seat, and the engine
/// onboards it with the **latest** config (the repo it names now).
#[test]
fn a_lane_added_by_a_config_change_is_onboarded_with_the_latest_config() {
    let mut s = Store::open_in_memory().unwrap();
    let admin_env = Env { ctx: None, pane_id: None, admin: true };
    admin::init_with_home(&mut s, &admin_env, CONFIG,Some(crate::skill_home()), false, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap(),
        vec!["toy:heat".to_string()]
    );

    // The config changes: a second lane, another repo, a bigger quota.
    let changed = CONFIG
        .replace("lanes = 1", "lanes = 2")
        .replace(r#"repo = "/repo""#, r#"repo = "/repo2""#)
        + "\n[[lanes]]\nname = \"pricing\"\nduty = \"prices\"\nowns = [\"lib/\"]\nkind = \"opencode\"\nprofile = \"opencode\"\n";
    admin::init_with_home(&mut s, &admin_env, &changed,Some(crate::skill_home()), false, T0 + 2).unwrap();

    // The new lane is onboarded; the lane that was there is not touched.
    let active = onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 3).unwrap();
    assert_eq!(active, vec!["toy:pricing".to_string()]);
    assert!(herdr.calls.iter().any(|c| matches!(c, Call::WorktreeCreate { repo, branch, .. }
        if repo == "/repo2" && branch == "toy/pricing")), "{:?}", herdr.calls);
    let seat = s.read(|tx| tx.get_seat("toy:pricing")).unwrap().unwrap();
    assert_eq!((seat.state, seat.generation), (SeatState::Active, 1));
}

/// Rework of T-6 (review P1): the config and the seat roster come from one
/// snapshot, and a queued lane that the config does not name (the state a
/// re-init landing between two reads used to produce) is **skipped**, never
/// parked — the next tick reads the newer config and onboards it.
#[test]
fn a_lane_missing_from_the_config_snapshot_is_skipped_not_parked() {
    let mut s = Store::open_in_memory().unwrap();
    let admin_env = Env { ctx: None, pane_id: None, admin: true };
    admin::init_with_home(&mut s, &admin_env, CONFIG,Some(crate::skill_home()), false, T0).unwrap();
    // The seat a re-init would have committed just before the roster read.
    s.write(|tx| {
        tx.insert_seat(&SeatRow {
            id: "toy:pricing".into(),
            project: "toy".into(),
            role: SeatRole::Lane,
            name: "pricing".into(),
            profile: "opencode".into(),
            generation: 0,
            state: SeatState::Queued,
            state_since: T0,
            state_reason: Some("registered".into()),
            task_id: None,
        })
    })
    .unwrap();

    let mut herdr = FakeHerdr::new();
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap(),
        vec!["toy:heat".to_string()],
        "only the lane the config names is onboarded"
    );
    let pricing = s.read(|tx| tx.get_seat("toy:pricing")).unwrap().unwrap();
    assert_eq!(
        pricing.state,
        SeatState::Queued,
        "not parked for a config that may be about to arrive"
    );

    // The config change lands; the next tick onboards the lane.
    let changed = CONFIG.replace("lanes = 1", "lanes = 2")
        + "\n[[lanes]]\nname = \"pricing\"\nduty = \"prices\"\nowns = [\"lib/\"]\nkind = \"opencode\"\nprofile = \"opencode\"\n";
    admin::init_with_home(&mut s, &admin_env, &changed,Some(crate::skill_home()), false, T0 + 2).unwrap();
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 3).unwrap(),
        vec!["toy:pricing".to_string()]
    );
    let pricing = s.read(|tx| tx.get_seat("toy:pricing")).unwrap().unwrap();
    assert_eq!((pricing.state, pricing.generation), (SeatState::Active, 1));
}

/// T-32: a grok lane's worktree is trusted before its agent starts (one entry,
/// written once), its start carries the unattended flag and the model, and a
/// claude lane's start carries `--model`.
#[test]
fn grok_is_trusted_and_started_with_its_model_and_claude_carries_its_model() {
    const BOTH: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "grok1"
duty = "everything"
owns = ["src/"]
kind = "grok"
profile = "grok"

[[lanes]]
name = "claude1"
duty = "everything"
owns = ["lib/"]
kind = "claude"
profile = "claude"

[models]
lanes = ["grok-4.7"]
on_quota = { lanes = "wait", review = "wait" }

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#;
    // The grok trust store lives under HOME: give it a scratch one.
    let home = std::env::temp_dir().join(format!(
        "gear-grok-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&home).unwrap();
    // HOME is process-wide: no other test may onboard a grok lane (or set
    // HOME) in this window.
    let _home = crate::HOME_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let previous = std::env::var("HOME").ok();
    std::env::set_var("HOME", &home);

    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(&mut s, &Env { ctx: None, pane_id: None, admin: true }, BOTH,Some(crate::skill_home()), false, T0).unwrap();
    let mut herdr = FakeHerdr::new();
    let mut c = cfg();
    c.profiles.insert("grok".into(), builtin("grok").unwrap());
    c.profiles.insert("claude".into(), builtin("claude").unwrap());
    let active = onboard_queued(&mut s, &mut herdr, &c, T0 + 1).unwrap();
    assert_eq!(active.len(), 2, "{active:?}");

    // The grok lane: the unattended flag and the model, as args.
    assert!(
        herdr.calls.iter().any(|call| matches!(call, Call::AgentStart { name, kind, args, .. }
            if name == "toy-grok1" && kind == "grok"
                && args == &vec!["--always-approve".to_string(), "-m".to_string(), "grok-4.7".to_string()])),
        "{:?}",
        herdr.calls
    );
    // Its worktree was trusted before the start: one entry, written once.
    let trust = std::fs::read_to_string(home.join(".grok/trusted_folders.toml")).unwrap();
    assert!(trust.contains("[folders.\"/fake-worktrees/toy-grok1\"]"), "{trust}");
    assert!(trust.contains("trusted = true"), "{trust}");
    assert_eq!(trust.matches("[folders.").count(), 1, "one entry: {trust}");

    // The claude lane: its model as `--model`.
    assert!(
        herdr.calls.iter().any(|call| matches!(call, Call::AgentStart { name, kind, args, .. }
            if name == "toy-claude1" && kind == "claude"
                && args.contains(&"--model".to_string())
                && args.contains(&"grok-4.7".to_string()))),
        "{:?}",
        herdr.calls
    );

    match previous {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&home);
}

/// T-133 (review F1): a held lane whose agent exited — its binding still
/// `Active` — is released to the queue from the plan, onboarded again and given
/// its next task, all by the engine.
#[test]
fn a_released_lane_whose_agent_is_gone_is_onboarded_and_given_its_task() {
    use gear_app::identity::Fingerprint;
    use gear_herdr::HerdrRead as _;
    use gear_model::GearCtx;

    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(
        &mut s,
        &Env { ctx: None, pane_id: None, admin: true },
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    let mut herdr = FakeHerdr::new();
    // A delivered prompt starts the lane's turn, as a real agent does (T-404:
    // an idle agent with no delivery in its tail is a lost delivery).
    herdr.prompt_starts_turn = true;
    let cfg = cfg();

    let cap = Fingerprint { pane_id: "w1:p1".into(), kind: Some("claude".into()), session_ref: None };
    let env = Env {
        ctx: Some(GearCtx { project: "toy".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some("w1:p1".into()),
        admin: false,
    };
    let apply = |s: &mut Store, herdr: &mut FakeHerdr, text: &str, rid: &str, now: i64| {
        let draft = gear_app::draft::parse(text).unwrap();
        let agents = herdr.agents().unwrap();
        gear_app::plan::apply(s, &env, Some(&cap), Some(&agents), &draft, text, rid, now)
    };

    // 1. the engine onboards the lane: an agent runs in its pane
    gear_engine::tick(&mut s, &mut herdr, &cfg, T0 + 1).unwrap();
    let binding = s.read(|tx| tx.get_binding("toy:heat", 1)).unwrap().unwrap();
    assert_eq!(binding.status, BindingStatus::Active);

    // 2. one task waits for heat; the captain holds the lane, so nothing is
    //    assigned
    let r = apply(
        &mut s,
        &mut herdr,
        "<!-- gear-plan project=toy base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: Fix the login crash\naccept: repro passes\nestimate: 30\ndeps: -\n# Staff\n- hold heat reason=the machine is recovering\n",
        "v1",
        T0 + 2,
    )
    .unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    gear_engine::tick(&mut s, &mut herdr, &cfg, T0 + 100).unwrap();
    assert_eq!(herdr.prompts_to(&binding.pane_id).len(), 0, "a held lane is assigned nothing");

    // 3. its agent exits; the store keeps the binding Active
    herdr.exit_agent(&binding.pane_id);
    let binding_now = s.read(|tx| tx.get_binding("toy:heat", 1)).unwrap().unwrap();
    assert_eq!(binding_now.status, BindingStatus::Active, "an exited agent keeps its binding");

    // 4. release: no agent in the pane, so the lane goes back to the queue
    apply(&mut s, &mut herdr, "<!-- gear-plan project=toy base=1 -->\n# Staff\n- release heat\n", "v2", T0 + 200).unwrap();
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Queued, "its agent is gone: {:?}", seat.state_reason);

    // 5. the engine onboards it again and delivers its task
    gear_engine::tick(&mut s, &mut herdr, &cfg, T0 + 300).unwrap();
    gear_engine::tick(&mut s, &mut herdr, &cfg, T0 + 400).unwrap();
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::Active);
    let binding = s.read(|tx| tx.get_binding("toy:heat", seat.generation)).unwrap().unwrap();
    let prompts = herdr.prompts_to(&binding.pane_id);
    assert_eq!(prompts.len(), 1, "its next task is delivered: {prompts:?}");
    assert!(prompts[0].starts_with("New task T-1 (contract v1)"), "{}", prompts[0]);
}

/// T-285 (T-134 review F3): a seat retried while an unnamed leftover agent
/// still runs in its pane names that agent and asks it to exit in the same
/// tick. Before, the exit command went out only when the pane already had a
/// live name, so an unnamed leftover was never asked and the seat parked after
/// three tries.
#[test]
fn an_unnamed_leftover_agent_is_named_and_asked_to_exit_in_the_same_tick() {
    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(
        &mut s,
        &Env { ctx: None, pane_id: None, admin: true },
        CONFIG,
        Some(crate::skill_home()),
        false,
        T0,
    )
    .unwrap();
    let mut herdr = FakeHerdr::new();
    // The leftover is a codex agent (its profile exits by the typed `/quit`).
    let mut cfg = cfg();
    cfg.profiles.insert("codex".into(), builtin("codex").unwrap());

    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg, T0 + 1).unwrap(),
        vec!["toy:heat".to_string()]
    );
    let pane = s.read(|tx| tx.get_binding("toy:heat", 1)).unwrap().unwrap().pane_id;

    // The fixture is re-run: the seat is queued again and a leftover codex
    // agent is in its pane, its name cleared by a Herdr restart.
    herdr.add_agent(&pane, "codex", gear_herdr::AgentStatus::Idle);
    s.write(|tx| tx.set_seat_state("toy:heat", SeatState::Queued, Some("fixture re-run"), T0 + 2))
        .unwrap();

    assert!(onboard_queued(&mut s, &mut herdr, &cfg, T0 + 3).unwrap().is_empty());

    // Named first, then prompted at that name: a prompt to an unnamed agent is
    // refused, and the command is never typed into the pane.
    assert_eq!(herdr.agent_name(&pane), Some("toy-heat"), "the leftover is named");
    let renamed_at = herdr
        .calls
        .iter()
        .position(|call| matches!(call, Call::Rename { pane: p, .. } if p == &pane))
        .unwrap_or_else(|| panic!("no rename: {:?}", herdr.calls));
    let asked_at = herdr
        .calls
        .iter()
        .position(|call| matches!(call, Call::Prompt { pane: p, .. } if p == &pane))
        .unwrap_or_else(|| panic!("the leftover was never asked: {:?}", herdr.calls));
    assert!(renamed_at < asked_at, "named before it is prompted: {:?}", herdr.calls);
    assert_eq!(herdr.prompts_to(&pane), vec!["/quit"], "{:?}", herdr.calls);
    assert_eq!(
        herdr.prompt_targets(),
        vec!["toy-heat"],
        "the command goes to the agent's name, never the pane id"
    );
    // That was one try: the seat still waits for the shell.
    assert_eq!(
        s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap().state,
        SeatState::Queued
    );

    // The agent exits: the pane is a shell and the seat starts.
    herdr.exit_agent(&pane);
    assert_eq!(
        onboard_queued(&mut s, &mut herdr, &cfg, T0 + 4).unwrap(),
        vec!["toy:heat".to_string()]
    );
    assert_eq!(
        s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap().state,
        SeatState::Active
    );
}
