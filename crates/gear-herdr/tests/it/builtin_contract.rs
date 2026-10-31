//! Contract for gear_herdr::builtin (T-56).

use gear_herdr::{builtin, classify_screen, ScreenFlag};

#[test]
fn opencode_is_built_in_and_verified() {
    let p = builtin("opencode").expect("opencode is built in");
    assert_eq!(p.kind, "opencode");
    assert!(p.verified);
    assert_eq!(p.unattended_args, vec!["--auto".to_string()]);
    assert_eq!(
        p.resume_args,
        Some(vec!["-s".to_string(), "{session}".to_string()])
    );
    assert_eq!(p.exit_keys, vec!["ctrl+c".to_string()]);
    let quota = classify_screen(&p, "  Error: ... Token Plan ... (2056)\n  Session failed");
    assert!(quota.contains(&ScreenFlag::Quota));
    assert!(
        classify_screen(&p, "Free usage limit reached. Connect a provider")
            .contains(&ScreenFlag::Quota)
    );
    // M4 §10.2: the screen never says that an agent is busy — Herdr's state
    // does. The busy marker of the profile is kept, but it decides nothing.
    assert!(classify_screen(&p, "   ■■⬝⬝ esc interrupt").is_empty());
}

#[test]
fn grok_is_built_in_and_verified() {
    // T-32: the facts of `gear --skill agent:grok`.
    let p = builtin("grok").expect("grok is built in");
    assert_eq!(p.kind, "grok");
    assert!(p.verified);
    assert_eq!(p.unattended_args, vec!["--always-approve".to_string()]);
    assert_eq!(
        p.resume_args,
        Some(vec!["-r".to_string(), "{session}".to_string()])
    );
    assert!(p.session_ref, "Herdr reports grok's session");
    // It exits on `/exit`, not on a key.
    assert_eq!(p.exit_keys, Vec::<String>::new());
    assert_eq!(p.exit_command.as_deref(), Some("/exit"));
    // M4 §10.2: its busy sign is no sign at all — Herdr's state decides.
    assert!(classify_screen(&p, "  status  │  Ctrl+c:cancel  │").is_empty());
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

/// T-115: a reviewer whose Codex quota is spent is seen as on quota.
///
/// The texts are Codex's own (the installed native binary, codex-cli 0.164.0):
/// the modal `Usage limit reached` over `Your included usage is exhausted.
/// Choose an option below to continue.`, the error line `Usage limit reached.
/// You've reached your usage limit. Increase your limits to continue using
/// codex.`, and the sentence the TUI prints when the plan is spent —
/// `You’ve hit your usage limit.`, with a **curly** apostrophe, followed by an
/// upgrade line, a credit link, an admin request or nothing. All are stops.
#[test]
fn a_spent_codex_limit_is_a_quota_stop() {
    let p = builtin("codex").expect("codex is built in");
    let modal = classify_screen(
        &p,
        "Usage limit reached\nYour included usage is exhausted. Choose an option below to continue.",
    );
    assert!(modal.contains(&ScreenFlag::Quota), "{modal:?}");
    let error = classify_screen(
        &p,
        " Usage limit reached. You've reached your usage limit. Increase your limits to continue using codex.",
    );
    assert!(error.contains(&ScreenFlag::Quota), "{error:?}");
    // The spent-limit sentence, exactly as the binary carries it (curly
    // apostrophe) and its forms with an upgrade line, a credit link and an
    // admin request behind it (F1 of the T-115 review).
    for text in [
        "You’ve hit your usage limit.",
        "You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits",
        "You’ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/settings/usage to purchase more credits",
        "You’ve hit your usage limit. Upgrade to Plus to continue using Codex (https://chatgpt.com/explore/plus)",
        "You’ve hit your usage limit. To get more access now, send a request to your admin",
        "You’ve hit your usage limit for gpt-6.1-sol",
        // The same sentence with a straight apostrophe: one marker for both.
        "You've hit your usage limit.",
    ] {
        let flags = classify_screen(&p, text);
        assert!(flags.contains(&ScreenFlag::Quota), "{text}: {flags:?}");
    }
    // The workspace forms of the same stop.
    for text in [
        "You've reached your workspace credit limit",
        "Your workspace is out of credits. Add credits to continue using Codex.",
        "You hit your spend cap set in your workspace. Increase your spend cap to continue.",
    ] {
        assert!(
            classify_screen(&p, text).contains(&ScreenFlag::Quota),
            "{text}"
        );
    }
}

/// T-115: the "only N% left" warning is **not** a stop. The agent still
/// works; a quota stop would park it or switch its model for nothing.
///
/// The two texts are read live (codex-cli 0.164.0, 2026-10-26): the warning
/// box `Heads up, you have less than 5% of your weekly limit left. Run
/// /status for a breakdown.` and the status line `⚠ weekly limit: only 3%
/// left · resets at 3:30 AM on 1 Nov · /status`.
#[test]
fn a_low_codex_limit_is_not_a_quota_stop() {
    let p = builtin("codex").expect("codex is built in");
    for text in [
        "Heads up, you have less than 5% of your weekly limit left. Run /status for a breakdown.",
        "⚠ weekly limit: only 3% left · resets at 3:30 AM on 1 Nov · /status",
        "GPT-6.1-Sol medium · Context 0% used · weekly 3% left",
        // The status-line items and the goal line of the same warning family:
        // neither is a stop (T-115 review F1).
        "Remaining usage on the weekly usage limit (omitted when unavailable)",
        "Approaching rate limits",
        "Goal hit usage limits (/goal resume)",
    ] {
        let flags = classify_screen(&p, text);
        assert!(!flags.contains(&ScreenFlag::Quota), "{text}: {flags:?}");
    }
}

/// T-284: a server-side stop is not a quota and not a login. The reviewer of
/// T-242 sat idle on exactly this screen, and a nudge fixes it (it is a
/// retry, not a spent plan): the screen flags must not stop the nudge.
#[test]
fn a_codex_model_at_capacity_is_neither_a_quota_nor_an_auth_stop() {
    let p = builtin("codex").expect("codex is built in");
    let flags = classify_screen(
        &p,
        "■ Selected model is at capacity. Please try a different model.",
    );
    assert!(!flags.contains(&ScreenFlag::Quota), "{flags:?}");
    assert!(!flags.contains(&ScreenFlag::Auth), "{flags:?}");
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

/// M4 §10.2: the screen never says that an agent is busy. Herdr's state
/// does — and the screen of an agent that is busy often says something else
/// entirely (`Working tree clean` holds pi's old busy sign).
#[test]
fn the_screen_never_says_that_an_agent_is_busy() {
    for kind in ["pi", "codex", "claude", "opencode", "grok"] {
        let p = builtin(kind).unwrap();
        assert!(
            classify_screen(&p, "── ⠋ Working ─────────────────────────").is_empty(),
            "{kind}"
        );
        assert!(
            classify_screen(&p, "── ⠧ Working ─────────────────────────").is_empty(),
            "{kind}"
        );
        // The line that holds pi's old busy sign in the middle of a word.
        assert!(
            classify_screen(&p, "Working tree clean").is_empty(),
            "{kind}"
        );
        // Codex's own busy sign.
        assert!(
            classify_screen(&p, "• Working (12s • esc to interrupt)").is_empty(),
            "{kind}"
        );
        // The idle screen says nothing either.
        assert!(classify_screen(&p, "LSP Inactive").is_empty(), "{kind}");
    }
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
    assert!(
        !flags.contains(&ScreenFlag::Quota),
        "a rate limit is not a spent quota"
    );
    // No credentials.
    assert!(
        classify_screen(&p, " Error: No API key found for anthropic.").contains(&ScreenFlag::Auth)
    );
    // The provider back-off: pi retries on its own, so it is not a stall.
    assert!(
        classify_screen(&p, "── ⠙ Retrying (3/3) in 7s... (escape to cancel) ──")
            .contains(&ScreenFlag::Backoff)
    );
}

/// The guardrails refusals of `@aliou/pi-guardrails` (T-113): the agent goes
/// on after them, so the captain is told.
#[test]
fn the_pi_guardrails_refusals_are_recognised() {
    let p = builtin("pi").unwrap();
    assert!(classify_screen(
        &p,
        "   Command auto-denied: No privileged commands in an unattended lane."
    )
    .contains(&ScreenFlag::Blocked));
    assert!(classify_screen(
        &p,
        "   Access to ~/probe.txt is blocked (outside working directory)."
    )
    .contains(&ScreenFlag::Blocked));
    // Ordinary output is not a refusal.
    assert!(!classify_screen(&p, " Took 0.0s\n done-sleeping").contains(&ScreenFlag::Blocked));
}
