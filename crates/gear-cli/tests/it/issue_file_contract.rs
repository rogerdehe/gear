//! Contract for `gear issue file` / `gear issue show` (T-683): the captain
//! records an issue request for the engine to file, and reads the number the
//! engine gave it. The CLI never runs `gh`; the store is the only writer here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gear_model::SeatRole;
use gear_store::{testing, Store};

const T0: i64 = 1_800_000_000;

/// A store with project `p`, its captain and one lane; `github` adds the
/// `[github]` mode the command needs.
fn store_with_plan(db: &Path, github: bool) -> Store {
    let mut store = Store::open(db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            let mut project = tx.get_project("p")?.expect("the project");
            let base =
                "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/tmp/gear-issues\"\n\
                        \n[quota]\nlanes = 1\ntemps = 0\n\
                        \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
                        \n[conventions]\nintegration = \"dev\"\n";
            project.config_toml = if github {
                format!("{base}\n[github]\nrepo = \"owner/name\"\n")
            } else {
                base.to_string()
            };
            tx.update_project(&project)?;
            Ok(())
        })
        .expect("the world");
    store
}

fn gear(home: &Path, db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(args)
        .env("HOME", home)
        .env("GEAR_DB", db)
        .env("GEAR_CTX", "p:captain:1")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn scratch(tag: &str) -> PathBuf {
    crate::scratch::temp(&format!("gear-issue-{tag}"))
}

fn requests(store: &mut Store) -> Vec<serde_json::Value> {
    store
        .read(|tx| tx.get_meta("forge.issue_requests.p"))
        .expect("the meta")
        .map(|raw| serde_json::from_str(&raw).expect("the requests parse"))
        .unwrap_or_default()
}

#[test]
fn a_captain_files_an_issue_request() {
    let home = scratch("file");
    let db = home.join("gear.db");
    let mut store = store_with_plan(&db, true);

    let out = gear(
        &home,
        &db,
        &[
            "issue", "file", "--title", "A bug", "--body", "steps", "--label", "bug",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("issue request 1 recorded"),
        "{}",
        stdout(&out)
    );

    let stored = requests(&mut store);
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert_eq!(stored[0]["id"], serde_json::json!(1));
    assert_eq!(stored[0]["title"], serde_json::json!("A bug"));
    assert_eq!(stored[0]["body"], serde_json::json!("steps"));
    assert_eq!(stored[0]["labels"], serde_json::json!(["bug"]));
    assert_eq!(stored[0]["number"], serde_json::Value::Null);

    let out = gear(
        &home,
        &db,
        &["issue", "file", "--title", "Another", "--body", "more"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("issue request 2 recorded"),
        "{}",
        stdout(&out)
    );
    assert_eq!(requests(&mut store).len(), 2);

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn show_reads_the_number_once_filed() {
    let home = scratch("show");
    let db = home.join("gear.db");
    let mut store = store_with_plan(&db, true);
    store
        .write(|tx| {
            tx.set_meta(
                "forge.issue_requests.p",
                r#"[{"id":1,"title":"A bug","body":"steps","labels":["bug"],"number":42}]"#,
            )
        })
        .expect("the filed request");

    let out = gear(&home, &db, &["issue", "show", "1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("issue request 1: filed as #42"),
        "{}",
        stdout(&out)
    );

    let out = gear(&home, &db, &["issue", "show", "9"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no issue request 9"),
        "{}",
        stderr(&out)
    );

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_project_without_github_refuses() {
    let home = scratch("no-github");
    let db = home.join("gear.db");
    let mut store = store_with_plan(&db, false);

    let out = gear(
        &home,
        &db,
        &["issue", "file", "--title", "A bug", "--body", "steps"],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("not in GitHub mode"),
        "{}",
        stderr(&out)
    );
    assert!(requests(&mut store).is_empty(), "nothing was written");

    std::fs::remove_dir_all(&home).ok();
}
