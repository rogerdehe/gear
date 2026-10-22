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
fn unverified_kinds_are_not_built_in() {
    for kind in ["claude", "codex", "pi", "nope"] {
        assert!(builtin(kind).is_none(), "{kind}");
    }
}
