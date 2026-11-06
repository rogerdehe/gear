//! The state of a voyage's labeled GitHub issues (T-645).
//!
//! The board and `gear` both read it, so it lives here: gear-board cannot
//! depend on gear-app. The state is derived from the store — the tasks that
//! carry the issue, their attempts and merges, and the intake item the label
//! scan left — never from GitHub itself.

use gear_model::{GearResult, MsgState, TaskState};
use gear_store::Tx;

/// What the board shows for one labeled issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    /// Every task carrying it is Done and merged.
    Done,
    /// Every task carrying it is Done, but one is not merged yet.
    Verifying,
    /// A task carrying it has an attempt.
    InProgress,
    /// Tasks carry it and none has an attempt yet.
    Planned,
    /// No task carries it and its intake item is resolved: the captain
    /// deferred it.
    Deferred,
    /// No task carries it and its intake item is still open.
    Pending,
    /// The issue no longer carries the voyage's label.
    Unlabeled,
}

/// The state of each issue the label scan recorded for `milestone`, ordered by
/// issue number.
pub fn issue_states(
    tx: &Tx<'_>,
    project: &str,
    milestone: &str,
) -> GearResult<Vec<(i64, IssueState)>> {
    let mut states = Vec::new();
    for issue in tx.voyage_issues(project, milestone)? {
        let state = if issue.labeled == 0 {
            IssueState::Unlabeled
        } else {
            issue_state(tx, project, milestone, issue.number)?
        };
        states.push((issue.number, state));
    }
    Ok(states)
}

/// The state of one labeled issue: the tasks of `milestone` that carry it
/// (cancelled ones left out), else its intake item.
fn issue_state(tx: &Tx<'_>, project: &str, milestone: &str, number: i64) -> GearResult<IssueState> {
    let tasks: Vec<gear_store::TaskRow> = tx
        .tasks_of_voyage(project, milestone)?
        .into_iter()
        .filter(|task| task.issue == Some(number) && task.state != TaskState::Cancelled)
        .collect();
    if tasks.is_empty() {
        let thread = format!("intake:{milestone}:#{number}");
        let deferred = tx
            .thread_messages(project, &thread)?
            .iter()
            .any(|message| message.state == MsgState::Resolved);
        return Ok(if deferred {
            IssueState::Deferred
        } else {
            IssueState::Pending
        });
    }
    let mut all_done = true;
    let mut any_attempt = false;
    let mut any_unmerged = false;
    for task in &tasks {
        if task.state != TaskState::Done {
            all_done = false;
        }
        if !tx.attempts_of_task(project, &task.id)?.is_empty() {
            any_attempt = true;
        }
        if tx
            .attempt_times_of_task(project, &task.id)?
            .iter()
            .all(|(_, times)| times.merged.is_none())
        {
            any_unmerged = true;
        }
    }
    Ok(if all_done {
        if any_unmerged {
            IssueState::Verifying
        } else {
            IssueState::Done
        }
    } else if any_attempt {
        IssueState::InProgress
    } else {
        IssueState::Planned
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::{AttemptState, SeatRole};
    use gear_store::{testing, AttemptRow, NewMessage, Store, TaskRow, VoyageIssueRow};

    const T0: i64 = 1_800_000_000;

    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)
        })
        .unwrap();
        s
    }

    fn labeled(s: &mut Store, number: i64) {
        s.write(|tx| {
            tx.upsert_voyage_issue(&VoyageIssueRow {
                project: "p".into(),
                number,
                milestone: "M2".into(),
                title: format!("issue {number}"),
                body_hash: "hash".into(),
                labeled: 1,
                first_seen: T0,
                last_seen: T0,
            })
        })
        .unwrap();
    }

    fn task(s: &mut Store, id: &str, state: TaskState, issue: i64) {
        s.write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: id.into(),
                rank: 1,
                state,
                priority: "must".into(),
                lane: Some("heat".into()),
                milestone: Some("M2".into()),
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                issue: Some(issue),
                updated: T0,
            })
        })
        .unwrap();
    }

    fn attempt(s: &mut Store, id: &str) {
        s.write(|tx| {
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: id.into(),
                n: 1,
                seat: "p:heat".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })
        })
        .unwrap();
    }

    fn merged(s: &mut Store, id: &str) {
        attempt(s, id);
        s.write(|tx| tx.set_attempt_time("p", id, 1, gear_store::AttemptStep::Merged, T0 + 5))
            .unwrap();
    }

    fn intake(s: &mut Store, number: i64, state: MsgState) {
        s.write(|tx| {
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: format!("intake:M2:#{number}"),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: gear_model::MsgKind::System,
                urgent: false,
                body: "issue".into(),
                reply_to: None,
                needs_ack: false,
                state,
                created: T0,
            })
        })
        .unwrap();
    }

    fn state_of(s: &mut Store, number: i64) -> IssueState {
        s.read(|tx| issue_states(tx, "p", "M2"))
            .unwrap()
            .into_iter()
            .find(|(n, _)| *n == number)
            .map(|(_, state)| state)
            .expect("the issue is recorded")
    }

    #[test]
    fn an_unlabeled_issue_is_unlabeled() {
        let mut s = world();
        labeled(&mut s, 7);
        s.write(|tx| {
            tx.upsert_voyage_issue(&VoyageIssueRow {
                project: "p".into(),
                number: 7,
                milestone: "M2".into(),
                title: "issue 7".into(),
                body_hash: "hash".into(),
                labeled: 0,
                first_seen: T0,
                last_seen: T0,
            })
        })
        .unwrap();
        assert_eq!(state_of(&mut s, 7), IssueState::Unlabeled);
    }

    #[test]
    fn a_done_and_merged_issue_is_done() {
        let mut s = world();
        labeled(&mut s, 7);
        task(&mut s, "T-1", TaskState::Done, 7);
        merged(&mut s, "T-1");
        assert_eq!(state_of(&mut s, 7), IssueState::Done);
    }

    #[test]
    fn a_done_but_unmerged_issue_is_verifying() {
        let mut s = world();
        labeled(&mut s, 7);
        task(&mut s, "T-1", TaskState::Done, 7);
        attempt(&mut s, "T-1");
        assert_eq!(state_of(&mut s, 7), IssueState::Verifying);
    }

    #[test]
    fn an_issue_with_a_started_attempt_is_in_progress() {
        let mut s = world();
        labeled(&mut s, 7);
        task(&mut s, "T-1", TaskState::Running, 7);
        attempt(&mut s, "T-1");
        assert_eq!(state_of(&mut s, 7), IssueState::InProgress);
    }

    #[test]
    fn an_issue_with_tasks_and_no_attempt_is_planned() {
        let mut s = world();
        labeled(&mut s, 7);
        task(&mut s, "T-1", TaskState::Ready, 7);
        assert_eq!(state_of(&mut s, 7), IssueState::Planned);
    }

    #[test]
    fn an_issue_whose_intake_item_is_resolved_is_deferred() {
        let mut s = world();
        labeled(&mut s, 7);
        intake(&mut s, 7, MsgState::Resolved);
        assert_eq!(state_of(&mut s, 7), IssueState::Deferred);
    }

    #[test]
    fn an_issue_with_no_task_and_an_open_intake_item_is_pending() {
        let mut s = world();
        labeled(&mut s, 7);
        intake(&mut s, 7, MsgState::Open);
        assert_eq!(state_of(&mut s, 7), IssueState::Pending);
    }

    #[test]
    fn a_cancelled_task_does_not_carry_the_issue() {
        let mut s = world();
        labeled(&mut s, 7);
        task(&mut s, "T-1", TaskState::Cancelled, 7);
        assert_eq!(state_of(&mut s, 7), IssueState::Pending);
    }
}
