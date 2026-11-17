//! Commit messages as templates (T-299).
//!
//! A team with commit rules (ticket keys, Conventional Commits) keeps its own
//! form; GEAR's are only the defaults. Pure: every writer — the engine's merge,
//! the reviewer's commit, the captain's plan — fills the same templates the same
//! way, so a project reads one message format everywhere.

/// The default message of a task's merge commit into the integration branch.
pub const MERGE_MESSAGE: &str = "merge({task}): {source}";

/// The default message of a reviewer's commit.
pub const REVIEW_MESSAGE: &str = "review({task}): {what}";

/// The default message of the captain's plan commit.
pub const PLAN_MESSAGE: &str = "{milestone}: {what}";

/// The trailer naming the task a squashed commit lands (M7.3).
pub const TRAILER_TASK: &str = "Gear-Task";

/// The trailer naming the attempt a squashed commit lands (M7.3).
pub const TRAILER_ATTEMPT: &str = "Gear-Attempt";

/// The trailer naming the candidate a squashed commit lands (M7.3).
pub const TRAILER_CANDIDATE: &str = "Gear-Candidate";

/// The trailer naming one commit a squashed commit carries (M7.3).
pub const TRAILER_SOURCE: &str = "Gear-Source";

/// The longest subject a squashed commit's goal is cut to (M7.3).
const SQUASH_GOAL_MAX: usize = 72;

/// The message of a squashed task merge (M7.3): the subject `{task}: {goal}`
/// with the goal's first line cut to a word at 72 characters, the lane's
/// commit subjects as body lines, and the trailers that name the task, the
/// attempt, the candidate and every commit the squash carries. Without a goal
/// the subject is the task id alone (`T-n`), never a placeholder.
pub fn squash_message(
    task: &str,
    attempt: &str,
    goal: &str,
    candidate: &str,
    sources: &[String],
    lines: &[String],
) -> String {
    let first = goal.lines().next().unwrap_or_default().trim();
    let mut out = String::new();
    out.push_str(task);
    if !first.is_empty() {
        out.push_str(": ");
        out.push_str(&cut_at_word(first, SQUASH_GOAL_MAX));
    }
    out.push_str("\n\n");
    if !lines.is_empty() {
        for line in lines {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(TRAILER_TASK);
    out.push_str(": ");
    out.push_str(task);
    out.push('\n');
    out.push_str(TRAILER_ATTEMPT);
    out.push_str(": ");
    out.push_str(attempt);
    out.push('\n');
    out.push_str(TRAILER_CANDIDATE);
    out.push_str(": ");
    out.push_str(candidate);
    for source in sources {
        out.push('\n');
        out.push_str(TRAILER_SOURCE);
        out.push_str(": ");
        out.push_str(source);
    }
    out
}

/// `text` cut to at most `max` characters, at a word boundary: a word that
/// would cross the limit is dropped whole, and trailing whitespace goes.
fn cut_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let cut = match head.rfind(char::is_whitespace) {
        Some(at) => head[..at].trim_end().to_string(),
        None => head,
    };
    cut
}

/// `template` with every `{name}` of `values` replaced by its value.
///
/// Every other text stays as it is, an unknown `{…}` included: a placeholder
/// GEAR does not fill in is the project's to see, not text to drop. A value is
/// inserted literally — it is never scanned for placeholders itself.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            // An unclosed brace is not a placeholder: the rest is literal.
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        match values.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_name_the_task_and_what_it_carries() {
        assert_eq!(
            fill(MERGE_MESSAGE, &[("task", "T-7"), ("source", "origin/mast")]),
            "merge(T-7): origin/mast"
        );
        assert_eq!(
            fill(REVIEW_MESSAGE, &[("task", "T-7"), ("what", "pass")]),
            "review(T-7): pass"
        );
        assert_eq!(
            fill(PLAN_MESSAGE, &[("milestone", "M2"), ("what", "two tasks")]),
            "M2: two tasks"
        );
    }

    #[test]
    fn every_placeholder_is_filled_in() {
        assert_eq!(
            fill(
                "{project}/{task} {source} {attempt} {what} {milestone}",
                &[
                    ("project", "p"),
                    ("task", "T-7"),
                    ("source", "dev"),
                    ("attempt", "2"),
                    ("what", "pass"),
                    ("milestone", "M2"),
                ]
            ),
            "p/T-7 dev 2 pass M2"
        );
    }

    #[test]
    fn a_placeholder_without_a_value_stays_as_it_is() {
        assert_eq!(fill("{task} {who}", &[("task", "T-7")]), "T-7 {who}");
        assert_eq!(
            fill("nothing to fill", &[("task", "T-7")]),
            "nothing to fill"
        );
        assert_eq!(fill("plain", &[]), "plain");
    }

    #[test]
    fn a_value_is_inserted_literally() {
        // The value is the project's text, not a template: it is not expanded.
        assert_eq!(
            fill("{what}", &[("what", "{task}"), ("task", "T-7")]),
            "{task}"
        );
    }

    #[test]
    fn an_unclosed_brace_is_literal() {
        assert_eq!(fill("{task} {oops", &[("task", "T-7")]), "T-7 {oops");
    }

    #[test]
    fn a_squash_names_the_task_the_goal_and_what_it_carries() {
        let sources = vec!["aaa111".to_string(), "bbb222".to_string()];
        let lines = vec![
            "add the parser".to_string(),
            "cover the empty input".to_string(),
        ];
        assert_eq!(
            squash_message(
                "T-7",
                "2",
                "Land the parser\nsecond line is dropped",
                "ccc333",
                &sources,
                &lines
            ),
            "T-7: Land the parser\n\n- add the parser\n- cover the empty input\n\n\
             Gear-Task: T-7\nGear-Attempt: 2\nGear-Candidate: ccc333\n\
             Gear-Source: aaa111\nGear-Source: bbb222"
        );
    }

    #[test]
    fn a_squash_without_lines_has_no_body_block() {
        assert_eq!(
            squash_message("T-7", "1", "one thing", "c0ffee", &[], &[]),
            "T-7: one thing\n\nGear-Task: T-7\nGear-Attempt: 1\n\
             Gear-Candidate: c0ffee"
        );
    }

    #[test]
    fn a_long_goal_is_cut_to_a_word_at_72_characters() {
        // 90 characters: the cut drops the word that crosses 72.
        let goal = "word ".repeat(17) + "tail";
        let message = squash_message("T-7", "1", &goal, "c0ffee", &[], &[]);
        let subject = message.lines().next().expect("the subject");
        assert_eq!(subject, format!("T-7: {}", "word ".repeat(14).trim_end()));
        assert!(subject.len() <= "T-7: ".len() + 72);
    }
}
