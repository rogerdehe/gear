//! Contract for a contract update reaching a lane mid-attempt (T-8): `plan
//! apply` marks the in-progress attempt, the engine delivers one line, and the
//! lane's next `gear` view adopts the new version.

use gear_app::draft::{parse, Draft};
use gear_app::home;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::tasks::{adopt_contract, on_delivery_confirmed, start_attempt, submit_result};
use gear_app::Env;
use gear_model::{AttemptState, GearCtx, SeatRole, TaskState};
use gear_store::{testing, ContractRow, Store, TaskRow};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";
const LANE_PANE: &str = "w1:p2";

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", NOW)?;
        tx.insert_task(&TaskRow {
            project: "p".into(),
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
            updated: NOW,
        })?;
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-1".into(),
            version: 1,
            goal: "g1".into(),
            accept: "a1".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: NOW,
        })?;
        Ok(())
    })
    .unwrap();
    s
}

/// T-1 with an attempt that is working (Started).
fn working() -> Store {
    let mut s = world();
    s.write(|tx| {
        start_attempt(tx, "p", "T-1", "p:heat", 1, NOW)?;
        on_delivery_confirmed(tx, "p", "T-1", 1, NOW)
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None },
    )
}

fn lane() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "heat".into(), generation: 1 }),
            pane_id: Some(LANE_PANE.into()),
            admin: false,
        },
        Fingerprint { pane_id: LANE_PANE.into(), kind: Some("opencode".into()), session_ref: None },
    )
}

/// Apply a draft that changes T-1's goal and acceptance (a contract bump).
fn bump_at(
    s: &mut Store,
    base: i64,
    goal: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let text = format!(
        "<!-- gear-plan project=p base={base} -->\n# Queue\n## T-1 [must] lane=heat\ngoal: {goal}\naccept: a2\ndeps: -\n"
    );
    let draft: Draft = parse(&text).unwrap();
    let (env, fp) = cap();
    apply(s, &env, Some(&fp), &draft, &text, rid, NOW + 10)
}

/// Apply a draft that changes T-1's goal and acceptance (contract v2).
fn bump(s: &mut Store, rid: &str) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    bump_at(s, 0, "g2", rid)
}

#[test]
fn a_bump_marks_the_attempt_that_is_working() {
    let mut s = working();
    let r = bump(&mut s, "r1").unwrap();
    assert_eq!(r.contract_changed, vec![("T-1".to_string(), 2)]);
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(
        (a.adopted_version, a.pending_update, a.state),
        (1, Some(2), AttemptState::Started),
        "the attempt keeps working to v1 until it looks, and knows v2 is waiting"
    );
}

#[test]
fn a_bump_of_a_handed_in_attempt_marks_nothing() {
    let mut s = working();
    s.write(|tx| submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", NOW + 5))
        .unwrap();
    bump(&mut s, "r1").unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(
        (a.adopted_version, a.pending_update, a.state),
        (1, None, AttemptState::Submitted),
        "a handed-in attempt is reviewed under the version rule, not nudged"
    );
}

#[test]
fn the_view_adopts_the_pending_version_once() {
    let mut s = working();
    bump(&mut s, "r1").unwrap();

    // The page shows the version the lane will work to.
    let (env, fp) = lane();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 20).unwrap();
    assert!(page.text.contains("contract v2"), "{}", page.text);
    assert_eq!(page.view.shown_contract, Some(2));

    // Recording the printed page adopts it.
    home::record_view(&mut s, &page.view).unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.adopted_version, a.pending_update), (2, None));

    // A second call has nothing left to adopt.
    let adopted = s.read(|tx| adopt_contract(tx, "p", "T-1", 1, 2, NOW + 30)).unwrap();
    assert_eq!(adopted, None);
}

/// R1 (review): a page adopts exactly the version it showed. A newer update
/// that landed while the page was being printed stays pending for its own
/// notice — it is never acknowledged unseen.
#[test]
fn a_page_adopts_only_the_version_it_showed() {
    let mut s = working();
    bump(&mut s, "r1").unwrap();
    let (env, fp) = lane();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 20).unwrap();
    assert_eq!(page.view.shown_contract, Some(2));

    // The captain bumps again before the page is recorded.
    bump_at(&mut s, 1, "g3", "r2").unwrap();

    home::record_view(&mut s, &page.view).unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(
        (a.adopted_version, a.pending_update),
        (2, Some(3)),
        "the lane saw v2; v3 is still waiting for its own notice"
    );

    // The next page shows v3 and adopts it.
    let page = home::build(&mut s, &env, Some(&fp), NOW + 30).unwrap();
    assert_eq!(page.view.shown_contract, Some(3));
    home::record_view(&mut s, &page.view).unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.adopted_version, a.pending_update), (3, None));
}

/// R2 (review): a handed-in attempt keeps the contract it was submitted under,
/// whatever the home page does afterwards.
#[test]
fn a_handed_in_attempt_keeps_its_submitted_contract() {
    let mut s = working();
    bump(&mut s, "r1").unwrap();
    // The lane hands in under v1 before it reads the update (the submission is
    // valid: v1 is what the attempt worked to).
    s.write(|tx| submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", NOW + 15))
        .unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.state, a.adopted_version, a.pending_update), (AttemptState::Submitted, 1, None));

    let (env, fp) = lane();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 20).unwrap();
    assert_eq!(page.view.shown_contract, Some(1), "the page shows the submitted contract");
    home::record_view(&mut s, &page.view).unwrap();
    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!((a.adopted_version, a.pending_update), (1, None), "the candidate stays bound to v1");
}

/// R2 (review): a page built before the hand-in and recorded after it adopts
/// nothing either.
#[test]
fn a_page_built_before_the_hand_in_adopts_nothing_after_it() {
    let mut s = working();
    bump(&mut s, "r1").unwrap();
    let (env, fp) = lane();
    let page = home::build(&mut s, &env, Some(&fp), NOW + 20).unwrap();
    assert_eq!(page.view.shown_contract, Some(2));

    s.write(|tx| submit_result(tx, "p", "T-1", 1, 1, "abc1234", "Report: r", NOW + 21))
        .unwrap();
    home::record_view(&mut s, &page.view).unwrap();

    let a = s.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
    assert_eq!(
        (a.state, a.adopted_version, a.pending_update),
        (AttemptState::Submitted, 1, None)
    );
}
