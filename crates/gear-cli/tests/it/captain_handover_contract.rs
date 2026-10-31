//! T-466: the captain handover. `gear captain handover --facts` prints the
//! template and its part 1 facts (handover.md part B), and `gear captain take`
//! asks the previous captain for its handover document; when none arrives
//! within the bounded time, the new captain is pointed at the previous session
//! to read and summarise.

use std::path::Path;

use gear_app::captain::{handover_facts, take, HandoverRequest, Machine, HANDOVER_ASK_MIN};
use gear_app::Env;
use gear_herdr::fake::FakeHerdr;
use gear_herdr::AgentStatus;
use gear_model::{GearCtx, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{
    testing, ContractRow, NewMessage, PlanRow, Store, TaskRow, VoyageRow, VoyageStation,
};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";
const NEW_PANE: &str = "w1:p3";

const CAP_CALLER: [u32; 2] = [60_001, 60_000];
const LANE_CALLER: [u32; 2] = [50_001, 50_000];
const NEW_CALLER: [u32; 2] = [80_001, 80_000];

fn config(root: &Path) -> String {
    format!(
        r#"
[project]
id = "p"
herdr_session = "sess"
repo = {root}

[quota]
lanes = 1
temps = 1

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "the engine and the CLI"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "pi"
profile = "pi"

[models]
lanes = ["provider/fast-model"]
review = ["provider/strong-model"]

[conventions]
gate = "make test"
"#,
        root = serde_json::json!(root.to_string_lossy()).to_string()
    )
}

/// The project, its voyage M6, its seats, a task and one open item.
fn world(root: &Path) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        let mut project = tx.get_project("p")?.unwrap();
        project.config_toml = config(root);
        project.plan_version = 7;
        tx.update_project(&project)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        // The previous captain's CLI session: the new captain reads it when
        // the handover document does not arrive.
        tx.set_binding_session_ref("p:captain", 1, "sess-abc")?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        tx.upsert_voyage(&VoyageRow {
            project: "p".to_string(),
            milestone: "M6".to_string(),
            title: "a fleet that needs no nursing".to_string(),
            done: Some("(1) every M6 task merged; (2) the voyage never stops".to_string()),
            bar: Some("DELIVERABLE: an error in normal use blocks the task".to_string()),
            eta: Some("12h".to_string()),
            departure: Some(NOW - 3_600),
            departure_tasks: Some(1),
            departure_estimate_min: Some(45),
            updated: NOW,
        })?;
        tx.set_voyage_stations(
            "p",
            "M6",
            &[
                VoyageStation {
                    id: "S1".to_string(),
                    needs: "deliveries never stall".to_string(),
                },
                VoyageStation {
                    id: "S2".to_string(),
                    needs: "captains hand over".to_string(),
                },
            ],
        )?;
        tx.insert_plan(&PlanRow {
            project: "p".to_string(),
            version: 7,
            draft: String::new(),
            applied: NOW - 600,
            by_seat: None,
            by_gen: None,
        })?;
        tx.insert_task(&TaskRow {
            project: "p".to_string(),
            id: "T-1".to_string(),
            rank: 1,
            state: TaskState::Running,
            priority: "must".to_string(),
            lane: Some("heat".to_string()),
            milestone: Some("M6".to_string()),
            required: true,
            contract_version: 1,
            deps: Vec::new(),
            followup_of: None,
            wait_on: None,
            wait_cond: None,
            rework_note: None,
            estimate_min: Some(45),
            issue: None,
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            updated: NOW,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".to_string(),
            task: "T-1".to_string(),
            version: 1,
            goal: "the engine hands over".to_string(),
            accept: "tests".to_string(),
            evidence_files: Vec::new(),
            evidence_due_min: None,
            body: String::new(),
            created: NOW,
            mechanical: None,
        })?;
        tx.insert_message(&NewMessage {
            meta: None,
            project: "p".to_string(),
            thread: "heat:1".to_string(),
            task: Some("T-1".to_string()),
            attempt: Some(1),
            sender: "p:heat".to_string(),
            sender_gen: 1,
            recipient: "p:captain".to_string(),
            kind: MsgKind::Question,
            urgent: true,
            body: "How do I read the handover?".to_string(),
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: NOW,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

fn herdr() -> FakeHerdr {
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
    herdr.add_agent(NEW_PANE, "claude", AgentStatus::Idle);
    herdr.set_agent_name(CAP_PANE, Some("p-captain"));
    herdr.set_pane_processes(CAP_PANE, Some(CAP_CALLER[1]), &[]);
    herdr.set_pane_processes(LANE_PANE, Some(LANE_CALLER[1]), &[]);
    herdr.set_pane_processes(NEW_PANE, Some(NEW_CALLER[1]), &[]);
    herdr
}

fn bare() -> Env {
    Env {
        ctx: None,
        pane_id: None,
        admin: false,
    }
}

fn engine() -> Machine {
    Machine {
        engine: Some("running · binary /usr/bin/gear · gear 0.6.1".to_string()),
    }
}

/// The template is printed with its part 1 facts: the voyage, the goal and its
/// clauses, the bar, the eta, the stations, the staffing, the board, the open
/// items and the plan — and the skeleton of parts 2 and 3.
#[test]
fn facts_print_the_template_with_its_part_1() {
    let root = crate::scratch::temp("captain-handover-facts");
    let mut s = world(&root);
    let facts = handover_facts(&mut s, &bare(), &root, &engine(), NOW).unwrap();

    assert_eq!(facts.project, "p");
    assert_eq!(facts.voyage, "M6");
    assert_eq!(facts.directory, "gear/voyages/M6/handovers");
    assert!(facts.document.is_none(), "no document yet");

    let text = &facts.text;
    assert!(
        text.contains("# Captain handover — p · voyage M6"),
        "{text}"
    );
    assert!(
        text.contains("gear/voyages/M6/handovers/captain-"),
        "{text}"
    );
    assert!(text.contains("## 1. Facts"), "{text}");
    assert!(text.contains("Goal: (1) every M6 task merged"), "{text}");
    assert!(
        text.contains("Done clauses: (1) every M6 task merged"),
        "{text}"
    );
    assert!(text.contains("Bar: DELIVERABLE"), "{text}");
    assert!(text.contains("Eta: 12h"), "{text}");
    assert!(text.contains("S1 now: deliveries never stall"), "{text}");
    assert!(text.contains("S2 next: captains hand over"), "{text}");
    assert!(text.contains("Staffing:"), "{text}");
    assert!(text.contains("heat: the engine and the CLI"), "{text}");
    assert!(text.contains("provider/fast-model"), "{text}");
    assert!(text.contains("Board: 1 tasks"), "{text}");
    assert!(text.contains("Open items (1):"), "{text}");
    assert!(text.contains("M-1 [urgent]"), "{text}");
    assert!(text.contains("How do I read the handover?"), "{text}");
    assert!(text.contains("gear reply M-1 --resolve"), "{text}");
    assert!(text.contains("Plan: applied version 7"), "{text}");
    assert!(text.contains("running · binary /usr/bin/gear"), "{text}");
    assert!(text.contains("## 2. Summary"), "{text}");
    assert!(
        text.contains("The user's standing orders this voyage"),
        "{text}"
    );
    assert!(text.contains("Files to read first:"), "{text}");
    assert!(text.contains("## 3. Free notes"), "{text}");
}

/// `gear captain take` asks the previous captain for the handover document at
/// once, records the request with its bounded time, and the take item names
/// the previous session for the new captain to read when the document does not
/// arrive.
#[test]
fn take_asks_the_previous_captain_and_points_at_its_session() {
    let root = crate::scratch::temp("captain-handover-take");
    let mut s = world(&root);
    let mut herdr = herdr();

    let taken = take(&mut s, &bare(), &mut herdr, &NEW_CALLER, &root, None, NOW).unwrap();

    assert!(taken.document.is_none(), "{:?}", taken.document);
    let request = taken.request.as_ref().expect("a handover request");
    assert_eq!(request.pane, CAP_PANE);
    assert_eq!(request.session_ref.as_deref(), Some("sess-abc"));
    assert!(request.asked, "the old captain's agent was asked");
    assert_eq!(request.deadline, NOW + HANDOVER_ASK_MIN * 60);

    // The prompt went to the previous captain's pane.
    let prompts = herdr.prompts_to(CAP_PANE);
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(
        prompts[0].contains("Captain handover of p"),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("gear captain handover --facts"),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("gear/voyages/M6/handovers"),
        "{}",
        prompts[0]
    );

    // The request is recorded for the new captain's facts after the deadline.
    let raw = s
        .read(|tx| tx.get_meta("captain-handover:p"))
        .unwrap()
        .expect("the request");
    let recorded: HandoverRequest = serde_json::from_str(&raw).unwrap();
    assert_eq!(recorded, *request);

    // The take item carries the pointer to the previous session.
    let items = s
        .read(|tx| tx.thread_messages("p", "captain:take"))
        .unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    let body = &items[0].body;
    assert!(body.contains("session sess-abc"), "{body}");
    assert!(body.contains("gear/voyages/M6/handovers"), "{body}");
    assert!(body.contains("write part 2 yourself"), "{body}");
}

/// After the bounded time with no document, `--facts` points the new captain
/// at the previous session to read and summarise.
#[test]
fn facts_point_at_the_previous_session_when_no_document_arrived() {
    let root = crate::scratch::temp("captain-handover-deadline");
    let mut s = world(&root);
    let mut herdr = herdr();
    take(&mut s, &bare(), &mut herdr, &NEW_CALLER, &root, None, NOW).unwrap();

    let facts = handover_facts(
        &mut s,
        &bare(),
        &root,
        &engine(),
        NOW + HANDOVER_ASK_MIN * 60 + 1,
    )
    .unwrap();
    assert!(facts.document.is_none());
    let text = &facts.text;
    assert!(
        text.contains("none under gear/voyages/M6/handovers after the request"),
        "{text}"
    );
    assert!(text.contains("session sess-abc (codex)"), "{text}");
    assert!(text.contains("write part 2 yourself"), "{text}");
}

/// A document the previous captain already wrote is named to the new captain
/// instead of asking for another.
#[test]
fn facts_name_an_existing_handover_document() {
    let root = crate::scratch::temp("captain-handover-document");
    let mut s = world(&root);
    let dir = root.join("gear/voyages/M6/handovers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("captain-2026-10-31-1730.md"),
        "# Captain handover\n",
    )
    .unwrap();

    let facts = handover_facts(&mut s, &bare(), &root, &engine(), NOW).unwrap();
    assert_eq!(
        facts.document.as_deref(),
        Some("gear/voyages/M6/handovers/captain-2026-10-31-1730.md")
    );
    assert!(
        facts.text.contains(
            "Previous captain's handover: gear/voyages/M6/handovers/captain-2026-10-31-1730.md"
        ),
        "{}",
        facts.text
    );
}

/// A previous captain whose pane is gone cannot be asked: the take item still
/// names its session to read.
#[test]
fn take_points_at_the_session_when_the_previous_pane_is_gone() {
    let root = crate::scratch::temp("captain-handover-gone");
    let mut s = world(&root);
    let mut herdr = herdr();
    herdr.exit_agent(CAP_PANE);

    let taken = take(&mut s, &bare(), &mut herdr, &NEW_CALLER, &root, None, NOW).unwrap();
    let request = taken.request.as_ref().expect("a handover request");
    assert!(!request.asked, "no agent was there to ask");
    assert!(herdr.prompts_to(CAP_PANE).is_empty());
    let items = s
        .read(|tx| tx.thread_messages("p", "captain:take"))
        .unwrap();
    assert!(
        items[0].body.contains("hosts no agent to ask"),
        "{}",
        items[0].body
    );
    assert!(
        items[0].body.contains("session sess-abc"),
        "{}",
        items[0].body
    );
}

/// The command itself: `gear captain handover --facts` prints the template,
/// and `--pane` is required when `--facts` is not given.
#[test]
fn the_command_prints_the_facts_and_requires_a_pane_to_rebind() {
    let home = crate::scratch::home("captain-handover");
    let root = crate::scratch::temp("captain-handover-root");
    let db = crate::scratch::default_store(&home);
    std::fs::create_dir_all(db.parent().expect("the state dir")).unwrap();
    {
        let mut store = Store::open(&db).unwrap();
        let mut world = world(&root);
        // Copy the seeded rows into the file store the command reads.
        store
            .write(|tx| {
                for project in world.read(|src| src.project_ids())? {
                    let row = world.read(|src| src.get_project(&project))?.unwrap();
                    tx.insert_project(&row)?;
                }
                for seat in world.read(|src| src.seats_of_project("p"))? {
                    tx.insert_seat(&seat)?;
                }
                if let Some(binding) = world.read(|src| src.get_binding("p:captain", 1))? {
                    tx.insert_binding(&binding)?;
                }
                if let Some(binding) = world.read(|src| src.get_binding("p:heat", 1))? {
                    tx.insert_binding(&binding)?;
                }
                let voyage = world.read(|src| src.get_voyage("p", "M6"))?.unwrap();
                tx.upsert_voyage(&voyage)?;
                tx.set_voyage_stations(
                    "p",
                    "M6",
                    &world.read(|src| src.voyage_stations("p", "M6"))?,
                )?;
                for task in world.read(|src| src.tasks_of_project("p"))? {
                    tx.insert_task(&task)?;
                }
                for version in 1..=1 {
                    if let Some(contract) =
                        world.read(|src| src.get_contract("p", "T-1", version))?
                    {
                        tx.insert_contract(&contract)?;
                    }
                }
                if let Some(plan) = world.read(|src| src.get_plan("p", 7))? {
                    tx.insert_plan(&plan)?;
                }
                for message in world.read(|src| src.thread_messages("p", "heat:1"))? {
                    tx.insert_message(&gear_store::NewMessage {
                        meta: None,
                        project: message.project,
                        thread: message.thread,
                        task: message.task,
                        attempt: message.attempt,
                        sender: message.sender,
                        sender_gen: message.sender_gen,
                        recipient: message.recipient,
                        kind: message.kind,
                        urgent: message.urgent,
                        body: message.body,
                        reply_to: message.reply_to,
                        needs_ack: message.needs_ack,
                        state: message.state,
                        created: message.created,
                    })?;
                }
                Ok(())
            })
            .unwrap();
    }

    let out = crate::scratch::gear(&home, &["captain", "handover", "--facts"])
        .env("GEAR_DB", &db)
        .current_dir(&root)
        .output()
        .expect("run gear captain handover --facts");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("## 1. Facts"), "{text}");
    assert!(text.contains("Goal: (1) every M6 task merged"), "{text}");
    assert!(text.contains("## 2. Summary"), "{text}");

    let no_pane = crate::scratch::gear(&home, &["captain", "handover"])
        .env("GEAR_DB", &db)
        .current_dir(&root)
        .output()
        .expect("run gear captain handover");
    assert!(!no_pane.status.success(), "a rebind needs --pane");
    let err = String::from_utf8_lossy(&no_pane.stderr);
    assert!(err.contains("--pane"), "{err}");
}

/// A caller with no environment, from a captain's stale generation, still gets
/// the facts: the old captain writes the document after the seat moved.
#[test]
fn facts_work_for_a_stale_generation() {
    let root = crate::scratch::temp("captain-handover-stale");
    let mut s = world(&root);
    let env = Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    };
    let facts = handover_facts(&mut s, &env, &root, &engine(), NOW).unwrap();
    assert!(facts.text.contains("# Captain handover — p · voyage M6"));
}
