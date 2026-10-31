//! T-486: the `gear` page of a captain that holds several projects, and the
//! qualified ids `gear --show` and `gear friction close` take.
//!
//! These run the real binary: the page is what the captain reads. The caller
//! is the pane this test runs in (its ancestry is the pane's shell), so the
//! test seeds the captains' bindings on that pane and asks Herdr for the
//! occupant's kind. Without a Herdr socket there is no seat to prove, and the
//! test does nothing (the rendered-page tests of `gear-app` still run).

use std::path::Path;
use std::process::{Command, Output};

use gear_model::{MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, NewFriction, NewMessage, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;

/// The pane this test runs in, and the kind of agent Herdr reports there.
fn pane_and_kind() -> Option<(String, String)> {
    use gear_herdr::HerdrRead;
    let pane = std::env::var("HERDR_PANE_ID").ok()?;
    let socket = std::env::var("HERDR_SOCKET_PATH").ok()?;
    let mut herdr = gear_herdr::SocketHerdr::connect(Path::new(&socket)).ok()?;
    let info = herdr.agent_get(&pane).ok()??;
    Some((pane, info.agent?))
}

fn gear(home: &Path, db: &Path, ctx: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("GEAR_DB", db)
        .env("GEAR_CTX", ctx)
        .env(
            "HERDR_PANE_ID",
            std::env::var("HERDR_PANE_ID").unwrap_or_default(),
        )
        .env(
            "HERDR_SOCKET_PATH",
            std::env::var("HERDR_SOCKET_PATH").unwrap_or_default(),
        )
        .env_remove("GEAR_ADMIN")
        .output()
        .expect("run gear")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Two projects, both captained from `pane`, each with a voyage, a task and
/// one open item: `a` a normal one, `b` an urgent one.
fn world(db: &Path, pane: &str, kind: &str) -> Store {
    let mut s = Store::open(db).unwrap();
    s.write(|tx| {
        for (project, title, milestone) in [
            ("a", "the first voyage", "M1"),
            ("b", "the second voyage", "M2"),
        ] {
            testing::seed_project(tx, project, "sess", NOW)?;
            testing::seed_active_seat(tx, project, "captain", SeatRole::Captain, pane, kind, NOW)?;
            tx.upsert_voyage(&VoyageRow {
                project: project.into(),
                milestone: milestone.into(),
                title: title.into(),
                done: Some("it is delivered".into()),
                bar: Some("DELIVERABLE".into()),
                eta: Some("2h".into()),
                departure: Some(NOW - 3_600),
                departure_tasks: Some(1),
                departure_estimate_min: Some(30),
                updated: NOW,
            })?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: project.into(),
                id: "T-1".into(),
                rank: 1,
                state: TaskState::Running,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: Some(milestone.into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: NOW,
            })?;
        }
        Ok(())
    })
    .unwrap();
    for (project, urgent, body) in [
        ("a", false, "the first question"),
        ("b", true, "the urgent question"),
    ] {
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.into(),
                thread: format!("{project}:1"),
                task: None,
                attempt: None,
                sender: format!("{project}:heat"),
                sender_gen: 1,
                recipient: format!("{project}:captain"),
                kind: MsgKind::Question,
                urgent,
                body: body.into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: NOW,
            })?;
            Ok(())
        })
        .unwrap();
    }
    s.write(|tx| {
        tx.insert_friction(&NewFriction {
            project: "b".into(),
            task: "T-1".into(),
            attempt: 1,
            role: "dev".into(),
            seat: "b:heat".into(),
            body: "the second project's friction".into(),
            required: false,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

#[test]
fn the_page_shows_every_held_project_and_urgent_items_first() {
    let Some((pane, kind)) = pane_and_kind() else {
        return;
    };
    let home = crate::scratch::home("home-multi");
    let db = home.join("gear.db");
    let _store = world(&db, &pane, &kind);

    let out = gear(&home, &db, "a:captain:1", &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);

    // Each held project's voyage line.
    assert!(text.contains("Voyage M1 the first voyage"), "{text}");
    assert!(text.contains("Voyage M2 the second voyage"), "{text}");
    // Each item line names its project.
    assert!(text.contains("a:M-1"), "{text}");
    assert!(text.contains("b:M-1"), "{text}");
    // The urgent item of the second project is above the normal one of the
    // first.
    let urgent = text.find("b:M-1").expect("the urgent item");
    let normal = text.find("a:M-1").expect("the normal item");
    assert!(urgent < normal, "urgent first: {text}");

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn show_takes_a_qualified_task_id() {
    let Some((pane, kind)) = pane_and_kind() else {
        return;
    };
    let home = crate::scratch::home("home-show");
    let db = home.join("gear.db");
    let mut s = world(&db, &pane, &kind);
    s.write(|tx| {
        for (project, goal) in [
            ("a", "the first project's task"),
            ("b", "the second project's task"),
        ] {
            tx.insert_contract(&gear_store::ContractRow {
                project: project.into(),
                task: "T-1".into(),
                version: 1,
                goal: goal.into(),
                accept: "the acceptance".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: NOW,
                mechanical: None,
            })?;
        }
        Ok(())
    })
    .unwrap();
    drop(s);

    // The caller is the captain of `a`; `b:T-1` names the other project.
    let out = gear(&home, &db, "a:captain:1", &["--show", "b:T-1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("the second project's task"), "{text}");

    // A bare id keeps the caller's own project (`a`).
    let out = gear(&home, &db, "a:captain:1", &["--show", "T-1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("the first project's task"),
        "{}",
        stdout(&out)
    );

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn friction_close_takes_a_qualified_report_id() {
    let Some((pane, kind)) = pane_and_kind() else {
        return;
    };
    let home = crate::scratch::home("home-friction");
    let db = home.join("gear.db");
    let _store = world(&db, &pane, &kind);

    let out = gear(
        &home,
        &db,
        "a:captain:1",
        &[
            "friction",
            "close",
            "b:F-1",
            "-m",
            "fixed in the second project",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("closed F-1"), "{}", stdout(&out));

    let mut s = Store::open(&db).unwrap();
    let closed = s.read(|tx| tx.frictions_of_project("b")).unwrap();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].closed_at.is_some(), "the report was closed");
    assert!(
        closed[0].outcome.as_deref().unwrap_or("").contains("fixed"),
        "{:?}",
        closed[0].outcome
    );

    std::fs::remove_dir_all(&home).ok();
}
