//! Contract for the ORCHESTRATE markers (T-78): the exact lines another agent
//! reads. They are pinned here so the shape cannot drift silently.

use gear_forge::marker;

#[test]
fn a_claim_names_the_agent_and_the_issue() {
    assert_eq!(
        marker::claim("base", 12),
        "<!-- ORCHESTRATE:CLAIM agent=base issue=12 -->"
    );
    // A seat id is what the engine has; it carries its project already.
    assert_eq!(
        marker::claim("gear:base", 7),
        "<!-- ORCHESTRATE:CLAIM agent=gear:base issue=7 -->"
    );
}

#[test]
fn a_completion_names_the_reviewer_the_rounds_and_ci() {
    assert_eq!(
        marker::completion("review-4", 2, "pass"),
        "<!-- ORCHESTRATE:COMPLETION reviewer=review-4 rounds=2 ci=pass -->"
    );
    assert_eq!(
        marker::completion("gear:review-4", 1, "none"),
        "<!-- ORCHESTRATE:COMPLETION reviewer=gear:review-4 rounds=1 ci=none -->"
    );
}

#[test]
fn a_marker_is_one_hidden_line() {
    for line in [marker::claim("base", 1), marker::completion("r", 1, "fail")] {
        assert!(line.starts_with("<!-- ORCHESTRATE:"), "{line}");
        assert!(line.ends_with(" -->"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        // One comment, so no markdown renderer shows it.
        assert_eq!(line.matches("<!--").count(), 1, "{line}");
        assert_eq!(line.matches("-->").count(), 1, "{line}");
    }
}
