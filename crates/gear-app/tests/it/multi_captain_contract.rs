//! T-485: one captain pane holds several projects' captain seats. The seats
//! keep their own generations, a release frees one, and a reply takes its
//! project from a qualified id (an unqualified id two projects hold is
//! refused with both qualified ids named).

use std::path::Path;

use gear_app::captain::{release, take_all};
use gear_app::identity::Fingerprint;
use gear_app::mail::{self, ReplyArgs, ReplyMode};
use gear_app::Env;
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{ErrorCode, GearCtx, MsgKind, MsgState, SeatRole, SeatState};
use gear_store::{testing, BindingStatus, NewMessage, Store};

const NOW: i64 = 1_800_000_000;
const OLD_PANE: &str = "w1:p1";
const CAP_PANE: &str = "w1:p2";

fn config(root: &Path, project: &str) -> String {
    format!(
        r#"
[project]
id = "{project}"
herdr_session = "sess"
repo = {root}

[quota]
lanes = 1
temps = 0

[captain]
pane = "{OLD_PANE}"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#,
        root = serde_json::json!(root.to_string_lossy()).to_string()
    )
}

fn world(root: &Path) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        for project in ["gear", "prometheus"] {
            testing::seed_project(tx, project, "sess", NOW)?;
            let mut row = tx.get_project(project)?.unwrap();
            row.config_toml = config(root, project);
            tx.update_project(&row)?;
            testing::seed_active_seat(
                tx,
                project,
                "captain",
                SeatRole::Captain,
                OLD_PANE,
                "codex",
                NOW,
            )?;
        }
        Ok(())
    })
    .unwrap();
    s
}

fn herdr() -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(OLD_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr
}

fn bare() -> Env {
    Env {
        ctx: None,
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    }
}

fn env(project: &str, generation: i64) -> Env {
    Env {
        ctx: Some(GearCtx {
            project: project.into(),
            seat: "captain".into(),
            generation,
        }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    }
}

fn fp() -> Fingerprint {
    Fingerprint {
        pane_id: CAP_PANE.into(),
        kind: Some("codex".into()),
        session_ref: None,
    }
}

fn question(s: &mut Store, project: &str, body: &str) -> String {
    s.write(|tx| {
        Ok(tx
            .insert_message(&NewMessage {
                meta: None,
                project: project.into(),
                thread: format!("question:{project}"),
                task: None,
                attempt: None,
                sender: format!("{project}:heat"),
                sender_gen: 1,
                recipient: format!("{project}:captain"),
                kind: MsgKind::Question,
                urgent: false,
                body: body.into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: NOW,
            })?
            .id)
    })
    .unwrap()
}

fn reply_args(ids: &[String], request_id: &str) -> ReplyArgs {
    ReplyArgs {
        ids: ids.to_vec(),
        text: "answer".into(),
        mode: ReplyMode::Resolve,
        wait_on: None,
        wait_until: None,
        downgrade: false,
        request_id: request_id.into(),
    }
}

#[test]
fn a_pane_takes_two_projects_at_their_own_generations_and_keeps_them() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();

    let taken = take_all(
        &mut s,
        &bare(),
        &mut herdr,
        &[],
        root.path(),
        &["gear".to_string(), "prometheus".to_string()],
        NOW,
    )
    .unwrap();
    assert_eq!(taken.len(), 2, "{taken:?}");
    for (result, project) in taken.iter().zip(["gear", "prometheus"]) {
        assert_eq!(result.project, project);
        assert_eq!(result.to, CAP_PANE);
        assert_eq!(result.generation, 2, "{result:?}");
        let seat = s
            .read(|tx| tx.get_seat(&format!("{project}:captain")))
            .unwrap()
            .unwrap();
        assert_eq!((seat.generation, seat.state), (2, SeatState::Active));
        let binding = s
            .read(|tx| tx.get_binding(&format!("{project}:captain"), 2))
            .unwrap()
            .unwrap();
        assert_eq!(
            (binding.pane_id.as_str(), binding.status),
            (CAP_PANE, BindingStatus::Active)
        );
    }

    // Taking again keeps the seats: no new generation, no new item.
    let again = take_all(
        &mut s,
        &bare(),
        &mut herdr,
        &[],
        root.path(),
        &["gear".to_string()],
        NOW + 10,
    )
    .unwrap();
    assert_eq!(again[0].generation, 2, "a held seat keeps its generation");
    let gear = s.read(|tx| tx.get_seat("gear:captain")).unwrap().unwrap();
    assert_eq!(gear.generation, 2);
}

#[test]
fn a_release_frees_one_project_and_keeps_the_other() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    take_all(
        &mut s,
        &bare(),
        &mut herdr,
        &[],
        root.path(),
        &["gear".to_string(), "prometheus".to_string()],
        NOW,
    )
    .unwrap();

    let released = release(&mut s, &bare(), &mut herdr, &[], "gear", NOW + 10).unwrap();
    assert_eq!(released.project, "gear");
    assert_eq!(released.pane, CAP_PANE);
    let gear = s.read(|tx| tx.get_seat("gear:captain")).unwrap().unwrap();
    assert_eq!(gear.state, SeatState::Released);
    let binding = s
        .read(|tx| tx.get_binding("gear:captain", gear.generation))
        .unwrap()
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Void);
    // The other seat stays bound and active.
    let other = s
        .read(|tx| tx.get_seat("prometheus:captain"))
        .unwrap()
        .unwrap();
    assert_eq!(other.state, SeatState::Active);
    let binding = s
        .read(|tx| tx.get_binding("prometheus:captain", other.generation))
        .unwrap()
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Active);
}

#[test]
fn a_qualified_reply_reaches_its_project_while_another_holds_the_same_id() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    take_all(
        &mut s,
        &bare(),
        &mut herdr,
        &[],
        root.path(),
        &["gear".to_string(), "prometheus".to_string()],
        NOW,
    )
    .unwrap();
    let gear = question(&mut s, "gear", "gear asks");
    let prometheus = question(&mut s, "prometheus", "prometheus asks");
    assert_eq!(
        gear, prometheus,
        "each project's first message has the same id"
    );

    // The caller is the newest seat's (gear) captain; the qualified id names
    // prometheus, whose captain this pane also holds.
    let out = mail::reply(
        &mut s,
        &env("gear", 2),
        Some(&fp()),
        &reply_args(&[format!("prometheus:{prometheus}")], "r1"),
        NOW + 20,
    )
    .unwrap();
    assert_eq!(out.handled, vec![(prometheus.clone(), "resolved".into())]);
    let prometheus_msg = s
        .read(|tx| tx.get_message("prometheus", &prometheus))
        .unwrap()
        .unwrap();
    assert_eq!(prometheus_msg.state, MsgState::Resolved);
    let gear_msg = s.read(|tx| tx.get_message("gear", &gear)).unwrap().unwrap();
    assert_eq!(
        gear_msg.state,
        MsgState::Open,
        "the other project is untouched"
    );
}

#[test]
fn an_unqualified_id_two_projects_hold_is_refused_with_both_qualified_ids() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    take_all(
        &mut s,
        &bare(),
        &mut herdr,
        &[],
        root.path(),
        &["gear".to_string(), "prometheus".to_string()],
        NOW,
    )
    .unwrap();
    let id = question(&mut s, "gear", "gear asks");
    question(&mut s, "prometheus", "prometheus asks");

    let error = mail::reply(
        &mut s,
        &env("gear", 2),
        Some(&fp()),
        &reply_args(&[id.clone()], "r2"),
        NOW + 20,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat);
    assert!(
        error.detail.contains(&format!("gear:{id}")),
        "{}",
        error.detail
    );
    assert!(
        error.detail.contains(&format!("prometheus:{id}")),
        "{}",
        error.detail
    );
}

#[test]
fn one_held_project_keeps_the_unqualified_reply() {
    let root = tempfile::tempdir().unwrap();
    let mut s = world(root.path());
    let mut herdr = herdr();
    take_all(
        &mut s,
        &bare(),
        &mut herdr,
        &[],
        root.path(),
        &["gear".to_string()],
        NOW,
    )
    .unwrap();
    let id = question(&mut s, "gear", "gear asks");
    let out = mail::reply(
        &mut s,
        &env("gear", 2),
        Some(&fp()),
        &reply_args(&[id.clone()], "r3"),
        NOW + 20,
    )
    .unwrap();
    assert_eq!(out.handled, vec![(id, "resolved".into())]);
}
