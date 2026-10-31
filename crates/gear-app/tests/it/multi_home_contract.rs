//! T-486: a captain that holds several projects sees every held project on
//! the `gear` page: each project's voyage line, then its inbox, each item
//! line naming its project, urgent items of all projects first. One project
//! keeps the page as it was.

use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::Env;
use gear_model::{GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, NewMessage, Store, TaskRow, VoyageRow};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

/// Two projects, both captained from the same pane, each with a voyage and
/// one open item: `a` a normal one, `b` an urgent one.
fn two_projects() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        for (project, title, milestone) in [
            ("a", "the first voyage", "M1"),
            ("b", "the second voyage", "M2"),
        ] {
            testing::seed_project(tx, project, "sess", NOW)?;
            testing::seed_active_seat(
                tx,
                project,
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                NOW,
            )?;
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
            task(tx, project, milestone)?;
        }
        Ok(())
    })
    .unwrap();
    item(
        &mut s,
        "a",
        "a:1",
        "a:heat",
        "a:captain",
        false,
        "the first question",
    );
    item(
        &mut s,
        "b",
        "b:1",
        "b:heat",
        "b:captain",
        true,
        "the urgent question",
    );
    s
}

/// One project only: the same page as before T-486.
fn one_project() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "a", "sess", NOW)?;
        testing::seed_active_seat(
            tx,
            "a",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        tx.upsert_voyage(&VoyageRow {
            project: "a".into(),
            milestone: "M1".into(),
            title: "the first voyage".into(),
            done: Some("it is delivered".into()),
            bar: Some("DELIVERABLE".into()),
            eta: Some("2h".into()),
            departure: Some(NOW - 3_600),
            departure_tasks: Some(1),
            departure_estimate_min: Some(30),
            updated: NOW,
        })?;
        task(tx, "a", "M1")?;
        Ok(())
    })
    .unwrap();
    item(
        &mut s,
        "a",
        "a:1",
        "a:heat",
        "a:captain",
        false,
        "the first question",
    );
    s
}

/// One running task in the voyage, so its report line is measured.
fn task(tx: &gear_store::Tx<'_>, project: &str, milestone: &str) -> gear_model::GearResult<()> {
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
    Ok(())
}

fn item(
    s: &mut Store,
    project: &str,
    thread: &str,
    sender: &str,
    recipient: &str,
    urgent: bool,
    body: &str,
) {
    s.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.into(),
            thread: thread.into(),
            task: None,
            attempt: None,
            sender: sender.into(),
            sender_gen: 1,
            recipient: recipient.into(),
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

fn env() -> Env {
    Env {
        ctx: Some(GearCtx {
            project: "a".into(),
            seat: "captain".into(),
            generation: 1,
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

#[test]
fn two_held_projects_show_their_voyage_lines_and_qualified_items() {
    let mut s = two_projects();
    let page = home::build(&mut s, &env(), Some(&fp()), NOW + 10).unwrap();
    let text = &page.text;

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
    // One view per project, each listing only its own message.
    assert_eq!(page.views.len(), 2, "one view per project");
    let a = page.views.iter().find(|v| v.project == "a").unwrap();
    let b = page.views.iter().find(|v| v.project == "b").unwrap();
    assert_eq!(a.shown_msgs, vec!["M-1".to_string()]);
    assert_eq!(b.shown_msgs, vec!["M-1".to_string()]);
}

#[test]
fn a_second_page_shows_no_item_again_as_unshown() {
    let mut s = two_projects();
    let page = home::build(&mut s, &env(), Some(&fp()), NOW + 10).unwrap();
    for view in &page.views {
        home::record_view(&mut s, view).unwrap();
    }
    // A reply to the captain in each project that no page has shown yet.
    for project in ["a", "b"] {
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: project.into(),
                thread: format!("{project}:1"),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: format!("{project}:captain"),
                kind: MsgKind::Reply,
                urgent: false,
                body: format!("answer in {project}"),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Info,
                created: NOW + 20,
            })?;
            Ok(())
        })
        .unwrap();
    }
    let page = home::build(&mut s, &env(), Some(&fp()), NOW + 30).unwrap();
    let text = &page.text;
    assert_eq!(text.matches("UNSHOWN (1)").count(), 2, "{text}");
    assert!(
        text.contains("a:M-2"),
        "the reply is named by project: {text}"
    );
    assert!(
        text.contains("b:M-2"),
        "the other project's reply is named too: {text}"
    );
    // The first page's items were shown: they are not unshown again.
    assert!(!text.contains("UNSHOWN (2)"), "{text}");
    // The item lines keep their project names.
    assert!(text.contains("a:M-1"), "{text}");
    assert!(text.contains("b:M-1"), "{text}");
}

#[test]
fn one_project_renders_as_before() {
    let mut s = one_project();
    let page = home::build(&mut s, &env(), Some(&fp()), NOW + 10).unwrap();
    let text = &page.text;
    assert!(
        text.starts_with("Voyage M1 the first voyage"),
        "the voyage line is first: {text}"
    );
    assert!(text.contains("seat a:captain"), "{text}");
    assert!(text.contains("INBOX (1 threads)"), "{text}");
    assert!(text.contains(" M-1 "), "the bare id: {text}");
    assert!(
        !text.contains("a:M-1"),
        "no qualified id with one project: {text}"
    );
    assert!(!text.contains("URGENT ("), "{text}");
    assert_eq!(page.views.len(), 1);
}
