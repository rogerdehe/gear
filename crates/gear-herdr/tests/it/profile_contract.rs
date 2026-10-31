//! Contract tests for profile.rs and outcome mapping. Do not edit to make them pass.

use gear_herdr::{
    classify_screen, outcome_from_error, validate_template, Outcome, Profile, ScreenFlag,
};

fn opencode() -> Profile {
    Profile {
        kind: "opencode".into(),
        busy_markers: vec!["esc interrupt".into()],
        ratelimit_markers: vec!["Error: Rate limit exceeded".into()],
        quota_markers: vec!["insufficient_quota".into()],
        quota_bar: None,
        auth_markers: vec!["Unauthorized".into()],
        // T-75: the back-off markers of a kind whose signs are not recorded
        // yet; the codex profile is the one that has them.
        backoff_markers: vec![],
        // T-113: the guardrails refusals of a kind without guardrails.
        blocked_markers: vec![],
        error_markers: vec!["overloaded_error".into()],
        new_session: "/new".into(),
        compact: "/compact".into(),
        usage_log: Some("opencode-db".into()),
        tail_lines: 3,
        confirm_rounds: 2,
        unattended_args: vec!["--auto".into()],
        instructions_args: vec![],
        session_ref: false,
        resume_args: None,
        exit_keys: vec!["ctrl+c".into()],
        exit_command: None,
        verified: false,
    }
}

#[test]
fn only_tail_window_is_matched() {
    let p = opencode();
    let screen = "Error: Rate limit exceeded\nok\nok\nok";
    assert!(
        classify_screen(&p, screen).is_empty(),
        "old error scrolled out of the 3-line window"
    );
    let screen = "x\nError: Rate limit exceeded\ny";
    assert_eq!(
        classify_screen(&p, screen).into_iter().collect::<Vec<_>>(),
        vec![ScreenFlag::RateLimit]
    );
}

#[test]
fn multiple_flags() {
    let p = opencode();
    let flags = classify_screen(&p, "  esc interrupt  \ninsufficient_quota\n");
    // M4 §10.2: the screen never says that an agent is busy — Herdr's state
    // does. The busy marker of the profile is kept, but it decides nothing.
    assert_eq!(
        flags.into_iter().collect::<Vec<_>>(),
        vec![ScreenFlag::Quota]
    );
}

#[test]
fn template_must_not_contain_markers() {
    let p = opencode();
    assert!(validate_template(&p, "Continue T-17. Task packet: tasks/T-17.md").is_ok());
    let err = validate_template(&p, "Stop if Error: Rate limit exceeded appears").unwrap_err();
    assert!(err.contains("Rate limit"));
}

#[test]
fn transport_errors_are_uncertain_protocol_refusals_are_refused() {
    let io = board_herdr_err_io();
    assert!(matches!(
        outcome_from_error::<()>(&io),
        Outcome::Uncertain { .. }
    ));
    let blocked = board_herdr::HerdrError::Protocol {
        code: "agent_blocked".into(),
        message: "x".into(),
    };
    assert!(matches!(
        outcome_from_error::<()>(&blocked),
        Outcome::Refused { .. }
    ));
    let weird = board_herdr::HerdrError::Protocol {
        code: "something_new".into(),
        message: "x".into(),
    };
    assert!(
        matches!(outcome_from_error::<()>(&weird), Outcome::Uncertain { .. }),
        "unknown code = uncertain"
    );
    let deadline = board_herdr::HerdrError::Deadline { operation: "read" };
    assert!(matches!(
        outcome_from_error::<()>(&deadline),
        Outcome::Uncertain { .. }
    ));
}

fn board_herdr_err_io() -> board_herdr::HerdrError {
    board_herdr::HerdrError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe"))
}
