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
}
