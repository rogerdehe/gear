//! Contract for gear_app::draft::parse (T-31, design/05). Pure.

use gear_app::draft::{parse, Priority, StaffItem};
use gear_model::ErrorCode;

const FULL: &str = r#"<!-- gear-plan project=harbor base=12 -->
# Goal
Ship login.

# Milestone M2: Login system
Scope; out of scope: SSO.

# Decisions
- 2026-10-21 T-31 jumps to the front

# Queue
## T-31 [now] lane=heat milestone=M2 required
goal: Fix the iOS login crash
accept: Repro case passes
evidence-files: notes/T-31-repro.md
evidence-due: 60
deps: -

## NEW [should] lane=wire
goal: Add rate limit
accept: tests pass
deps: T-31, T-12

# Staff
- temp R-4 task=T-45 profile=opencode-review
- temp R-2 task=T-31 resume
- park heat until=2026-10-21T15:30Z
- park media until-task=T-12
- hold wire reason=waiting for user authorization
- retire prompts

# Verdicts
- T-29 attempt=2 contract=3 candidate=abc1234 pass evidence=logs/T-29-verify.md
- T-33 attempt=1 contract=1 candidate=def5678 fail rework="Edge cases not covered"

# Review
V-9 keep plan=12: keep the order; next=2026-10-21T16:00Z
"#;

fn bad_line(text: &str) -> String {
    let e = parse(text).unwrap_err();
    assert_eq!(e.code, ErrorCode::BadFormat, "{e:?}");
    e.detail
}

#[test]
fn full_draft_parses() {
    let d = parse(FULL).unwrap();
    assert_eq!((d.project.as_str(), d.base), ("harbor", 12));
    assert_eq!(d.goal, "Ship login.");
    assert_eq!(d.milestones, vec![("M2: Login system".to_string(), "Scope; out of scope: SSO.".to_string())]);
    assert_eq!(d.decisions, vec!["2026-10-21 T-31 jumps to the front".to_string()]);
    assert_eq!(d.queue.len(), 2);
    let a = &d.queue[0];
    assert_eq!(a.id.as_deref(), Some("T-31"));
    assert_eq!(a.priority, Priority::Now);
    assert_eq!((a.lane.as_deref(), a.milestone.as_deref(), a.required), (Some("heat"), Some("M2"), true));
    assert_eq!(a.evidence_files, vec!["notes/T-31-repro.md".to_string()]);
    assert_eq!(a.evidence_due, Some(60));
    assert!(a.deps.is_empty());
    let b = &d.queue[1];
    assert_eq!((b.id.as_deref(), b.priority, b.required), (None, Priority::Should, false));
    assert_eq!(b.deps, vec!["T-31".to_string(), "T-12".to_string()]);
    assert_eq!(d.staff.len(), 6);
    assert!(matches!(&d.staff[1], StaffItem::Temp { id, resume: true, .. } if id == "R-2"));
    assert!(matches!(&d.staff[3], StaffItem::Park { until_task: Some(t), .. } if t == "T-12"));
    assert!(matches!(&d.staff[4], StaffItem::Hold { reason, .. } if reason == "waiting for user authorization"));
    assert_eq!(d.verdicts.len(), 2);
    assert!(d.verdicts[0].pass && d.verdicts[0].evidence.as_deref() == Some("logs/T-29-verify.md"));
    assert!(!d.verdicts[1].pass && d.verdicts[1].rework.as_deref() == Some("Edge cases not covered"));
    assert_eq!((d.reviews[0].id.as_str(), d.reviews[0].plan), ("V-9", 12));
}

#[test]
fn sections_are_optional_but_header_is_not() {
    let d = parse("<!-- gear-plan project=p base=0 -->\n# Queue\n").unwrap();
    assert!(d.queue.is_empty() && d.staff.is_empty() && d.goal.is_empty());
    assert!(bad_line("# Goal\nx\n").starts_with("line 1: "));
}

#[test]
fn errors_name_the_line() {
    let unknown_field = FULL.replace("evidence-due: 60", "owner: bob");
    assert!(bad_line(&unknown_field).starts_with("line 16: "), "{}", bad_line(&unknown_field));
    let bad_state = FULL.replace("deps: -\n", "deps: -\nstate: done\n");
    assert!(bad_line(&bad_state).starts_with("line 18: "));
    let bad_priority = FULL.replace("[now]", "[asap]");
    assert!(bad_line(&bad_priority).starts_with("line 12: "));
    let unknown_section = FULL.replace("# Decisions", "# Notes");
    assert!(bad_line(&unknown_section).starts_with("line 8: "));
}

#[test]
fn verdict_shape_is_strict() {
    let pass_without_evidence = FULL.replace(" evidence=logs/T-29-verify.md", "");
    assert!(bad_line(&pass_without_evidence).starts_with("line 33: "));
    let fail_without_rework = FULL.replace(r#" rework="Edge cases not covered""#, "");
    assert!(bad_line(&fail_without_rework).starts_with("line 34: "));
    let both = FULL.replace("pass evidence=", "pass rework=\"x\" evidence=");
    assert!(bad_line(&both).starts_with("line 33: "));
}

/// T-141 (rework F1, F2): the quoted value of a verdict line is the **last**
/// field and runs from its first quote to its closing quote, so the prose may
/// contain quotes of its own; the `key=value` fields come before it.
#[test]
fn a_verdict_quoted_value_keeps_its_quotes_and_comes_last() {
    /// A draft with one verdict line.
    fn draft(line: &str) -> String {
        format!("<!-- gear-plan project=p base=0 -->\n# Verdicts\n{line}\n")
    }
    let quoted = "return the literal \"unknown\" on missing input";

    // The reviewer's example: a quoted literal inside the rework text.
    let d = parse(&draft(
        "- T-1 attempt=1 contract=1 candidate=abc1234 fail rework=\"return the literal \"unknown\" on missing input\"",
    ))
    .unwrap();
    assert_eq!(d.verdicts[0].rework.as_deref(), Some(quoted));

    // The same with the quotes escaped.
    let d = parse(&draft(
        "- T-1 attempt=1 contract=1 candidate=abc1234 fail rework=\"return the literal \\\"unknown\\\" on missing input\"",
    ))
    .unwrap();
    assert_eq!(
        d.verdicts[0].rework.as_deref(),
        Some("return the literal \\\"unknown\\\" on missing input")
    );

    // The estimate comes before the quoted value.
    let d = parse(&draft(
        "- T-1 attempt=1 contract=1 candidate=abc1234 fail estimate=25 rework=\"return the literal \"unknown\" on missing input\"",
    ))
    .unwrap();
    assert_eq!(d.verdicts[0].rework.as_deref(), Some(quoted));
    assert_eq!(d.verdicts[0].estimate, Some(25));

    // A value without a closing quote is refused (F2).
    let detail = bad_line(&draft(
        "- T-1 attempt=1 contract=1 candidate=abc1234 fail rework=\"fix the edge case",
    ));
    assert!(detail.contains("no closing quote"), "{detail}");

    // A field after the quoted value is refused, and the error names the
    // order.
    let detail = bad_line(&draft(
        "- T-1 attempt=1 contract=1 candidate=abc1234 fail rework=\"fix the edge case\" estimate=25",
    ));
    assert!(detail.contains("last field"), "{detail}");
    assert!(detail.contains("estimate=25"), "{detail}");
}

#[test]
fn staff_lines_are_strict() {
    let park_both = FULL.replace("park media until-task=T-12", "park media until=2026-10-21T15:30Z until-task=T-12");
    assert!(bad_line(&park_both).starts_with("line 28: "));
    let unknown_verb = FULL.replace("- retire prompts", "- fire prompts");
    assert!(bad_line(&unknown_verb).starts_with("line 30: "));
}

/// T-133: `- release <lane>` takes one lane and nothing else.
#[test]
fn release_is_a_staff_line() {
    let d = parse("<!-- gear-plan project=p base=1 -->\n# Staff\n- release heat\n").unwrap();
    assert!(matches!(&d.staff[0], StaffItem::Release { name, .. } if name == "heat"));
    assert!(bad_line("<!-- gear-plan project=p base=1 -->\n# Staff\n- release\n").contains("`release` needs a lane"));
    assert!(
        bad_line("<!-- gear-plan project=p base=1 -->\n# Staff\n- release heat now\n")
            .contains("`release` takes only a lane")
    );
}
