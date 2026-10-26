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
fn a_hand_in_names_the_lane_that_finished_its_part() {
    // The writer's completion: `agent`, never `reviewer` (T-98).
    assert_eq!(
        marker::hand_in("gear:base", 1),
        "<!-- ORCHESTRATE:COMPLETION agent=gear:base rounds=1 -->"
    );
    assert_eq!(
        marker::hand_in("sandbox:calc", 2),
        "<!-- ORCHESTRATE:COMPLETION agent=sandbox:calc rounds=2 -->"
    );
}

#[test]
fn a_completion_names_the_reviewer_the_rounds_and_ci_when_it_is_known() {
    // The reviewer's completion: `ci` only when the gate ran (at the merge).
    assert_eq!(
        marker::completion("review-4", 2, Some("pass")),
        "<!-- ORCHESTRATE:COMPLETION reviewer=review-4 rounds=2 ci=pass -->"
    );
    assert_eq!(
        marker::completion("gear:review-4", 1, None),
        "<!-- ORCHESTRATE:COMPLETION reviewer=gear:review-4 rounds=1 -->"
    );
    assert!(
        !marker::completion("gear:review-4", 1, None).contains("ci="),
        "an unknown CI state is omitted, not guessed"
    );
}

#[test]
fn a_marker_is_one_hidden_line() {
    for line in [
        marker::claim("base", 1),
        marker::hand_in("base", 1),
        marker::completion("r", 1, Some("fail")),
    ] {
        assert!(line.starts_with("<!-- ORCHESTRATE:"), "{line}");
        assert!(line.ends_with(" -->"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        // One comment, so no markdown renderer shows it.
        assert_eq!(line.matches("<!--").count(), 1, "{line}");
        assert_eq!(line.matches("-->").count(), 1, "{line}");
    }
}
