//! Contract for gear_app::plan::render_task_packet (T-53) and the rework
//! round of a packet (T-118).

use gear_app::plan::render_task_packet;
use gear_app::tasks::{on_delivery_confirmed, settle_review, start_attempt, submit_result};
use gear_model::{ErrorCode, SeatRole, TaskState};
use gear_store::{testing, ContractRow, ReviewVerdictRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
/// The candidate of the first round — the one that failed.
const CANDIDATE: &str = "aaa1111";
/// The findings of the failed review, with the ids the reviewer named as
/// blocking (T-23).
const FINDINGS: &str = "F1: the rework is missing\nF2: the wording of a comment";

fn world(attempt: bool) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", T0)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
        tx.insert_task(&TaskRow { repo: None, base: None, gate: None, rework_estimate_min: None, project: "p".into(), id: "T-7".into(), rank: 1, state: TaskState::Ready, priority: "must".into(),
            lane: Some("heat".into()), milestone: None, required: false, contract_version: 1, deps: vec![], followup_of: None,
            wait_on: None, wait_cond: None, rework_note: None, estimate_min: None,  issue: None,updated: T0 })?;
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 1, goal: "Fix the login crash".into(),
            accept: "repro passes".into(), evidence_files: vec!["notes/T-7.md".into()], evidence_due_min: Some(60), body: String::new(), created: T0, mechanical: None, })?;
        if attempt { start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?; }
        Ok(())
    }).unwrap();
    s
}

#[test]
fn packet_has_contract_and_copyable_commands() {
    let mut s = world(true);
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(text.ends_with("You implement; you never design. If anything here is unclear or contradicts the design or a message, stop and send gear send --kind question (Question / Options / Context / Default) at once, then go on with your default. Commit and push this branch at least every 15 minutes.\n"));
    for needle in [
        "# T-7 — Fix the login crash",
        "Lane: heat · Contract: v1 · State: ready",
        "## Goal", "## Acceptance", "repro passes", "## Evidence", "notes/T-7.md",
        "gear send --kind progress",
        "gear send --kind result --task T-7 --attempt 1 --contract 1 --candidate <commit> \"Report: <path>\"",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in\n{text}");
    }
    assert!(!text.contains("Pending update"));
}

/// T-168 (design §6.2): the packet says what the round's budget is and
/// what a hand-in past it must carry.
#[test]
fn the_packet_says_the_budget_and_the_friction_flag() {
    let mut s = world(true);
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(
        text.contains("Budget: 30 min. Past it, your hand-in must carry --friction"),
        "{text}"
    );
    // The packet lists what belongs in a friction report (design §6.1).
    assert!(text.contains("a permission you lacked"), "{text}");
}

/// T-257: the report goes to the voyage's reports folder, one folder per
/// voyage; a task without a milestone keeps the free `<path>`.
#[test]
fn the_report_path_is_the_voyages_reports_folder() {
    let mut s = world(true);
    s.write(|tx| {
        let mut task = tx.get_task("p", "T-7")?.unwrap();
        task.milestone = Some("M3".into());
        tx.update_task(&task)
    })
    .unwrap();
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(
        text.contains("--candidate <commit> \"Report: gear/voyages/M3/reports/T-<n>.md\""),
        "{text}"
    );

    // Without a milestone the lane may report where it likes.
    let mut s = world(true);
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(text.contains("--candidate <commit> \"Report: <path>\""), "{text}");
}

#[test]
fn a_task_that_adopted_its_current_version_has_no_pending_section() {
    let mut s = world(true);
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(!text.contains("## Pending update"), "{text}");
    assert!(!text.contains("Run `gear` (no arguments) to adopt it"), "{text}");
}

#[test]
fn pending_update_and_missing_task() {
    let mut s = world(true);
    s.write(|tx| {
        tx.insert_contract(&ContractRow { project: "p".into(), task: "T-7".into(), version: 2, goal: "Fix it better".into(),
            accept: "a2".into(), evidence_files: vec![], evidence_due_min: None, body: String::new(), created: T0 + 1, mechanical: None, })?;
        let mut t = tx.get_task("p", "T-7")?.unwrap();
        t.contract_version = 2;
        tx.update_task(&t)
    }).unwrap();
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(text.contains("# T-7 — Fix the login crash"), "rendered from the adopted v1:\n{text}");
    assert!(text.contains("## Pending update: v2"), "{text}");
    // T-378: the pending version's own wording, not only its number.
    assert!(text.contains("Goal (v2):\nFix it better\n"), "{text}");
    assert!(text.contains("Acceptance (v2):\na2\n"), "{text}");
    assert!(
        text.contains("Run `gear` (no arguments) to adopt it, then work and hand in to v2.\n"),
        "{text}"
    );
    assert_eq!(s.read(|tx| render_task_packet(tx, "p", "T-9")).unwrap_err().code, ErrorCode::NoTarget);
    let mut s = world(false);
    assert!(s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap().contains("--attempt <n>"));
}

/// T-272: a mechanical task's packet says the review is skipped and that the
/// merge gate is the only check; another task's packet says nothing of it.
#[test]
fn a_mechanical_task_says_the_gate_is_the_only_check() {
    let mut s = world(true);
    let plain = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(!plain.contains("Review: none"), "{plain}");

    // The task's contract v2 carries the reason; the lane works to it.
    s.write(|tx| {
        tx.insert_contract(&ContractRow {
            project: "p".into(),
            task: "T-7".into(),
            version: 2,
            goal: "Fix the login crash".into(),
            accept: "repro passes".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0 + 1,
            mechanical: Some("delete three dead fns".into()),
        })?;
        let mut t = tx.get_task("p", "T-7")?.unwrap();
        t.contract_version = 2;
        tx.update_task(&t)?;
        // The open attempt works to the new version: the packet renders from
        // the version it adopted.
        let mut attempt = tx.get_attempt("p", "T-7", 1)?.unwrap();
        attempt.contract_version = 2;
        attempt.adopted_version = 2;
        tx.update_attempt(&attempt)
    })
    .unwrap();

    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(
        text.contains(
            "Review: none — mechanical (delete three dead fns): the merge gate is the only check, \
             so run its command before you hand in."
        ),
        "{text}"
    );
}

/// The world of a rework round (T-118): T-7 was handed in as `CANDIDATE`, the
/// review failed on the blocking finding F1, and the lane started attempt 2.
fn rework_world() -> Store {
    let mut s = world(false);
    s.write(|tx| {
        start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
        on_delivery_confirmed(tx, "p", "T-7", 1, T0)?;
        submit_result(tx, "p", "T-7", 1, 1, CANDIDATE, "Report: ops/reports/T-7.md", T0 + 1)?;
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-7".into(),
            attempt: 1,
            passed: false,
            findings: FINDINGS.into(),
            by_seat: "p:r-1".into(),
            created: T0 + 2,
        })?;
        tx.set_review_blocking("p", "T-7", 1, "F1", T0 + 2)?;
        settle_review(tx, "p", "T-7", 1, T0 + 3)?;
        // The lane works on the rework: attempt 2 is open.
        start_attempt(tx, "p", "T-7", "p:heat", 1, T0 + 4)?;
        Ok(())
    })
    .unwrap();
    s
}

/// T-118: the packet of a rework round leads with the captain's rework text
/// and demotes the older report to history. A lane that read the first
/// round's report, found nothing to fix and handed the same candidate in
/// again (trial B02-06) never got this far.
#[test]
fn a_rework_packet_leads_with_the_rework_and_keeps_the_old_review_as_history() {
    let mut s = rework_world();
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();

    // The rework comes first, before the contract.
    let rework_at = text.find("## Rework (attempt 2, from the review of attempt 1)")
        .unwrap_or_else(|| panic!("no rework section:\n{text}"));
    let goal_at = text.find("## Goal").expect("a goal section");
    assert!(rework_at < goal_at, "the rework leads:\n{text}");
    // The captain's rework text is in it: only the blocking finding.
    let rework = &text[rework_at..goal_at];
    assert!(rework.contains("F1: the rework is missing"), "{rework}");
    assert!(!rework.contains("F2"), "the logged finding is not a rework: {rework}");

    // The older report is history, under its own heading.
    assert!(text.contains("## Earlier reviews (history)"), "{text}");
    assert!(!text.contains("\n## Review\n"), "the old heading is gone:\n{text}");
    let history_at = text.find("## Earlier reviews (history)").unwrap();
    assert!(history_at > rework_at, "the history follows the rework:\n{text}");
    assert!(text[history_at..].contains("attempt 1 · fail · p:r-1"), "{text}");

    // The hand-in command names the attempt of this round.
    assert!(
        text.contains("gear send --kind result --task T-7 --attempt 2 --contract 1 --candidate <commit>"),
        "{text}"
    );
}

/// A first hand-in is not a rework: the packet keeps the plain review
/// heading (T-118).
#[test]
fn a_first_round_packet_has_no_rework_section() {
    let mut s = world(true);
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(!text.contains("## Rework"), "{text}");
    assert!(!text.contains("## Earlier reviews (history)"), "{text}");
}

/// A finished task is not in a rework round any more: once a later attempt
/// passed, the packet keeps the plain review history (T-118).
#[test]
fn a_finished_task_has_no_rework_section() {
    let mut s = rework_world();
    s.write(|tx| {
        on_delivery_confirmed(tx, "p", "T-7", 2, T0 + 5)?;
        submit_result(tx, "p", "T-7", 2, 1, "bbb2222", "Report: ops/reports/T-7.md", T0 + 5)?;
        tx.insert_review_verdict(&ReviewVerdictRow {
            project: "p".into(),
            task: "T-7".into(),
            attempt: 2,
            passed: true,
            findings: "the rework is there".into(),
            by_seat: "p:r-1".into(),
            created: T0 + 6,
        })?;
        settle_review(tx, "p", "T-7", 2, T0 + 7)
    })
    .unwrap();
    let text = s.read(|tx| render_task_packet(tx, "p", "T-7")).unwrap();
    assert!(!text.contains("## Rework"), "{text}");
    assert!(text.contains("## Review"), "the plain review history stays:\n{text}");
}
