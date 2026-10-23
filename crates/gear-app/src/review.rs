//! A reviewer's verdict on one attempt (design/08, conventions §8).
//!
//! A temp reviewer hands in a verdict with
//! `gear send --kind review --task T-n --attempt <a> --verdict pass|fail "<findings>"`.
//! The verdict is stored as the attempt's review record; **nothing is settled
//! here** — the task, the attempt and the captain's decision stay as they are.
//! The verdict reaches the captain as an action item in their inbox.

use gear_model::{ErrorCode, GearError, GearResult, SeatRole, Ts};
use gear_store::{ReviewVerdictRow, Tx};

/// The two verdicts a reviewer may hand in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
}

impl Outcome {
    /// The word the CLI and the packet use.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
        }
    }

    /// Parse the word; anything else is `BadFormat`.
    pub fn parse(text: &str) -> GearResult<Outcome> {
        match text {
            "pass" => Ok(Outcome::Pass),
            "fail" => Ok(Outcome::Fail),
            other => Err(GearError::new(
                ErrorCode::BadFormat,
                format!("unknown verdict {other:?}: expected pass or fail"),
            )),
        }
    }
}

/// The verdict a reviewer hands in, before it is checked against the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub project: String,
    pub task: String,
    pub attempt: i64,
    pub outcome: Outcome,
    /// The findings: a path or the text itself.
    pub findings: String,
    /// The ids of the findings that block (T-23), comma separated. Empty means
    /// every finding is only logged.
    pub blocking: String,
    /// The seat that hands the verdict in.
    pub by_seat: String,
}

/// Store the review verdict of `review` on the attempt's review record.
///
/// Checks, in this order:
/// - the task exists (`NoTarget`);
/// - the attempt exists (`NoTarget`), and its outcome is still open
///   (`BadTransition`): a review of an attempt that is already settled is late
///   and must not create captain work (R-67 P1);
/// - the seat is a **temp** seat (`Forbidden`): a lane or the captain may not
///   hand in a verdict;
/// - the temp seat is bound to **that** task (`NoTarget`): a reviewer bound to
///   another task reviews nothing.
///
/// The same verdict again changes nothing; a different one for the same
/// attempt is `BadTransition` — a record is written once.
pub fn submit_review(tx: &Tx<'_>, review: &Review, now: Ts) -> GearResult<()> {
    if tx.get_task(&review.project, &review.task)?.is_none() {
        return Err(GearError::new(
            ErrorCode::NoTarget,
            format!("no task {} in project {}", review.task, review.project),
        ));
    }
    let attempt = tx
        .get_attempt(&review.project, &review.task, review.attempt)?
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::NoTarget,
                format!(
                    "no attempt {} of task {} in project {}",
                    review.attempt, review.task, review.project
                ),
            )
        })?;
    // The settlement already happened: a late review changes nothing and
    // leaves no action item for the captain.
    if attempt.state.is_terminal() {
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "attempt {} of task {} is {}: its review is settled, a late verdict changes nothing",
                review.attempt,
                review.task,
                attempt.state.as_str()
            ),
        ));
    }
    let seat = tx.get_seat(&review.by_seat)?.ok_or_else(|| {
        GearError::new(
            ErrorCode::NoTarget,
            format!("no seat {} in project {}", review.by_seat, review.project),
        )
    })?;
    if seat.role != SeatRole::Temp {
        return Err(GearError::new(
            ErrorCode::Forbidden,
            format!(
                "seat {} is a {} seat: only a temp reviewer hands in a verdict",
                seat.id,
                seat.role.as_str()
            ),
        ));
    }
    match seat.task_id.as_deref() {
        Some(task) if task == review.task => {}
        Some(task) => {
            return Err(GearError::new(
                ErrorCode::NoTarget,
                format!(
                    "seat {} is bound to task {task}, not to {}",
                    seat.id, review.task
                ),
            ))
        }
        None => {
            return Err(GearError::new(
                ErrorCode::NoTarget,
                format!("seat {} is not bound to a task", seat.id),
            ))
        }
    }

    // One record per attempt: the same verdict again changes nothing, a
    // different one would rewrite a fact the captain may already have seen.
    if let Some(stored) = tx.get_review_verdict(&review.project, &review.task, review.attempt)? {
        if stored.passed == (review.outcome == Outcome::Pass) && stored.findings == review.findings {
            return Ok(());
        }
        return Err(GearError::new(
            ErrorCode::BadTransition,
            format!(
                "attempt {} of task {} already has a review verdict ({})",
                review.attempt,
                review.task,
                if stored.passed { "pass" } else { "fail" }
            ),
        ));
    }

    let row = ReviewVerdictRow {
        project: review.project.clone(),
        task: review.task.clone(),
        attempt: review.attempt,
        passed: review.outcome == Outcome::Pass,
        findings: review.findings.clone(),
        by_seat: review.by_seat.clone(),
        created: now,
    };
    tx.insert_review_verdict(&row)?;
    // The findings that block go back to the lane as rework; the rest are
    // information for the captain (T-23).
    if !review.blocking.trim().is_empty() {
        tx.set_review_blocking(
            &review.project,
            &review.task,
            review.attempt,
            review.blocking.trim(),
            now,
        )?;
    }
    Ok(())
}

/// The line the captain reads in the inbox: the verdict, the attempt it is
/// about and the findings the reviewer wrote.
pub fn headline(review: &Review) -> String {
    format!(
        "review {} on {} attempt {}",
        review.outcome.as_str(),
        review.task,
        review.attempt
    )
}
