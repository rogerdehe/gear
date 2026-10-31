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

/// The writer's completion marker: the line a hand-in comment carries, naming
/// the **lane** that finished its part (T-98).
///
/// `marker::hand_in("gear:base", 1)` →
/// `<!-- ORCHESTRATE:COMPLETION agent=gear:base rounds=1 -->`
pub fn hand_in(agent: &str, rounds: u32) -> String {
    format!("<!-- ORCHESTRATE:COMPLETION agent={agent} rounds={rounds} -->")
}

/// The reviewer's completion marker: the line a verdict (or the landing that
/// carried it) carries, naming the **temp seat that judged the work** (T-98).
///
/// `marker::completion("review-4", 2, Some("pass"))` →
/// `<!-- ORCHESTRATE:COMPLETION reviewer=review-4 rounds=2 ci=pass -->`
///
/// `ci` is the project's own gate, and it is written only when it is known:
/// `Some("pass")` at the merge that ran it, `None` everywhere else.
pub fn completion(reviewer: &str, rounds: u32, ci: Option<&str>) -> String {
    match ci {
        Some(ci) => {
            format!("<!-- ORCHESTRATE:COMPLETION reviewer={reviewer} rounds={rounds} ci={ci} -->")
        }
        None => format!("<!-- ORCHESTRATE:COMPLETION reviewer={reviewer} rounds={rounds} -->"),
    }
}
