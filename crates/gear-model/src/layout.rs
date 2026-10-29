//! Where GEAR looks for a project's remote and keeps a voyage (T-257).
//!
//! Conventions are configuration: the git remote a hand-in is checked against
//! and the folder a voyage's plan, reports and reviews live in are named in a
//! project's `[conventions]`, as templates — `{project}` and `{milestone}`
//! are filled in. The constants here are the names GEAR itself uses, and the
//! defaults of a project that says nothing — never the only way (a team's own
//! habit: a remote called `upstream`, one folder per voyage).
//!
//! Pure: no I/O, no store, so the app, the CLI and the board all resolve the
//! same templates the same way.

/// The default remote: the one a clone names `origin`.
pub const REMOTE: &str = "origin";

/// The default voyage folder (T-257): one folder per voyage, holding its
/// plan, its reports and its reviews.
pub const VOYAGE_DIR: &str = "gear/voyages/{milestone}";

/// `template` with `{project}` and `{milestone}` filled in: the folder a
/// voyage keeps its plan, reports and reviews in.
pub fn voyage_dir(template: &str, project: &str, milestone: &str) -> String {
    template
        .replace("{project}", project)
        .replace("{milestone}", milestone)
}

/// The voyage's plan file: its folder and `/plan.md`.
pub fn plan_file(template: &str, project: &str, milestone: &str) -> String {
    format!("{}/plan.md", voyage_dir(template, project, milestone))
}

/// The folder a voyage's reports live in: its folder and `/reports`.
pub fn reports_dir(template: &str, project: &str, milestone: &str) -> String {
    format!("{}/reports", voyage_dir(template, project, milestone))
}

/// The folder a voyage's reviews live in: its folder and `/reviews`.
pub fn reviews_dir(template: &str, project: &str, milestone: &str) -> String {
    format!("{}/reviews", voyage_dir(template, project, milestone))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_paths_gear_uses() {
        assert_eq!(REMOTE, "origin");
        assert_eq!(VOYAGE_DIR, "gear/voyages/{milestone}");
        assert_eq!(voyage_dir(VOYAGE_DIR, "harbor", "M3"), "gear/voyages/M3");
        assert_eq!(
            plan_file(VOYAGE_DIR, "harbor", "M3"),
            "gear/voyages/M3/plan.md"
        );
        assert_eq!(
            reports_dir(VOYAGE_DIR, "harbor", "M3"),
            "gear/voyages/M3/reports"
        );
        assert_eq!(
            reviews_dir(VOYAGE_DIR, "harbor", "M3"),
            "gear/voyages/M3/reviews"
        );
    }

    #[test]
    fn a_voyage_dir_names_the_project_and_the_milestone() {
        assert_eq!(
            voyage_dir("plans/{project}/{milestone}", "harbor", "M3"),
            "plans/harbor/M3"
        );
        assert_eq!(
            plan_file("plans/{project}/{milestone}", "harbor", "M3"),
            "plans/harbor/M3/plan.md"
        );
        assert_eq!(
            reports_dir("plans/{project}/{milestone}", "harbor", "M3"),
            "plans/harbor/M3/reports"
        );
        assert_eq!(
            reviews_dir("plans/{project}/{milestone}", "harbor", "M3"),
            "plans/harbor/M3/reviews"
        );
    }

    #[test]
    fn a_template_without_placeholders_is_its_own_path() {
        assert_eq!(voyage_dir("voyages", "harbor", "M3"), "voyages");
        assert_eq!(plan_file("voyages", "harbor", "M3"), "voyages/plan.md");
        assert_eq!(reports_dir("voyages", "harbor", "M3"), "voyages/reports");
        assert_eq!(reviews_dir("voyages", "harbor", "M3"), "voyages/reviews");
    }

    #[test]
    fn a_placeholder_is_filled_everywhere_it_appears() {
        assert_eq!(
            voyage_dir("{project}/{milestone}/{milestone}", "harbor", "M3"),
            "harbor/M3/M3"
        );
        assert_eq!(
            plan_file("{project}/{milestone}", "harbor", "<M>"),
            "harbor/<M>/plan.md"
        );
    }
}
