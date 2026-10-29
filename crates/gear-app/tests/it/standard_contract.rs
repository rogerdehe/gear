//! T-84: the project standard travels in every packet.
//!
//! `gear/standard.md` in the project root (what `gear --skill adopt` writes)
//! becomes one line in the task packet and in the review packet, with its
//! absolute path; a project without one says nothing.

use std::fs;
use std::path::Path;

use gear_app::admin;
use gear_app::plan::render_task_packet;
use gear_app::standard;
use gear_app::Env;
use gear_model::TaskState;
use gear_store::{ContractRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;

const TEMPLATE: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "{root}"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

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

/// A registered project whose root is a temp dir, with one task on lane heat.
fn world() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(
        &mut s,
        &Env { ctx: None, pane_id: None, admin: true },
        &TEMPLATE.replace("{root}", &root.path().display().to_string()),
        Some(crate::skill_home()),
        None,
        T0,
    )
    .unwrap();
    s.write(|tx| {
        tx.insert_task(&TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "toy".into(),
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
            issue: None,
            updated: T0,
        })?;
        tx.insert_contract(&ContractRow {
            project: "toy".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
            mechanical: None,
        })
    })
    .unwrap();
    (root, s)
}

fn write_standard(root: &Path) {
    fs::create_dir_all(root.join("gear")).unwrap();
    fs::write(root.join("gear/standard.md"), "# Standard\n\none good example\n").unwrap();
}

#[test]
fn the_line_names_the_absolute_file_and_what_it_is_for() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(standard::line(&root.path().display().to_string()), None, "no file, no line");

    write_standard(root.path());

    let line = standard::line(&root.path().display().to_string()).expect("the line");
    let path = fs::canonicalize(root.path().join("gear/standard.md")).unwrap();
    assert_eq!(
        line,
        format!(
            "Project standard: {} — new and changed code follows it, even where the code around it does not; the reviewer judges against it.",
            path.display()
        )
    );
    // A relative root resolves the same way.
    assert!(standard::line("").is_none(), "an empty root names no file");
}

#[test]
fn the_task_packet_carries_the_line_only_when_the_standard_exists() {
    let (root, mut s) = world();
    let without = s.read(|tx| render_task_packet(tx, "toy", "T-1")).unwrap();
    assert!(!without.contains("Project standard:"), "{without}");

    write_standard(root.path());

    let with = s.read(|tx| render_task_packet(tx, "toy", "T-1")).unwrap();
    assert!(with.contains("Project standard: "), "{with}");
    assert!(with.contains("gear/standard.md"), "{with}");
    assert!(with.contains("the reviewer judges against it"), "{with}");
    // It travels with the packet, not instead of it.
    assert!(with.contains("## Goal"), "{with}");
    assert!(with.contains("gear send --kind result"), "{with}");
}
