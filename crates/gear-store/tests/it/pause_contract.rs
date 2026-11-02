//! `gear admin pause`: a paused project leaves the engine's project list, the
//! other projects of the store stay in it, and a resume brings it back.

use gear_store::{ProjectRow, Store};

fn project(id: &str) -> ProjectRow {
    ProjectRow {
        id: id.into(),
        herdr_session: "sess".into(),
        config_toml: String::new(),
        config_version: 1,
        quota_lanes: 1,
        quota_temps: 0,
        quota_includes_captain: false,
        plan_version: 0,
        created: 0,
    }
}

#[test]
fn a_paused_project_leaves_the_engines_list_and_a_resume_brings_it_back() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&project("lawrence"))?;
            tx.insert_project(&project("prometheus"))
        })
        .unwrap();
    let key = gear_store::Tx::project_paused_key("lawrence");
    store
        .write(|tx| tx.set_meta(&key, "1 paused by the user"))
        .unwrap();
    let active = store.read(|tx| tx.active_project_ids()).unwrap();
    assert_eq!(active, vec!["prometheus".to_string()]);
    let all = store.read(|tx| tx.project_ids()).unwrap();
    assert_eq!(all.len(), 2, "a pause registers nothing away");
    assert!(store
        .read(|tx| tx.project_paused("lawrence"))
        .unwrap()
        .is_some());

    store.write(|tx| tx.set_meta(&key, "")).unwrap();
    let active = store.read(|tx| tx.active_project_ids()).unwrap();
    assert_eq!(
        active,
        vec!["lawrence".to_string(), "prometheus".to_string()]
    );
}
