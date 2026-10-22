//! Contract for the slice-2 store methods: plans, tasks, contracts (T-33);
//! attempts, verdicts (T-34). Raw reads/writes; state rules live in gear-app.

use gear_model::{AttemptState, ErrorCode, TaskState};
use gear_store::{testing, AttemptRow, ContractRow, PlanRow, Store, TaskRow, VerdictRow};

const NOW: i64 = 1_800_000_000;

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| testing::seed_project(tx, "p", "default", NOW).map(|_| ())).unwrap();
    s
}

fn task(id: &str, rank: i64) -> TaskRow {
    TaskRow {
        project: "p".into(), id: id.into(), rank, state: TaskState::Ready, priority: "must".into(),
        lane: Some("heat".into()), milestone: None, required: true, contract_version: 1,
        deps: vec!["T-0".into()], followup_of: None, wait_on: None, wait_cond: None,
        rework_note: None, updated: NOW,
    }
}

fn attempt(t: &str, n: i64, seat: &str, state: AttemptState) -> AttemptRow {
    AttemptRow {
        project: "p".into(), task: t.into(), n, seat: seat.into(), generation: 1,
        contract_version: 1, adopted_version: 1, pending_update: None, state,
        candidate: None, report: None, safe_point: None, started: NOW, updated: NOW,
    }
}

#[test]
fn plans_version_and_roundtrip() {
    let mut s = store();
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 0);
    let row = PlanRow { project: "p".into(), version: 1, draft: "d".into(), applied: NOW, by_seat: Some("p:captain".into()), by_gen: Some(1) };
    s.write(|tx| tx.insert_plan(&row)).unwrap();
    s.write(|tx| tx.insert_plan(&PlanRow { version: 2, ..row.clone() })).unwrap();
    assert_eq!(s.read(|tx| tx.plan_version("p")).unwrap(), 2);
    assert_eq!(s.read(|tx| tx.get_plan("p", 1)).unwrap(), Some(row.clone()));
    assert_eq!(s.write(|tx| tx.insert_plan(&row)).unwrap_err().code, ErrorCode::BadFormat);
}

#[test]
fn tasks_roundtrip_order_and_update() {
    let mut s = store();
    s.write(|tx| { tx.insert_task(&task("T-2", 2))?; tx.insert_task(&task("T-1", 1))?; tx.insert_task(&task("T-3", 1)) }).unwrap();
    let ids: Vec<String> = s.read(|tx| tx.tasks_of_project("p")).unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, vec!["T-1", "T-3", "T-2"]);
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap(), Some(task("T-1", 1)));
    let mut t = task("T-1", 1);
    t.state = TaskState::Running; t.deps = vec![]; t.rework_note = Some("x".into()); t.updated = NOW + 1;
    s.write(|tx| tx.update_task(&t)).unwrap();
    assert_eq!(s.read(|tx| tx.get_task("p", "T-1")).unwrap(), Some(t));
    assert_eq!(s.write(|tx| tx.update_task(&task("T-9", 1))).unwrap_err().code, ErrorCode::NoTarget);
    assert_eq!(s.write(|tx| tx.insert_task(&task("T-2", 5))).unwrap_err().code, ErrorCode::BadFormat);
}

#[test]
fn contracts_roundtrip() {
    let mut s = store();
    let c = ContractRow { project: "p".into(), task: "T-1".into(), version: 1, goal: "g".into(), accept: "a".into(),
        evidence_files: vec!["notes/x.md".into()], evidence_due_min: Some(60), body: "b".into(), created: NOW };
    s.write(|tx| tx.insert_contract(&c)).unwrap();
    assert_eq!(s.read(|tx| tx.get_contract("p", "T-1", 1)).unwrap(), Some(c.clone()));
    assert_eq!(s.read(|tx| tx.get_contract("p", "T-1", 2)).unwrap(), None);
    assert_eq!(s.write(|tx| tx.insert_contract(&c)).unwrap_err().code, ErrorCode::BadFormat);
}

#[test]
#[ignore = "T-34: un-ignored by the captain when T-34 merges"]
fn attempts_roundtrip_order_and_in_progress() {
    let mut s = store();
    s.write(|tx| {
        tx.insert_attempt(&attempt("T-1", 2, "p:heat", AttemptState::Started))?;
        tx.insert_attempt(&attempt("T-1", 1, "p:heat", AttemptState::Rejected))?;
        tx.insert_attempt(&attempt("T-2", 1, "p:heat", AttemptState::Unknown))?;
        tx.insert_attempt(&attempt("T-3", 1, "p:heat", AttemptState::Submitted))?;
        tx.insert_attempt(&attempt("T-4", 1, "p:wire", AttemptState::Delivered))
    }).unwrap();
    let ns: Vec<i64> = s.read(|tx| tx.attempts_of_task("p", "T-1")).unwrap().into_iter().map(|a| a.n).collect();
    assert_eq!(ns, vec![1, 2]);
    let busy: Vec<(String, i64)> = s.read(|tx| tx.in_progress_attempts_of_seat("p:heat")).unwrap().into_iter().map(|a| (a.task, a.n)).collect();
    assert_eq!(busy, vec![("T-1".to_string(), 2), ("T-2".to_string(), 1)]);
    let mut a = attempt("T-1", 2, "p:heat", AttemptState::Submitted);
    a.candidate = Some("abc".into()); a.adopted_version = 2; a.pending_update = Some(3); a.updated = NOW + 5;
    s.write(|tx| tx.update_attempt(&a)).unwrap();
    assert_eq!(s.read(|tx| tx.get_attempt("p", "T-1", 2)).unwrap(), Some(a));
    assert_eq!(s.write(|tx| tx.update_attempt(&attempt("T-9", 1, "p:heat", AttemptState::Started))).unwrap_err().code, ErrorCode::NoTarget);
    assert_eq!(s.write(|tx| tx.insert_attempt(&attempt("T-2", 1, "p:heat", AttemptState::Started))).unwrap_err().code, ErrorCode::BadFormat);
}

#[test]
#[ignore = "T-34: un-ignored by the captain when T-34 merges"]
fn verdicts_append_and_order() {
    let mut s = store();
    let v = |attempt: i64, passed: bool, created: i64| VerdictRow { project: "p".into(), task: "T-1".into(), attempt,
        contract_version: 1, candidate: "c".into(), passed, evidence: None, by_seat: "p:captain".into(), created };
    s.write(|tx| { tx.insert_verdict(&v(2, true, NOW + 9))?; tx.insert_verdict(&v(1, false, NOW))?; tx.insert_verdict(&v(1, false, NOW)) }).unwrap();
    let got: Vec<(i64, bool)> = s.read(|tx| tx.verdicts_of_task("p", "T-1")).unwrap().into_iter().map(|r| (r.attempt, r.passed)).collect();
    assert_eq!(got, vec![(1, false), (1, false), (2, true)]);
}
