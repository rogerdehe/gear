//! A reviewer's verdict on one attempt (design/08, conventions §8).
//!
//! A temp reviewer hands in a verdict with
//! `gear send --kind review --task T-n --attempt <a> --verdict pass|fail "<findings>"`.
//! The verdict is stored as the attempt's review record; **nothing is settled
//! here** — the task, the attempt and the captain's decision stay as they are.
//! The verdict reaches the captain as an action item in their inbox.

use gear_model::{ErrorCode, GearError, GearResult, SeatRole, Ts};
use gear_store::{AttemptStep, ReviewVerdictRow, Tx};

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
    // The verdict has its own time (T-38): the review time ends here, the
    // wait for the captain begins.
    tx.set_attempt_time(
        &review.project,
        &review.task,
        review.attempt,
        AttemptStep::Verdict,
        now,
    )?;
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

/// The findings a reviewer hands in: a path to a file in the reviewer's
/// working directory is read (T-21), anything else is the findings text.
///
/// A review's findings must live in the store, not only in a file on a review
/// branch: the lane doing the rework has to read them from GEAR whatever
/// happens to the reviewer, its worktree or its branch. A path that names no
/// file is `BAD_FORMAT` and nothing is stored.
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
        return read_findings(path, trimmed);
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

/// The text of a findings file, bounded: at most [`FINDINGS_MAX`] bytes are
/// read from the file, so a mistakenly huge file costs a bounded read inside
/// the store's transaction, not the whole file.
fn read_findings(path: &std::path::Path, display: &str) -> GearResult<String> {
    let file = std::fs::File::open(path).map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot read the findings file {display:?}: {e}"),
        )
    })?;
    // The size of the whole file, for the cut note.
    let total = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut buf = Vec::new();
    use std::io::Read;
    file.take(FINDINGS_MAX as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot read the findings file {display:?}: {e}"),
            )
        })?;
    Ok(bounded_findings(&buf, display, total.max(buf.len() as u64)))
}

/// The extensions that name a review file.
const TEXT_EXTENSIONS: &[&str] = &[".md", ".txt", ".markdown", ".rst", ".log"];

/// Is this argument a path to a file the reviewer meant to hand in?
///
/// The rule leans to the loud mistake: an argument that *looks* like a path
/// but names no file is refused (`BAD_FORMAT`, nothing stored), because
/// storing a path instead of the findings is silent — the lane reads a path
/// that does not exist. Prose is kept as prose, so the reviewer can hand in
/// the text itself.
///
/// An argument looks like a path when:
/// - it is written as one: `./x`, `../x`, `/x`, `~/x` — an explicit relative
///   or absolute path, whatever the extension;
/// - it carries a text extension (`notes.md`, `ops/reviews/T-1.md`);
/// - it has a directory part and a file-like last part (`ops/reviews/T-1`,
///   `crates/app/src/lib.rs`);
/// - its last part has a space in it (`ops/reviews/T-1 draft.md`): the first
///   word is already a path, or it names a file itself.
///
/// Prose stays prose: `n/a`, `and/or is broken`, `see ops/REVIEW.md`,
/// `P1 — the header drifts`.
fn looks_like_path(arg: &str) -> bool {
    if arg.is_empty() || arg.len() > 1024 {
        return false;
    }
    if is_explicit_path(arg) {
        return true;
    }
    let mut words = arg.split_whitespace();
    let first = match words.next() {
        Some(first) => first,
        None => return false,
    };
    let last = arg.split_whitespace().next_back().unwrap_or(arg);
    match words.next() {
        // Several words: a path whose last part carries a space, or prose
        // that mentions a path.
        Some(_) => {
            is_path_shaped(first) || (has_text_extension(last) && looks_like_file_name(first))
        }
        // One word: a path with a text extension, or with a directory part
        // and a file-like last part (`n/a` is not a path).
        None => {
            has_text_extension(arg)
                || (arg.rsplit('/').next() != Some(arg) && looks_like_file_name(last))
        }
    }
}

/// `./x`, `../x`, `/x`, `~/x`: written as a path.
fn is_explicit_path(arg: &str) -> bool {
    arg.starts_with("./")
        || arg.starts_with("../")
        || arg.starts_with('/')
        || arg.starts_with("~/")
}

/// Is this one word a path on its own?
fn is_path_shaped(word: &str) -> bool {
    is_explicit_path(word)
        || has_text_extension(word)
        || (word.rsplit('/').next() != Some(word) && looks_like_file_name(word.rsplit('/').next().unwrap_or(word)))
}

/// Does the argument end in a text extension?
fn has_text_extension(arg: &str) -> bool {
    TEXT_EXTENSIONS.iter().any(|ext| arg.ends_with(ext))
}

/// Does this part of a path name a file? A bare word (`a` in `n/a`) does not.
fn looks_like_file_name(part: &str) -> bool {
    !part.is_empty() && (part.contains('.') || part.contains('-') || part.contains('_'))
}

/// Findings are stored whole up to this many bytes; a longer file is cut with
/// a note, so the store never carries a whole book.
pub const FINDINGS_MAX: usize = 64 * 1024;

/// The text of a findings file, bounded. `total` is the size of the whole
/// file (for the note).
fn bounded_findings(bytes: &[u8], path: &str, total: u64) -> String {
    let cut = bytes.len() > FINDINGS_MAX;
    let slice = &bytes[..bytes.len().min(FINDINGS_MAX)];
    let mut text = String::from_utf8_lossy(slice).into_owned();
    if cut {
        text.push_str(&format!(
            "\n[findings cut at {FINDINGS_MAX} bytes of {total}; the whole file is at {path:?}]",
        ));
    }
    text
}

#[cfg(test)]
mod findings_tests {
    use super::*;

    #[test]
    fn a_path_is_read_into_the_store() {
        let dir = crate::testtmp::temp("gear-findings");
        let file = dir.join("T-1.md");
        std::fs::write(&file, "# Findings\n\nP1: the rows drift.\n").unwrap();
        let text = findings_text(file.to_str().unwrap()).unwrap();
        assert!(text.contains("P1: the rows drift"), "{text}");
        assert!(!text.contains("gear-findings"), "the path is not the findings: {text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_path_with_spaces_is_read() {
        let dir = crate::testtmp::temp("gear-findings-sp");
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

    /// The review of T-21 (F1): a missing path is refused whatever it looks
    /// like — with a space in its last part, without an extension, absolute.
    #[test]
    fn a_missing_path_is_refused_whatever_it_looks_like() {
        for arg in [
            "ops/reviews/T-1 draft.md",
            "./ops/reviews/T-1",
            "../reviews/T-1",
            "/tmp/gear-t21/findings",
            "~/reviews/T-1.md",
            "ops/reviews/T-1",
            "T-1 draft.md",
            "notes.md",
        ] {
            let err = findings_text(arg).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadFormat, "{arg}");
            assert!(err.detail.contains("no findings file"), "{}", err.detail);
        }
    }

    /// Prose is prose: the reviewer may hand in the findings as text.
    #[test]
    fn prose_is_not_a_path() {
        for arg in [
            "n/a",
            "and/or is broken",
            "see ops/REVIEW.md for the rules",
            "P1 — the header drifts",
            "P1: the row drifts right",
            "the rows drift right",
            "TODO/FIXME",
        ] {
            assert_eq!(findings_text(arg).unwrap(), arg, "{arg}");
        }
    }

    #[test]
    fn a_long_file_is_cut_with_a_note() {
        let text = bounded_findings(&vec![b'x'; FINDINGS_MAX + 10], "big.md", FINDINGS_MAX as u64 + 10);
        assert!(text.ends_with("the whole file is at \"big.md\"]"), "{text}");
        assert!(text.contains(&format!("of {}", FINDINGS_MAX as u64 + 10)), "{text}");
    }
}
