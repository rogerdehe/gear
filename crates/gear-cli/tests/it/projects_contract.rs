//! `gear admin projects` (T-322): one view across the Herdr sessions.
//!
//! Since v0.4.5 each Herdr session has its own store and nothing listed them
//! together. The command reports every store on this machine — the shared
//! one, one per session, and the registered ones — with the projects in it,
//! and it only reads: nothing is written, no engine starts.

use std::path::Path;
use std::process::Output;

use gear_app::plan::current_voyage;
use gear_store::{testing, Store, VoyageRow};

const T0: i64 = 1_800_000_000;

/// A project in the store at `db`, with a voyage `M2` that departed.
fn seeded(db: &Path, project: &str, session: &str, voyage: Option<&str>) {
    let mut store = Store::open(db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, project, session, T0)?;
            if let Some(milestone) = voyage {
                tx.upsert_voyage(&VoyageRow {
                    project: project.to_string(),
                    milestone: milestone.to_string(),
                    title: format!("the {milestone} station"),
                    done: None,
                    bar: None,
                    eta: None,
                    departure: Some(T0),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(30),
                    updated: T0,
                })?;
            }
            Ok(())
        })
        .expect("a project");
}

/// `gear admin projects …` under `home`, as a caller's process.
fn projects(home: &Path, args: &[&str]) -> Output {
    crate::scratch::gear(home, args).output().expect("run gear admin projects")
}

/// The store headers the human output printed: the lines that are not
/// indented, i.e. the paths.
fn headers(out: &Output) -> Vec<String> {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.starts_with(' '))
        .map(str::to_string)
        .collect()
}

/// A path as the command prints it: canonical, since the stores are
/// deduplicated by canonical path and that is what it reports.
fn shown(path: &Path) -> String {
    std::fs::canonicalize(path)
        .expect("the path exists")
        .to_string_lossy()
        .into_owned()
}

/// A registry line for `socket` -> `db`.
fn register(home: &Path, socket: &str, db: &Path) {
    let registry = home.join(".config/gear/targets.tsv");
    std::fs::create_dir_all(registry.parent().expect("the registry's directory")).expect("the config dir");
    let mut text = std::fs::read_to_string(&registry).unwrap_or_default();
    text.push_str(&format!("{}\t{}\n", socket, db.display()));
    std::fs::write(&registry, text).expect("the registry");
}

#[test]
fn every_store_on_this_machine_is_listed() {
    let home = crate::scratch::home("projects");
    let shared = home.join(".local/state/gear/gear.db");
    let session_store = home.join(".local/state/gear/s1/gear.db");
    let registered = home.join("elsewhere/reg.db");
    std::fs::create_dir_all(session_store.parent().expect("the session dir")).expect("the state dir");
    std::fs::create_dir_all(registered.parent().expect("the registered dir")).expect("the config dir");
    // The shared store holds a project in a voyage, a session store one
    // without, the registered one none at all.
    seeded(&shared, "a", "default", Some("M2"));
    seeded(&session_store, "b", "s1", None);
    Store::open(&registered).expect("an empty store");
    // The registry names the registered store, names the session store a
    // second time, and names a file that is not there.
    register(&home, "/sock/a", &registered);
    register(&home, "/sock/a-same", &session_store);
    register(&home, "/sock/gone", &home.join("gone/gear.db"));

    let out = projects(&home, &["admin", "projects"]);

    // Every existing store once, in path order.
    let mut expected = vec![shown(&shared), shown(&session_store), shown(&registered)];
    expected.sort();
    assert_eq!(headers(&out), expected);
    let text = String::from_utf8_lossy(&out.stdout);
    // Each store's projects under it, with the voyage (or `-`).
    let shared_block = format!(
        "{}\n  a · session default · config v1 · voyage M2",
        shown(&shared)
    );
    assert!(text.contains(&shared_block), "the shared store said:\n{text}");
    let session_block = format!(
        "{}\n  b · session s1 · config v1 · voyage -",
        shown(&session_store)
    );
    assert!(text.contains(&session_block), "the session store said:\n{text}");
    let registered_block = format!("{}\n  (no projects)", shown(&registered));
    assert!(text.contains(&registered_block), "the registered store said:\n{text}");

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn json_names_every_store_its_projects_and_the_error() {
    let home = crate::scratch::home("projects-json");
    let shared = home.join(".local/state/gear/gear.db");
    let broken = home.join(".local/state/gear/broken/gear.db");
    std::fs::create_dir_all(shared.parent().expect("the state dir")).expect("the state dir");
    std::fs::create_dir_all(broken.parent().expect("the session dir")).expect("the state dir");
    seeded(&shared, "shared-proj", "default", Some("M2"));
    std::fs::write(&broken, "not a database").expect("a broken store");

    let out = projects(&home, &["admin", "projects", "--json"]);

    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let doc: serde_json::Value = serde_json::from_str(&text).expect("the projects as JSON");
    let stores = doc.as_array().expect("the stores");
    let mut expected = vec![shown(&shared), shown(&broken)];
    expected.sort();
    let listed: Vec<String> = stores
        .iter()
        .map(|store| store["store"].as_str().expect("a store path").to_string())
        .collect();
    assert_eq!(listed, expected);
    let by_path = |path: &str| -> &serde_json::Value {
        stores
            .iter()
            .find(|store| store["store"].as_str() == Some(path))
            .expect("the store")
    };
    let good = by_path(&shown(&shared));
    assert_eq!(good["error"], serde_json::Value::Null, "a store that opened has no error");
    let project = &good["projects"].as_array().expect("the projects")[0];
    assert_eq!(project["id"], "shared-proj");
    assert_eq!(project["session"], "default");
    assert_eq!(project["config_version"], 1);
    assert_eq!(project["voyage"], "M2");
    let bad = by_path(&shown(&broken));
    assert!(
        bad["error"].as_str().expect("an error detail").contains("not a database"),
        "the broken store said: {bad}"
    );
    assert_eq!(bad["projects"].as_array().expect("the projects").len(), 0);

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_store_that_does_not_open_is_reported_and_the_others_still_print() {
    let home = crate::scratch::home("projects-broken");
    let broken = home.join(".local/state/gear/broken/gear.db");
    let shared = home.join(".local/state/gear/gear.db");
    std::fs::create_dir_all(broken.parent().expect("the session dir")).expect("the state dir");
    std::fs::create_dir_all(shared.parent().expect("the state dir")).expect("the state dir");
    std::fs::write(&broken, "not a database").expect("a broken store");
    seeded(&shared, "shared-proj", "default", None);

    let out = projects(&home, &["admin", "projects"]);

    assert!(out.status.success(), "a broken store is no failure: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let broken_block = format!("{}\n  cannot read: ", shown(&broken));
    assert!(text.contains(&broken_block), "the broken store said:\n{text}");
    assert!(text.contains("shared-proj"), "the good store still printed:\n{text}");

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn it_writes_nothing_and_starts_no_engine() {
    // A home with no store at all: the command creates no directory, not even
    // that of the default store.
    let home = crate::scratch::home("projects-nothing");
    let out = projects(&home, &["admin", "projects"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "no store, nothing to say");
    assert!(
        !home.join(".local").exists(),
        "the command created {}",
        home.join(".local").display()
    );

    // A home whose store is registered for the Herdr session the caller runs
    // in: a command that started the engine would leave its lock next to the
    // store (L-54).
    let socket = home.join(".config/herdr/sessions/s/hero.sock");
    let store = home.join(".local/state/gear/s/gear.db");
    std::fs::create_dir_all(store.parent().expect("the session dir")).expect("the state dir");
    seeded(&store, "a-proj", "s", None);
    register(&home, &socket.to_string_lossy(), &store);
    let out = crate::scratch::gear(&home, &["admin", "projects"])
        .env("GEAR_DB", &store)
        .env("HERDR_SOCKET_PATH", &socket)
        .output()
        .expect("run gear admin projects");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let dir = store.parent().expect("the store's directory");
    for name in ["engine.lock", "engine.log"] {
        assert!(!dir.join(name).exists(), "the command started the engine: {}", dir.join(name).display());
    }
    // The store it read is still the store it was.
    let mut read = Store::open_read_only(&store).expect("the store");
    let voyage = read.read(|tx| current_voyage(tx, "a-proj")).expect("the voyage");
    assert_eq!(voyage, "", "nothing was written");

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_home_that_is_not_there_lists_nothing() {
    // No HOME: there is nothing to scan, and nothing to complain about.
    let home = crate::scratch::home("projects-nohome");
    let out = crate::scratch::gear(&home, &["admin", "projects", "--json"])
        .env_remove("HOME")
        .output()
        .expect("run gear admin projects");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
    std::fs::remove_dir_all(&home).ok();
}
