//! T-646: the store keeps only the newest plan versions of a project.

use gear_store::{PlanRow, Store, PLAN_VERSIONS_KEPT};

const NOW: i64 = 1_800_000_000;

fn plan(version: i64) -> PlanRow {
    PlanRow {
        project: "p".into(),
        version,
        draft: format!("draft {version}"),
        applied: NOW + version,
        by_seat: Some("p:captain".into()),
        by_gen: Some(1),
    }
}

#[test]
fn only_the_newest_plan_versions_are_kept() {
    let mut s = Store::open_in_memory().unwrap();
    for version in 1..=25 {
        s.write(|tx| tx.insert_plan(&plan(version))).unwrap();
    }
    assert_eq!(PLAN_VERSIONS_KEPT, 20);
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 25);
    assert_eq!(s.read(|tx| tx.oldest_plan_version("p")).unwrap(), Some(6));
    assert!(s.read(|tx| tx.get_plan("p", 3)).unwrap().is_none());
    assert!(s.read(|tx| tx.get_plan("p", 5)).unwrap().is_none());
    for version in 6..=25 {
        let stored = s.read(|tx| tx.get_plan("p", version)).unwrap();
        assert_eq!(
            stored.map(|row| row.version),
            Some(version),
            "version {version} is kept"
        );
    }
    // Another project keeps its own history.
    s.write(|tx| {
        tx.insert_plan(&PlanRow {
            project: "q".into(),
            ..plan(1)
        })
    })
    .unwrap();
    assert_eq!(s.read(|tx| tx.oldest_plan_version("q")).unwrap(), Some(1));
    assert_eq!(s.read(|tx| tx.oldest_plan_version("r")).unwrap(), None);
}

#[test]
fn every_project_is_pruned_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prune.db");
    Store::open(&path).unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for version in 1..=30 {
            conn.execute(
                "INSERT INTO plans (project, version, draft, applied, by_seat, by_gen)
                 VALUES ('q', ?1, ?2, ?3, 'q:captain', 1)",
                rusqlite::params![version, format!("draft {version}"), NOW + version],
            )
            .unwrap();
        }
        for version in 1..=5 {
            conn.execute(
                "INSERT INTO plans (project, version, draft, applied, by_seat, by_gen)
                 VALUES ('r', ?1, ?2, ?3, 'r:captain', 1)",
                rusqlite::params![version, format!("draft {version}"), NOW + version],
            )
            .unwrap();
        }
    }
    let mut s = Store::open(&path).unwrap();
    assert_eq!(s.write(|tx| tx.prune_plan_versions()).unwrap(), 10);
    assert_eq!(s.read(|tx| tx.oldest_plan_version("q")).unwrap(), Some(11));
    assert_eq!(s.read(|tx| tx.plan_version("q")).unwrap(), 30);
    assert!(s.read(|tx| tx.get_plan("q", 10)).unwrap().is_none());
    assert!(s.read(|tx| tx.get_plan("q", 11)).unwrap().is_some());
    assert_eq!(s.read(|tx| tx.oldest_plan_version("r")).unwrap(), Some(1));
    assert_eq!(s.write(|tx| tx.prune_plan_versions()).unwrap(), 0);
}
