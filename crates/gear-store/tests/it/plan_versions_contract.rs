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
