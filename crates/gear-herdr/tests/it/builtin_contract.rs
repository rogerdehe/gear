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

/// T-115: a reviewer whose Codex quota is spent is seen as on quota.
///
/// The texts are Codex's own (the installed binary, codex-cli 0.164.0): the
/// modal `Usage limit reached` over `Your included usage is exhausted. Choose
/// an option below to continue.`, and the error line `Usage limit reached.
/// You've reached your usage limit. Increase your limits to continue using
/// codex.`. Both are stops.
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
    ] {
        let flags = classify_screen(&p, text);
        assert!(!flags.contains(&ScreenFlag::Quota), "{text}: {flags:?}");
    }
}

#[test]
fn unverified_kinds_are_not_built_in() {
    // `codex` left this list with T-5: its SOP is verified
    // (`gear --skill agent:codex`), so it is built in like `opencode`.
    for kind in ["pi", "nope"] {
        assert!(builtin(kind).is_none(), "{kind}");
    }
}
