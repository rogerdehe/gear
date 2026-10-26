//! Contract for gear_herdr::builtin (T-56).

use gear_herdr::{builtin, classify_screen, ScreenFlag};

#[test]
fn opencode_is_built_in_and_verified() {
    let p = builtin("opencode").expect("opencode is built in");
    assert_eq!(p.kind, "opencode");
    assert!(p.verified);
    assert_eq!(p.unattended_args, vec!["--auto".to_string()]);
    assert_eq!(p.resume_args, Some(vec!["-s".to_string(), "{session}".to_string()]));
    assert_eq!(p.exit_keys, vec!["ctrl+c".to_string()]);
    let quota = classify_screen(&p, "  Error: ... Token Plan ... (2056)\n  Session failed");
    assert!(quota.contains(&ScreenFlag::Quota));
    assert!(classify_screen(&p, "Free usage limit reached. Connect a provider").contains(&ScreenFlag::Quota));
    assert!(classify_screen(&p, "   ■■⬝⬝ esc interrupt").contains(&ScreenFlag::Busy));
}

#[test]
fn grok_is_built_in_and_verified() {
    // T-32: the facts of `gear --skill agent:grok`.
    let p = builtin("grok").expect("grok is built in");
    assert_eq!(p.kind, "grok");
    assert!(p.verified);
    assert_eq!(p.unattended_args, vec!["--always-approve".to_string()]);
    assert_eq!(p.resume_args, Some(vec!["-r".to_string(), "{session}".to_string()]));
    assert!(p.session_ref, "Herdr reports grok's session");
    // It exits on `/exit`, not on a key.
    assert_eq!(p.exit_keys, Vec::<String>::new());
    assert_eq!(p.exit_command.as_deref(), Some("/exit"));
    // Its busy sign is the shortcut line of a running turn.
    assert!(classify_screen(&p, "  status  │  Ctrl+c:cancel  │").contains(&ScreenFlag::Busy));
}

#[test]
fn codex_is_built_in_and_never_shows_its_update_dialog() {
    // T-57: without the update check off, Codex shows an "Update available"
    // dialog at start and the agent sits blocked, unseen (2026-10-25, codex
    // 0.163.0 → 0.164.0).
    let p = builtin("codex").expect("codex is built in");
    assert_eq!(p.kind, "codex");
    assert!(p.verified);
    assert_eq!(
        p.unattended_args,
        vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "-c".to_string(),
            "check_for_update_on_startup=false".to_string(),
            // T-76: the folder the agent runs in is trusted (filled from the
            // worktree at start), so no "Trust this folder?" dialog.
            "-c".to_string(),
            "projects={\"{repo_root}\"={trust_level=\"trusted\"}}".to_string(),
        ]
    );
}

#[test]
fn unverified_kinds_are_not_built_in() {
    // `codex` left this list with T-5: its SOP is verified
    // (`gear --skill agent:codex`), so it is built in like `opencode`.
    for kind in ["nope", "omp"] {
        assert!(builtin(kind).is_none(), "{kind}");
    }
}

/// T-113: pi's facts, taken from pi 1.4.0's own output on this machine
/// (a live run in a scratch pane; the SOP `gear --skill agent:pi`).
#[test]
fn pi_is_built_in_and_verified() {
    let p = builtin("pi").expect("pi is built in");
    assert_eq!(p.kind, "pi");
    assert!(p.verified);
    // `--approve` is pi's project-trust override: without it a lane in a
    // fresh worktree waits on a trust prompt.
    assert_eq!(p.unattended_args, vec!["--approve".to_string()]);
    // Herdr reports pi's session file, and `--session` takes that path.
    assert!(p.session_ref, "Herdr reports pi's session file");
    assert_eq!(
        p.resume_args,
        Some(vec!["--session".to_string(), "{session}".to_string()])
    );
    // It exits on `/quit`, not on a key.
    assert_eq!(p.exit_keys, Vec::<String>::new());
    assert_eq!(p.exit_command.as_deref(), Some("/quit"));
}

/// The busy sign of pi: the spinner line of a running turn. The glyph
/// changes, the word does not.
#[test]
fn the_pi_busy_sign_is_the_working_line() {
    let p = builtin("pi").unwrap();
    assert!(classify_screen(&p, "── ⠋ Working ─────────────────────────").contains(&ScreenFlag::Busy));
    assert!(classify_screen(&p, "── ⠧ Working ─────────────────────────").contains(&ScreenFlag::Busy));
    // The idle screen has no working line.
    assert!(!classify_screen(&p, "LSP Inactive").contains(&ScreenFlag::Busy));
}

/// The provider signs of pi, taken from its own output (1.4.0).
#[test]
fn the_pi_provider_signs_are_recognised() {
    let p = builtin("pi").unwrap();
    // A spent quota.
    let quota = " Error: 429: {\"message\":\"You exceeded your current quota, please check your plan and billing details.\",\"type\":\"insufficient_quota\",\"code\":\"insufficient_quota\"}";
    assert!(classify_screen(&p, quota).contains(&ScreenFlag::Quota));
    // A rate limit (a stop for the retry, not a quota stop).
    let limited = " Error: 429: {\"message\":\"Rate limit reached for mock-model\",\"type\":\"rate_limit_error\",\"code\":\"rate_limit_exceeded\"}";
    let flags = classify_screen(&p, limited);
    assert!(flags.contains(&ScreenFlag::RateLimit));
    assert!(!flags.contains(&ScreenFlag::Quota), "a rate limit is not a spent quota");
    // No credentials.
    assert!(classify_screen(&p, " Error: No API key found for anthropic.").contains(&ScreenFlag::Auth));
    // The provider back-off: pi retries on its own, so it is not a stall.
    assert!(classify_screen(&p, "── ⠙ Retrying (3/3) in 7s... (escape to cancel) ──").contains(&ScreenFlag::Backoff));
}

/// The guardrails refusals of `@aliou/pi-guardrails` (T-113): the agent goes
/// on after them, so the captain is told.
#[test]
fn the_pi_guardrails_refusals_are_recognised() {
    let p = builtin("pi").unwrap();
    assert!(
        classify_screen(&p, "   Command auto-denied: No privileged commands in an unattended lane.")
            .contains(&ScreenFlag::Blocked)
    );
    assert!(
        classify_screen(&p, "   Access to ~/probe.txt is blocked (outside working directory).")
            .contains(&ScreenFlag::Blocked)
    );
    // Ordinary output is not a refusal.
    assert!(!classify_screen(&p, " Took 0.0s\n done-sleeping").contains(&ScreenFlag::Blocked));
}
