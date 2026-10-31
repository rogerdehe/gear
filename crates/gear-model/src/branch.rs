//! Branch names are configuration (T-252).
//!
//! A project names the branch a task's work goes on, the branch it is reviewed
//! on and the branch of each seat's own worktree in its `[conventions]`, as
//! templates: `{project}`, `{lane}`, `{seat}` and `{n}` are filled in. The
//! constants here are the names GEAR itself uses, and the defaults of a project
//! that says nothing — never the only way (naming is a team's own habit).
//!
//! Pure: no I/O, no store, so the engine, the app and the forge all expand the
//! same templates the same way.

/// The default task branch: the lane's own branch for the task.
pub const TASK_BRANCH: &str = "{lane}/t{n}";

/// The default review branch of a task.
pub const REVIEW_BRANCH: &str = "review/t{n}";

/// The default branch of a seat's own worktree (T-252): the name says whose
/// worktree it is, so Herdr names the worktree `<project>-<seat>`.
pub const SEAT_BRANCH: &str = "{project}/{seat}";

/// `template` with `{project}`, `{lane}` and `{n}` filled in. `project` is the
/// project id, `lane` the lane name and `task` the task id (`T-40` → `{n}` is
/// `40`).
pub fn expand(template: &str, project: &str, lane: &str, task: &str) -> String {
    template
        .replace("{project}", project)
        .replace("{lane}", lane)
        .replace("{n}", &number(task))
}

/// The review branch of a task (T-253, T-438): the project's
/// `[conventions].review_branch` template with `{project}`, `{lane}` and `{n}`
/// filled in — `review/t<n>` by default (conventions 17 §3: the reviewers'
/// branch). The engine hands it to the reviewer and the app checks the push
/// onto it, so both read the same name.
pub fn review_branch(template: &str, project: &str, lane: &str, task: &str) -> String {
    expand(template, project, lane, task)
}

/// `template` with `{project}`, `{seat}` filled in: the branch of a seat's own
/// worktree (T-252). The merge worktree is seat `merge`.
pub fn expand_seat(template: &str, project: &str, seat: &str) -> String {
    template
        .replace("{project}", project)
        .replace("{seat}", seat)
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
        assert_eq!(expand(TASK_BRANCH, "p", "pay", "T-40"), "pay/t40");
        assert_eq!(expand(REVIEW_BRANCH, "p", "l", "T-7"), "review/t7");
    }

    #[test]
    fn a_review_branch_is_the_projects_review_template() {
        assert_eq!(
            review_branch(REVIEW_BRANCH, "p", "heat", "T-40"),
            "review/t40"
        );
        assert_eq!(
            review_branch("b/{project}/review/t{n}", "acme", "heat", "T-40"),
            "b/acme/review/t40"
        );
    }

    #[test]
    fn a_template_names_the_project_and_the_lane() {
        assert_eq!(
            expand("{project}/{lane}/t{n}", "acme", "pay", "T-40"),
            "acme/pay/t40"
        );
        assert_eq!(
            expand("task/{project}-{n}", "gear", "mast", "T-7"),
            "task/gear-7"
        );
    }

    #[test]
    fn a_seat_branch_names_the_project_and_the_seat() {
        assert_eq!(expand_seat(SEAT_BRANCH, "p", "hull"), "p/hull");
        assert_eq!(expand_seat("{project}-{seat}", "acme", "pay"), "acme-pay");
    }

    #[test]
    fn the_number_of_a_task_id_is_its_digits() {
        assert_eq!(number("T-1"), "1");
        assert_eq!(number("T-40"), "40");
        assert_eq!(number("T-"), "t");
    }
}
