//! T-487: a pane that holds several captain seats gets one doorbell, not one
//! per project. The engine joins the items of every seat bound to that pane
//! into one ring, each line naming its project, urgent first; a project
//! outside its working hours adds nothing while the other projects' rings go
//! on; a single project rings as it always did.

use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{MsgKind, MsgState, OpKind, OpState, SeatRole};
use gear_store::{testing, NewMessage, Store, VoyageRow};

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
}

/// Two projects whose captain seats are bound to the same pane, with one codex
/// agent in it. `q_config` is project q's `[schedule]` block (empty for none).
fn world(q_config: &str) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
            testing::seed_project(tx, "q", "sess", T0)?;
            let mut q = tx.get_project("q")?.unwrap();
            q.config_toml = format!(
                "[project]\nid = \"q\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n[quota]\nlanes = 1\ntemps = 0\n\n[captain]\npane = \"{CAP_PANE}\"\nkind = \"codex\"\nprofile = \"codex\"\n\n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\nkind = \"codex\"\nprofile = \"codex\"\n{q_config}"
            );
            tx.update_project(&q)?;
            testing::seed_active_seat(tx, "q", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "p-captain");
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    World { store, herdr, cfg }
}

/// One open item for a project's captain, and its id.
fn captain_item(w: &mut World, project: &str, urgent: bool, at: i64) -> String {
    w.store
        .write(|tx| {
            Ok(tx
                .insert_message(&NewMessage {
                    meta: None,
                    project: project.into(),
                    thread: format!("{project}:{at}"),
                    task: None,
                    attempt: None,
                    sender: format!("{project}:captain"),
                    sender_gen: 1,
                    recipient: format!("{project}:captain"),
                    kind: MsgKind::System,
                    urgent,
                    body: "act on it".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: at,
                })?
                .id)
        })
        .unwrap()
}

fn doorbells(w: &mut World, project: &str, seat: &str) -> Vec<gear_store::OpRow> {
    w.store
        .read(|tx| tx.ops_of_seat_kind(&format!("{project}:{seat}"), OpKind::Doorbell))
        .unwrap()
}

#[test]
fn two_projects_sharing_a_pane_get_one_ring_naming_both() {
    let mut w = world("");
    let p_item = captain_item(&mut w, "p", false, T0);
    let q_item = captain_item(&mut w, "q", true, T0);
    let report = tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let prompts = w.herdr.prompts_to(CAP_PANE);
    assert_eq!(prompts.len(), 1, "one doorbell for the pane: {prompts:?}");
    let text = prompts[0];
    assert!(text.contains(&p_item) && text.contains(&q_item), "{text}");
    assert!(
        text.contains("p:") && text.contains("q:"),
        "each line names its project: {text}"
    );
    // Urgent first: q's line leads. (Message ids are per project, so both
    // items are M-1; the project lines tell them apart.)
    let q_at = text.find("- q:").unwrap();
    let p_at = text.find("- p:").unwrap();
    assert!(q_at < p_at, "the urgent project leads: {text}");
    // One op per seat: the ring's own (typed once) and q's (covered, not typed).
    let p_ops = doorbells(&mut w, "p", "captain");
    let q_ops = doorbells(&mut w, "q", "captain");
    assert_eq!(p_ops.len(), 1, "{p_ops:?}");
    assert_eq!(q_ops.len(), 1, "{q_ops:?}");
    assert_eq!(p_ops[0].state, OpState::Submitted);
    assert_eq!(q_ops[0].state, OpState::Submitted);
    assert!(p_ops[0]
        .payload
        .get("covers")
        .and_then(|v| v.as_array())
        .is_some_and(|ids| ids.iter().any(|id| id == &serde_json::json!(q_item))));
    assert!(report.intended.len() >= 2);
}

#[test]
fn an_urgent_joined_ring_is_typed_at_once_into_a_working_shared_captain() {
    // The review's F1: an urgent ring of two projects must reach a shared
    // captain agent that is working, as a single project's urgent item does.
    let mut w = world("");
    w.herdr.set_status(CAP_PANE, AgentStatus::Working);
    let p_item = captain_item(&mut w, "p", true, T0);
    // A filler gives q's urgent item a different id than p's M-1.
    let _filler = captain_item(&mut w, "q", false, T0);
    let q_item = captain_item(&mut w, "q", true, T0 + 1);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 2).unwrap();
    let prompts = w.herdr.prompts_to(CAP_PANE);
    assert_eq!(prompts.len(), 1, "typed at once: {prompts:?}");
    let text = prompts[0];
    assert!(text.contains(&p_item) && text.contains(&q_item), "{text}");
    for (project, item) in [("p", &p_item), ("q", &q_item)] {
        let ops = doorbells(&mut w, project, "captain");
        assert_eq!(ops.len(), 1, "{project}: {ops:?}");
        assert_eq!(ops[0].state, OpState::Submitted, "{project}: {ops:?}");
        let covers = ops[0]
            .payload
            .get("covers")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            covers,
            vec![serde_json::json!(item)],
            "{project}: {covers:?}"
        );
    }
}

#[test]
fn a_project_off_hours_adds_nothing_while_the_other_rings() {
    // q rests: its departure passed and its voyage eta is over.
    let mut w = world("\n[schedule]\ndepart_at = \"2027-01-01T00:00:00Z\"\n");
    w.store
        .write(|tx| {
            tx.upsert_voyage(&VoyageRow {
                project: "q".into(),
                milestone: "M1".into(),
                title: "q".into(),
                done: Some("done".into()),
                bar: None,
                eta: Some("1h".into()),
                departure: Some(T0),
                departure_tasks: Some(0),
                departure_estimate_min: None,
                updated: T0,
            })
        })
        .unwrap();
    let p_item = captain_item(&mut w, "p", false, T0);
    let _q_item = captain_item(&mut w, "q", true, T0);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let prompts = w.herdr.prompts_to(CAP_PANE);
    assert_eq!(
        prompts.len(),
        1,
        "the working project still rings: {prompts:?}"
    );
    assert!(
        prompts[0].starts_with("GEAR: 1 item(s) waiting for you: "),
        "one item, the working project's: {}",
        prompts[0]
    );
    assert!(prompts[0].contains(&p_item), "{}", prompts[0]);
    assert!(
        !prompts[0].contains("- q:"),
        "the resting project adds nothing: {}",
        prompts[0]
    );
    assert!(
        doorbells(&mut w, "q", "captain").is_empty(),
        "no ring op for the resting project"
    );
}

#[test]
fn a_single_project_rings_as_today() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                CAP_PANE,
                "codex",
                T0,
            )?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "p-captain");
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    let mut w = World { store, herdr, cfg };
    let item = captain_item(&mut w, "p", true, T0);
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + 1).unwrap();
    let prompts = w.herdr.prompts_to(CAP_PANE);
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].starts_with("GEAR: 1 item(s) waiting for you (1 urgent): "),
        "{}",
        prompts[0]
    );
    assert!(prompts[0].contains(&item), "{}", prompts[0]);
    assert!(
        prompts[0].contains("Run the command `gear` (no arguments)"),
        "{}",
        prompts[0]
    );
}
