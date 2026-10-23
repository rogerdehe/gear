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

/// The findings a reviewer hands in: a path to a file in the reviewer's
/// working directory is read (T-21), anything else is the findings text.
///
/// A review's findings must live in the store, not only in a file on a review
/// branch: the lane doing the rework has to read them from GEAR whatever
/// happens to the reviewer, its worktree or its branch. A path that does not
/// exist is `BAD_FORMAT` and nothing is stored.
pub fn findings_text(arg: &str) -> GearResult<String> {
    let trimmed = arg.trim();
    // A path names one file: one line. Anything with line breaks is text, even
    // if it happens to contain a slash.
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return Ok(arg.to_string());
    }
    // A file that exists is read — spaces in its name do not matter. Its text
    // is what the store keeps, so the rework lane never needs the reviewer's
    // worktree or branch.
    let path = std::path::Path::new(trimmed);
    if path.is_file() {
        let bytes = std::fs::read(path).map_err(|e| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot read the findings file {trimmed:?}: {e}"),
            )
        })?;
        return Ok(bounded_findings(&bytes, trimmed));
    }
    // A path that names no file is a mistake, not findings text: the reviewer
    // meant to hand in a file. Nothing is stored (`BAD_FORMAT`).
    if looks_like_path(trimmed) {
        return Err(GearError::new(
            ErrorCode::BadFormat,
            format!(
                "no findings file {trimmed:?} in the reviewer's working directory: hand in the text itself or a path that exists"
            ),
        ));
    }
    Ok(arg.to_string())
}

/// A single-line argument that names a file even though no such file exists:
/// a repository path ending in a text extension (`ops/reviews/T-1.md`) or a
/// bare `*.md`/`*.txt` name. A word such as `n/a` is not a path.
fn looks_like_path(arg: &str) -> bool {
    !arg.is_empty()
        && arg.len() <= 1024
        && !arg.contains(char::is_whitespace)
        && (arg.ends_with(".md") || arg.ends_with(".txt") || arg.ends_with(".markdown"))
}

/// Findings are stored whole up to this many bytes; a longer file is cut with
/// a note, so the store never carries a whole book.
pub const FINDINGS_MAX: usize = 64 * 1024;

/// The text of a findings file, bounded.
fn bounded_findings(bytes: &[u8], path: &str) -> String {
    let cut = bytes.len() > FINDINGS_MAX;
    let slice = &bytes[..bytes.len().min(FINDINGS_MAX)];
    let mut text = String::from_utf8_lossy(slice).into_owned();
    if cut {
        text.push_str(&format!(
            "\n[findings cut at {FINDINGS_MAX} bytes of {}; the whole file is at {path:?}]",
            bytes.len()
        ));
    }
    text
}

#[cfg(test)]
mod findings_tests {
    use super::*;

    #[test]
    fn a_path_is_read_into_the_store() {
        let dir = std::env::temp_dir().join(format!("gear-findings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("T-1.md");
        std::fs::write(&file, "# Findings\n\nP1: the rows drift.\n").unwrap();
        let text = findings_text(file.to_str().unwrap()).unwrap();
        assert!(text.contains("P1: the rows drift"), "{text}");
        assert!(!text.contains("gear-findings"), "the path is not the findings: {text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_path_with_spaces_is_read() {
        let dir = std::env::temp_dir().join(format!("gear-findings-sp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("T-1 draft.md");
        std::fs::write(&file, "P2: the header is stale.\n").unwrap();
        let text = findings_text(file.to_str().unwrap()).unwrap();
        assert!(text.contains("P2: the header is stale"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn text_is_kept_and_a_missing_file_is_bad_format() {
        assert_eq!(findings_text("the rows drift right").unwrap(), "the rows drift right");
        assert_eq!(findings_text("P1 — x\nP2 — y").unwrap(), "P1 — x\nP2 — y");
        assert_eq!(findings_text("n/a").unwrap(), "n/a");
        let err = findings_text("ops/reviews/T-404.md").unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat);
        assert!(err.detail.contains("T-404.md"), "{}", err.detail);
    }

    #[test]
    fn a_long_file_is_cut_with_a_note() {
        let text = bounded_findings(&vec![b'x'; FINDINGS_MAX + 10], "big.md");
        assert!(text.ends_with("the whole file is at \"big.md\"]"), "{text}");
    }
}
