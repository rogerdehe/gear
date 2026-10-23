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
    // It has no exit keys: a released agent's pane is closed by Herdr, and
    // nothing in GEAR sends grok a command (T-42).
    assert_eq!(p.exit_keys, Vec::<String>::new());
    // Its busy sign is the shortcut line of a running turn.
    assert!(classify_screen(&p, "  status  │  Ctrl+c:cancel  │").contains(&ScreenFlag::Busy));
}

#[test]
fn unverified_kinds_are_not_built_in() {
    // `codex` left this list with T-5: its SOP is verified
    // (`gear --skill agent:codex`), so it is built in like `opencode`.
    for kind in ["pi", "nope"] {
        assert!(builtin(kind).is_none(), "{kind}");
    }
}
