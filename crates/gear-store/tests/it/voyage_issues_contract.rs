//! T-645: the store records a voyage's labeled issues.

use gear_store::{Store, VoyageIssueRow};

const T0: i64 = 1_800_000_000;

fn row(number: i64, title: &str, labeled: i64, last_seen: i64) -> VoyageIssueRow {
    VoyageIssueRow {
        project: "p".into(),
        number,
        milestone: "M2".into(),
        title: title.into(),
        body_hash: "hash".into(),
        labeled,
        first_seen: T0,
        last_seen,
    }
}

#[test]
fn a_voyage_issue_is_upserted_and_read_by_milestone() {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        tx.upsert_voyage_issue(&row(12, "the first", 1, T0))?;
        tx.upsert_voyage_issue(&row(7, "the second", 1, T0))
    })
    .unwrap();
    let issues = s.read(|tx| tx.voyage_issues("p", "M2")).unwrap();
    assert_eq!(
        issues.iter().map(|i| i.number).collect::<Vec<_>>(),
        vec![7, 12],
        "ordered by number"
    );
    assert_eq!(issues[1].title, "the first");
    assert_eq!(issues[1].labeled, 1);
    assert_eq!(issues[1].first_seen, T0);
    assert_eq!(issues[0].milestone, "M2");
    assert!(s.read(|tx| tx.voyage_issues("p", "M3")).unwrap().is_empty());
}

#[test]
fn a_second_upsert_updates_the_row_and_keeps_the_first_seen() {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| tx.upsert_voyage_issue(&row(12, "the first", 1, T0)))
        .unwrap();
    s.write(|tx| {
        tx.upsert_voyage_issue(&VoyageIssueRow {
            title: "the renamed".into(),
            body_hash: "other".into(),
            labeled: 0,
            last_seen: T0 + 60,
            ..row(12, "the first", 1, T0)
        })
    })
    .unwrap();
    let issues = s.read(|tx| tx.voyage_issues("p", "M2")).unwrap();
    assert_eq!(issues.len(), 1, "one row per issue");
    assert_eq!(issues[0].title, "the renamed");
    assert_eq!(issues[0].body_hash, "other");
    assert_eq!(issues[0].labeled, 0);
    assert_eq!(issues[0].first_seen, T0);
    assert_eq!(issues[0].last_seen, T0 + 60);
}
