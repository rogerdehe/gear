//! Tests for `gear_app::home` (T-17).
//!
//! The doc comments of `build` and `record_view` are the spec: `build` only
//! reads, `record_view` writes what was shown.
//!
//! The rows are written straight into the store: the home page only reads,
//! and the threads of the batch are easier to build that way.

use gear_app::draft::parse;
use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{
    AttemptState, ErrorCode, GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole, TaskState, Ts,
};
use gear_store::{testing, AttemptRow, NewMessage, NewOp, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn seat_env(seat: &str, pane: &str) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: seat.into(),
            generation: 1,
        }),
        pane_id: Some(pane.into()),
        admin: false,
    }
}

fn fp(pane: &str, kind: &str) -> Fingerprint {
    Fingerprint {
        pane_id: pane.into(),
        kind: Some(kind.into()),
        session_ref: None,
    }
}

/// Write one message into the store and return its id.
fn insert(
    s: &mut Store,
    thread: &str,
    sender: &str,
    recipient: &str,
    kind: MsgKind,
    state: MsgState,
    body: &str,
    created: Ts,
) -> String {
    s.write(|tx| {
        let row = NewMessage {
            meta: None,
            project: "p".into(),
            thread: thread.into(),
            task: None,
            attempt: None,
            sender: sender.into(),
            sender_gen: 1,
            recipient: recipient.into(),
            kind,
            urgent: false,
            body: body.into(),
            reply_to: None,
            needs_ack: false,
            state,
            created,
        };
        Ok(tx.insert_message(&row)?.id)
    })
    .unwrap()
}

/// A lane asks the captain.
fn ask(s: &mut Store, body: &str, created: Ts) -> String {
    insert(
        s,
        "heat:1",
        "p:heat",
        "p:captain",
        MsgKind::Question,
        MsgState::Open,
        body,
        created,
    )
}

/// The captain answers a lane.
fn answer(s: &mut Store, body: &str, created: Ts) -> String {
    insert(
        s,
        "heat:1",
        "p:captain",
        "p:heat",
        MsgKind::Reply,
        MsgState::Info,
        body,
        created,
    )
}

/// Apply a plan whose `# Decisions` carries the one given line.
fn apply_decision(s: &mut Store, decision: &str, rid: &str) {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Decisions\n- {decision}\n");
    apply(
        s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        None,
        &parse(&text).unwrap(),
        &text,
        rid,
        NOW,
    )
    .unwrap();
}

#[test]
fn captain_sees_batch_with_ids_and_bodies() {
    let mut s = world();
    ask(&mut s, "Schema v1 or v2?", NOW);

    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();

    assert!(
        home.text.contains("p:captain"),
        "identity line: {}",
        home.text
    );
    assert!(
        home.text.contains("generation 1"),
        "identity line: {}",
        home.text
    );
    assert!(home.text.contains("M-1"), "the message id: {}", home.text);
    assert!(
        home.text.contains("Schema v1 or v2?"),
        "the body: {}",
        home.text
    );
    assert!(home.text.contains("heat:1"), "the thread id: {}", home.text);
    assert!(
        home.text.contains("next:"),
        "a suggested command: {}",
        home.text
    );
}

#[test]
fn more_count_shown() {
    let mut s = world();
    for i in 0..5 {
        ask(&mut s, &format!("question {i}"), NOW + i);
    }

    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 10,
    )
    .unwrap();

    assert!(home.text.contains("M-1"), "the first message is shown");
    assert!(home.text.contains("M-3"), "three messages are shown");
    assert!(
        !home.text.contains("M-4"),
        "the fourth message is only counted"
    );
    assert!(
        home.text.contains("+2 more"),
        "the rest is counted: {}",
        home.text
    );
}

#[test]
fn lane_sees_unshown_reply() {
    let mut s = world();
    answer(&mut s, "Use v2", NOW);

    let home = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 2,
    )
    .unwrap();

    assert!(home.text.contains("p:heat"), "identity line: {}", home.text);
    assert!(home.text.contains("Use v2"), "the answer: {}", home.text);
    assert!(home.text.contains("M-1"), "the answer id: {}", home.text);
}

#[test]
fn recorded_view_hides_it_next_time() {
    let mut s = world();
    answer(&mut s, "Use v2", NOW);

    let first = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 2,
    )
    .unwrap();
    assert!(first.text.contains("Use v2"));
    home::record_view(&mut s, &first.view).unwrap();

    let second = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 3,
    )
    .unwrap();
    assert!(
        !second.text.contains("Use v2"),
        "the answer was shown already: {}",
        second.text
    );
}

#[test]
fn message_in_the_same_second_as_the_view_is_not_lost() {
    let mut s = world();
    // A view of the empty page, recorded at NOW.
    let empty = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW,
    )
    .unwrap();
    home::record_view(&mut s, &empty.view).unwrap();

    // The answer arrives in the same second as that view.
    answer(&mut s, "Use v2", NOW);

    let home = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 1,
    )
    .unwrap();
    assert!(
        home.text.contains("Use v2"),
        "the answer must not be lost: {}",
        home.text
    );
    assert_eq!(home.view.shown_msgs, vec!["M-1".to_string()]);
}

#[test]
fn instruction_to_the_captain_is_printed_once() {
    let mut s = world();
    insert(
        &mut s,
        "T-1:1",
        "p:heat",
        "p:captain",
        MsgKind::Instruction,
        MsgState::Open,
        "do the thing",
        NOW,
    );

    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();

    assert_eq!(
        home.text.matches("do the thing").count(),
        1,
        "printed once: {}",
        home.text
    );
    assert_eq!(
        home.view.shown_msgs,
        vec!["M-1".to_string()],
        "the id is listed once"
    );
}

#[test]
fn view_lists_exactly_what_was_shown() {
    let mut s = world();
    ask(&mut s, "Schema v1 or v2?", NOW);
    ask(&mut s, "And the second one?", NOW + 1);
    answer(&mut s, "Use v2", NOW + 2);

    let captain = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 3,
    )
    .unwrap();
    assert_eq!(
        captain.view.shown_msgs,
        vec!["M-1".to_string(), "M-2".to_string()]
    );
    assert_eq!(captain.view.seat, "p:captain");
    assert_eq!(captain.view.generation, 1);
    assert_eq!(captain.view.project, "p");
    assert_eq!(captain.view.ts, NOW + 3);
    assert!(
        captain.view.shown_ops.is_empty(),
        "no operation is shown in slice 1"
    );

    let lane = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 4,
    )
    .unwrap();
    assert_eq!(lane.view.shown_msgs, vec!["M-3".to_string()]);
}

#[test]
fn build_is_read_only() {
    let mut s = world();
    ask(&mut s, "Schema v1 or v2?", NOW);

    let before = s
        .read(|tx| tx.actionable_for("p", "p:captain", NOW + 1))
        .unwrap();
    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    let after = s
        .read(|tx| tx.actionable_for("p", "p:captain", NOW + 1))
        .unwrap();
    assert_eq!(before, after, "building the home page changes no message");

    let views = s.read(|tx| tx.views_since("p:captain", 1, 0)).unwrap();
    assert!(views.is_empty(), "building the home page records no view");
    assert!(
        !home.view.shown_msgs.is_empty(),
        "the view still names what was shown"
    );
}

#[test]
fn record_view_persists() {
    let mut s = world();
    ask(&mut s, "Schema v1 or v2?", NOW);
    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();

    home::record_view(&mut s, &home.view).unwrap();

    let views = s.read(|tx| tx.views_since("p:captain", 1, 0)).unwrap();
    assert_eq!(views.len(), 1, "one view row");
    assert_eq!(views[0], home.view);
}

#[test]
fn the_captain_home_starts_with_the_snapshot_before_the_inbox() {
    let mut s = world();
    // A second lane and four tasks: heat is on T-1, runtime has two queued.
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "runtime", SeatRole::Lane, "w1:p3", "opencode", NOW)?;
        for (id, rank, lane, state) in [
            ("T-1", 1, "heat", TaskState::Running),
            ("T-2", 2, "heat", TaskState::Ready),
            ("T-3", 3, "runtime", TaskState::Ready),
            ("T-4", 4, "runtime", TaskState::Ready),
        ] {
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
                deps: Vec::new(),
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: None,
                updated: NOW,
            })?;
        }
        // The attempt behind T-1's Running state: the snapshot names the
        // task's stage in the graph's words (T-347).
        tx.insert_attempt(&AttemptRow {
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
            started: NOW,
            updated: NOW,
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
        })?;
        Ok(())
    })
    .unwrap();
    ask(&mut s, "Schema v1 or v2?", NOW);

    let mut herdr = gear_herdr::fake::FakeHerdr::new();
    herdr.add_agent(LANE_PANE, "opencode", gear_herdr::AgentStatus::Working);
    herdr.add_agent("w1:p3", "opencode", gear_herdr::AgentStatus::Idle);
    let agents = gear_herdr::HerdrRead::agents(&mut herdr).unwrap();
    let home = home::build_with_agents(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
        Some(&agents),
    )
    .unwrap();

    // The snapshot: totals, both lanes with their current task and queue,
    // the deliveries in flight and the temp slots.
    assert!(
        home.text
            .contains("SNAPSHOT 4 tasks · 0 done · 1 in progress · 0 waiting"),
        "{}",
        home.text
    );
    assert!(
        home.text
            .contains("heat · agent working · T-1 working · 1 queued"),
        "{}",
        home.text
    );
    assert!(
        home.text
            .contains("runtime · agent idle · no current task · 2 queued"),
        "{}",
        home.text
    );
    assert!(
        home.text.contains(
            "deliveries not confirmed: 0 · temps 0 running · 0 starting · 0 blocked (0 of 2)"
        ),
        "{}",
        home.text
    );

    // And it is before the inbox.
    let snapshot = home.text.find("SNAPSHOT").expect("the snapshot");
    let inbox = home.text.find("INBOX").expect("the inbox");
    assert!(snapshot < inbox, "the snapshot comes first: {}", home.text);
}

#[test]
fn a_lane_with_nothing_queued_is_still_in_the_snapshot() {
    let mut s = world();
    // A registered lane that has no tasks at all: the captain has to see it
    // to refill it (the review of T-9).
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "runtime", SeatRole::Lane, "w1:p3", "opencode", NOW)
    })
    .unwrap();

    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();

    assert!(
        home.text
            .contains("runtime · agent unknown · no current task · 0 queued"),
        "{}",
        home.text
    );
    assert!(
        home.text
            .contains("heat · agent unknown · no current task · 0 queued"),
        "{}",
        home.text
    );
}

#[test]
fn the_lane_home_marks_a_new_session_recovery() {
    let mut s = world();
    s.write(|tx| {
        tx.insert_task(&TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: "T-1".into(),
            rank: 1,
            state: TaskState::Running,
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
            updated: NOW,
        })?;
        tx.insert_attempt(&AttemptRow {
            session_ref: None,
            session_cli: None,
            session_action: None,
            sent_at: None,
            project: "p".into(),
            task: "T-1".into(),
            n: 2,
            seat: "p:heat".into(),
            generation: 1,
            contract_version: 1,
            adopted_version: 1,
            pending_update: None,
            state: AttemptState::Delivered,
            candidate: None,
            report: None,
            safe_point: None,
            started: NOW,
            updated: NOW,
        })?;
        tx.set_session_recovery("p", "T-1", 2, "codex", "session-x")
    })
    .unwrap();

    let home = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 2,
    )
    .unwrap();
    assert!(
        home.text.contains("round: new-session recovery"),
        "the round is marked: {}",
        home.text
    );
    assert!(home.text.contains("task T-1 attempt 2"), "{}", home.text);
}

#[test]
fn the_lane_home_has_no_snapshot() {
    let mut s = world();
    let home = home::build(
        &mut s,
        &seat_env("heat", LANE_PANE),
        Some(&fp(LANE_PANE, "opencode")),
        NOW + 1,
    )
    .unwrap();

    assert!(
        !home.text.contains("SNAPSHOT"),
        "the snapshot is the captain's: {}",
        home.text
    );
}

#[test]
fn unregistered_caller_rejected() {
    let mut s = world();
    let env = Env {
        ctx: None,
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    };
    let err = home::build(&mut s, &env, None, NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotRegistered);

    let env = Env {
        ctx: None,
        pane_id: None,
        admin: true,
    };
    let err = home::build(&mut s, &env, None, NOW).unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::NotRegistered,
        "the admin flag is not an identity"
    );
}

#[test]
fn the_snapshot_omits_retired_lanes() {
    let mut s = world();
    s.write(|tx| tx.set_seat_state("p:heat", gear_model::SeatState::Retired, None, NOW))
        .unwrap();
    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    assert!(
        !home
            .text
            .lines()
            .any(|line| line.trim_start().starts_with("heat ·")),
        "{}",
        home.text
    );
}

/// The snapshot line of the named lane.
fn lane_line(text: &str, name: &str) -> String {
    text.lines()
        .find(|line| line.trim_start().starts_with(&format!("{name} ·")))
        .unwrap_or_else(|| panic!("{name} is in the snapshot: {text}"))
        .to_string()
}

/// T-260: a lane the captain's `- tail:` silences says so in the snapshot —
/// a silenced lane must not read as a quiet one.
#[test]
fn the_snapshot_marks_a_tailed_lane() {
    let mut s = world();
    s.write(|tx| {
        testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p3", "opencode", NOW)?;
        testing::seed_active_seat(tx, "p", "mast", SeatRole::Lane, "w1:p4", "opencode", NOW)
    })
    .unwrap();
    apply_decision(&mut s, "tail: hull", "r1");

    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    assert!(
        lane_line(&home.text, "hull").ends_with(" · tail: warnings off"),
        "{}",
        home.text
    );
    // The lane the tail does not name reads as before.
    assert_eq!(
        lane_line(&home.text, "mast"),
        "  mast · agent unknown · no current task · 0 queued"
    );

    // The tail is cleared: the mark goes.
    apply_decision(&mut s, "tail: -", "r2");
    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    assert_eq!(
        lane_line(&home.text, "hull"),
        "  hull · agent unknown · no current task · 0 queued",
        "{}",
        home.text
    );
    assert!(!home.text.contains("tail: warnings off"), "{}", home.text);
}

/// One unconfirmed delivery of the lane `heat`, created at `created` (T-271).
fn deliver(s: &mut Store, created: Ts) {
    s.write(|tx| {
        let op = tx.insert_op(&NewOp {
            project: "p".into(),
            seat: "p:heat".into(),
            generation: 1,
            kind: OpKind::Deliver,
            task: Some("T-1".into()),
            attempt: Some(1),
            ref_msg: None,
            plan_version: None,
            payload: serde_json::json!({}),
            created,
        })?;
        tx.set_op_state("p", &op.id, OpState::Submitted, None, created + 1)?;
        Ok(())
    })
    .unwrap();
}

/// T-271: the snapshot says which deliveries are not confirmed — three lines,
/// the rest as a count.
#[test]
fn the_snapshot_names_the_unconfirmed_deliveries() {
    let mut s = world();
    // Four deliveries, the oldest 4 minutes back.
    for age in [240, 180, 120, 60] {
        deliver(&mut s, NOW - age);
    }
    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    let lines: Vec<&str> = home.text.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("deliveries not confirmed: 4"))
        .unwrap_or_else(|| panic!("the deliveries line: {}", home.text));
    assert_eq!(lines[at + 1], "    O-1 heat T-1 4m", "{}", home.text);
    assert_eq!(lines[at + 2], "    O-2 heat T-1 3m", "{}", home.text);
    assert_eq!(lines[at + 3], "    O-3 heat T-1 2m", "{}", home.text);
    assert_eq!(lines[at + 4], "    +1 more", "{}", home.text);
}

/// T-271: no delivery, no extra line.
#[test]
fn the_snapshot_names_no_delivery_when_there_is_none() {
    let mut s = world();
    let home = home::build(
        &mut s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    assert!(
        home.text.contains("deliveries not confirmed: 0"),
        "{}",
        home.text
    );
    assert!(
        !home
            .text
            .lines()
            .any(|line| line.trim_start().starts_with("O-")),
        "{}",
        home.text
    );
}

// ------------------------------------------------------- config drift (T-321)

/// The canonical form of a path, as the CLI records it in the meta row.
fn canonical_path(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// T-321: `p` runs on `applied`, registered from the file at `path` — the
/// project's config text and its meta path row, as a real `init` leaves them.
/// Returns the path the page must name (canonical, as the CLI writes it).
fn applied_from(s: &mut Store, path: &std::path::Path, applied: &str) -> String {
    let canonical = canonical_path(path);
    s.write(|tx| {
        let mut row = tx.get_project("p")?.expect("the project");
        row.config_toml = applied.to_string();
        tx.update_project(&row)?;
        tx.set_meta(&gear_app::admin::config_path_key("p"), &canonical)
    })
    .unwrap();
    canonical
}

/// The snapshot's lines, for the line-after-the-totals assertions.
fn snapshot_lines(s: &mut Store) -> Vec<String> {
    let home = home::build(
        s,
        &seat_env("captain", CAP_PANE),
        Some(&fp(CAP_PANE, "codex")),
        NOW + 1,
    )
    .unwrap();
    home.text.lines().map(str::to_string).collect()
}

/// The line right after the snapshot's totals line.
fn second_snapshot_line(s: &mut Store) -> String {
    let lines = snapshot_lines(s);
    let at = lines
        .iter()
        .position(|line| line.starts_with("SNAPSHOT "))
        .unwrap_or_else(|| panic!("the snapshot: {lines:?}"));
    lines[at + 1].clone()
}

/// T-321: a config file whose text is the applied config shows no drift line.
#[test]
fn a_config_file_equal_to_the_applied_config_shows_no_drift() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.toml");
    let text = "[project]\nid = \"p\"\n";
    std::fs::write(&path, text).unwrap();
    let mut s = world();
    applied_from(&mut s, &path, text);

    let lines = snapshot_lines(&mut s);
    let at = lines
        .iter()
        .position(|line| line.starts_with("SNAPSHOT "))
        .expect("the snapshot");
    // The second line is the first lane, not a warning.
    assert!(lines[at + 1].starts_with("  heat · "), "{lines:?}");
    assert!(
        !lines.iter().any(|line| line.starts_with("! config")),
        "{lines:?}"
    );
}

/// T-321: a project whose store records no config path shows no line at all.
#[test]
fn a_project_with_no_recorded_config_path_shows_no_line() {
    let mut s = world();
    let lines = snapshot_lines(&mut s);
    assert!(
        !lines.iter().any(|line| line.starts_with("! config")),
        "{lines:?}"
    );
}

/// T-321: an edited config file is a drift line, the snapshot's second line,
/// naming the file and the applied version.
#[test]
fn an_edited_config_file_is_a_drift_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.toml");
    std::fs::write(&path, "[project]\nid = \"p\"\n").unwrap();
    let mut s = world();
    let canonical = applied_from(&mut s, &path, "[project]\nid = \"p\"\n");
    // The user edited the file after the project was registered.
    std::fs::write(&path, "[project]\nid = \"p\"\n[quota]\nlanes = 2\n").unwrap();

    assert_eq!(
        second_snapshot_line(&mut s),
        format!(
            "! config drift: {canonical} differs from the applied config v1: re-apply it with GEAR_ADMIN=1 gear admin init {canonical}, or restore the file"
        )
    );
}

/// T-321: once the applied config matches the file again, the line is gone.
#[test]
fn the_drift_line_goes_when_the_applied_config_matches_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.toml");
    let edited = "[project]\nid = \"p\"\n[quota]\nlanes = 2\n";
    std::fs::write(&path, "[project]\nid = \"p\"\n").unwrap();
    let mut s = world();
    applied_from(&mut s, &path, "[project]\nid = \"p\"\n");
    std::fs::write(&path, edited).unwrap();
    assert!(second_snapshot_line(&mut s).starts_with("! config drift: "));

    // The config file's text is applied: nothing differs any more.
    applied_from(&mut s, &path, edited);
    let lines = snapshot_lines(&mut s);
    assert!(
        !lines.iter().any(|line| line.starts_with("! config")),
        "{lines:?}"
    );
}

/// T-321: a config file that is gone cannot be compared, and the page says so
/// — the applied version still runs.
#[test]
fn a_removed_config_file_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.toml");
    std::fs::write(&path, "[project]\nid = \"p\"\n").unwrap();
    let mut s = world();
    let canonical = applied_from(&mut s, &path, "[project]\nid = \"p\"\n");
    std::fs::remove_file(&path).unwrap();

    assert_eq!(
        second_snapshot_line(&mut s),
        format!("! config file {canonical} cannot be read (config v1 still runs)")
    );
}
