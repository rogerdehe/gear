//! The GitHub side of the plan (T-79): the config's mode and the `## NEW`
//! block an issue becomes.
//!
//! `gear plan import-issues` (T-81) prints the blocks; the captain pastes
//! them into the voyage's plan and fills in the lane, the milestone and the
//! estimate. The plan stays the one source of truth: an issue is an entry,
//! never a second queue.

use crate::draft::Priority;

/// The `## NEW` block one issue becomes (T-79).
///
/// - the **goal** is the issue's title;
/// - the **accept** is the first line of its body (or a line naming the
///   issue when the body is empty);
/// - the **priority** comes from its labels: `priority/now|must|should|could|
///   wont`, `must` when none names one;
/// - `lane=<lane>`, `milestone=<M>` and `estimate: 30` are the captain's to
///   fill in: an issue says what is wanted, not who does it.
///
/// The last line ties the task to the issue (`issue: #12`), so a later apply
/// keeps the link and the engine can close the issue when the merge lands.
pub fn issue_block(issue: &gear_forge::Issue) -> String {
    let priority = issue
        .labels
        .iter()
        .find_map(|label| Priority::from_label(label))
        .unwrap_or(Priority::Must);
    let accept = first_line(&issue.body).unwrap_or_else(|| {
        format!(
            "the request of issue #{number} is met",
            number = issue.number
        )
    });
    format!(
        "## NEW [{priority}] lane=<lane> milestone=<M>\n\
         goal: {goal}\n\
         accept: {accept}\n\
         estimate: 30\n\
         deps: -\n\
         issue: #{number}\n",
        priority = priority.as_str(),
        goal = one_line(&issue.title),
        number = issue.number,
    )
}

/// The first non-empty line of `text`, flattened to one line and bounded.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(one_line)
}

/// One line for a plan field: whitespace collapsed, and long text cut at a
/// word boundary (the captain rewrites it; the plan's fields are one line).
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cap = 200usize;
    if flat.chars().count() <= cap {
        return flat;
    }
    let mut cut: String = flat.chars().take(cap).collect();
    if let Some(at) = cut.rfind(' ') {
        cut.truncate(at);
    }
    cut.push('…');
    cut
}
