//! The ORCHESTRATE markers (T-78): the lines a forge's text carries so that a
//! human and another agent can both tell what happened.
//!
//! The protocol is BMAD-Team's: a marker is one HTML comment, so no markdown
//! renderer shows it, and every field is `key=value` with one word per value
//! (a seat id, an issue number, a state). Two markers exist:
//!
//! - a **claim** opens a task's pull request: who claimed which issue;
//! - a **completion** closes the work in the pull request: who reviewed it,
//!   how many review rounds it took, and what CI said.
//!
//! The text is exact because another agent reads it; the tests pin both
//! lines so the shape cannot drift silently.

/// The claim marker: the line a claim pull request's body carries.
///
/// `marker::claim("base", 12)` →
/// `<!-- ORCHESTRATE:CLAIM agent=base issue=12 -->`
pub fn claim(agent: &str, issue: u64) -> String {
    format!("<!-- ORCHESTRATE:CLAIM agent={agent} issue={issue} -->")
}

/// The completion marker: the line a hand-in comment carries.
///
/// `marker::completion("review-4", 2, "pass")` →
/// `<!-- ORCHESTRATE:COMPLETION reviewer=review-4 rounds=2 ci=pass -->`
///
/// `ci` is the state of the project's own checks — `pass`, `fail`, `running`
/// or `none` when the project has none.
pub fn completion(reviewer: &str, rounds: u32, ci: &str) -> String {
    format!("<!-- ORCHESTRATE:COMPLETION reviewer={reviewer} rounds={rounds} ci={ci} -->")
}
