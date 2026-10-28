//! Branch names are configuration (T-252).
//!
//! A project names the branch a task's work goes on and the branch it is
//! reviewed on in its `[conventions]`, as templates: `{project}`, `{lane}` and
//! `{n}` are filled in. The constants here are the names GEAR itself uses, and
//! the defaults of a project that says nothing — never the only way (naming is
//! a team's own habit).
//!
//! Pure: no I/O, no store, so the engine, the app and the forge all expand the
//! same templates the same way.

/// The default task branch: the lane's own branch for the task.
pub const TASK_BRANCH: &str = "{lane}/t{n}";

/// The default review branch of a task.
pub const REVIEW_BRANCH: &str = "review/t{n}";

/// `template` with `{project}`, `{lane}` and `{n}` filled in. `project` is the
/// project id, `lane` the lane name and `task` the task id (`T-40` → `{n}` is
/// `40`).
pub fn expand(template: &str, project: &str, lane: &str, task: &str) -> String {
    template
        .replace("{project}", project)
        .replace("{lane}", lane)
        .replace("{n}", &number(task))
}

/// The number in a task id: the id's trailing run of digits (`T-40` → `40`).
/// An id without digits falls back to the old name shape (lowercased, without
/// dashes), so `{n}` is never empty.
fn number(task: &str) -> String {
    let run: String = task
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if run.is_empty() {
        return task.to_lowercase().replace('-', "");
    }
    run.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_names_gear_uses() {
        assert_eq!(expand(TASK_BRANCH, "gear", "mast", "T-40"), "mast/t40");
        assert_eq!(expand(REVIEW_BRANCH, "gear", "mast", "T-40"), "review/t40");
    }

    #[test]
    fn a_template_names_the_project_and_the_lane() {
        assert_eq!(
            expand("{project}/{lane}/t{n}", "gear", "mast", "T-252"),
            "gear/mast/t252"
        );
        assert_eq!(expand("task/{project}-{n}", "gear", "mast", "T-7"), "task/gear-7");
    }

    #[test]
    fn the_number_of_a_task_id_is_its_digits() {
        assert_eq!(number("T-1"), "1");
        assert_eq!(number("T-40"), "40");
        assert_eq!(number("T-"), "t");
    }
}
