//! T-136: the session fields of the profiles (§2 of gear/voyages/M4/design.md,
//! with the live checks of §12) and the patch that overrides them (§13.3).

use gear_herdr::{builtin, classify_screen, Profile, ProfilePatch, ScreenFlag};

/// The four fields of §2, per kind: the command that starts a new session,
/// the command that compacts it, the args that resume it and the reader of
/// its records.
fn fields(kind: &str) -> (String, String, Option<Vec<String>>, Option<String>) {
    let profile = builtin(kind).unwrap_or_else(|| panic!("{kind} is built in"));
    (
        profile.new_session.clone(),
        profile.compact.clone(),
        profile.resume_args.clone(),
        profile.usage_log.clone(),
    )
}

#[test]
fn every_kind_of_the_table_has_its_session_fields() {
    // pi
    let (new_session, compact, resume, log) = fields("pi");
    assert_eq!(new_session, "/new");
    assert_eq!(compact, "/compact");
    assert_eq!(
        resume,
        Some(vec!["--session".to_string(), "{session}".to_string()])
    );
    assert_eq!(log.as_deref(), Some("pi-jsonl"));

    // codex
    let (new_session, compact, resume, log) = fields("codex");
    assert_eq!(new_session, "/new");
    assert_eq!(compact, "/compact");
    assert_eq!(
        resume,
        Some(vec!["resume".to_string(), "{session}".to_string()])
    );
    assert_eq!(log.as_deref(), Some("codex-rollout"));

    // claude
    let (new_session, compact, resume, log) = fields("claude");
    assert_eq!(new_session, "/clear");
    assert_eq!(compact, "/compact");
    assert_eq!(
        resume,
        Some(vec!["--resume".to_string(), "{session}".to_string()])
    );
    assert_eq!(log.as_deref(), Some("claude-jsonl"));

    // opencode
    let (new_session, compact, resume, log) = fields("opencode");
    assert_eq!(new_session, "/new");
    assert_eq!(compact, "/compact");
    assert_eq!(
        resume,
        Some(vec!["-s".to_string(), "{session}".to_string()])
    );
    assert_eq!(log.as_deref(), Some("opencode-db"));
}

#[test]
fn a_kind_without_a_live_check_has_no_session_fields() {
    // grok is not one of the kinds of §2 and was never checked live.
    let (new_session, compact, _, log) = fields("grok");
    assert_eq!(new_session, "");
    assert_eq!(compact, "");
    assert_eq!(log, None);
}

#[test]
fn a_patch_overrides_the_session_fields() {
    let patch = ProfilePatch {
        new_session: Some("/fresh".into()),
        compact: Some("/shrink".into()),
        usage_log: Some("pi-jsonl".into()),
        error_markers: Some(vec!["overloaded_error".into()]),
        ..ProfilePatch::default()
    };
    let profile = builtin("codex").unwrap().patched(&patch, "codex");
    assert_eq!(profile.new_session, "/fresh");
    assert_eq!(profile.compact, "/shrink");
    assert_eq!(profile.usage_log.as_deref(), Some("pi-jsonl"));
    assert_eq!(profile.error_markers, vec!["overloaded_error".to_string()]);
    // The built-in is untouched.
    let builtin = builtin("codex").unwrap();
    assert_eq!(builtin.new_session, "/new");
    assert_eq!(builtin.compact, "/compact");
    assert_eq!(builtin.usage_log.as_deref(), Some("codex-rollout"));
}

#[test]
fn a_patch_may_empty_the_compact_command() {
    // `compact = ""` means the CLI has none (§2).
    let patch = ProfilePatch {
        compact: Some(String::new()),
        ..ProfilePatch::default()
    };
    let profile = builtin("pi").unwrap().patched(&patch, "pi");
    assert_eq!(profile.compact, "");
    assert_eq!(profile.new_session, "/new");
}

#[test]
fn the_screen_never_says_that_an_agent_is_busy() {
    // §10.2: busy is Herdr's state. The screen of an agent that works often
    // says something else: pi's old busy sign was the word ` Working`, and
    // `Working tree clean` holds it.
    for kind in ["pi", "codex", "claude", "opencode"] {
        let profile = builtin(kind).unwrap();
        assert!(
            classify_screen(&profile, "Working tree clean").is_empty(),
            "{kind}: the screen must not classify busy"
        );
        assert!(
            classify_screen(&profile, "• Working (12s • esc to interrupt)").is_empty(),
            "{kind}: the screen must not classify busy"
        );
    }
}

#[test]
fn the_codex_profile_exits_with_the_typed_command() {
    // §12 item 4: ctrl+c twice is not safe for Codex (the second one reached
    // the shell and broke the next typed start). The exit is the typed
    // command `/quit`, and no key is sent any more.
    let codex = builtin("codex").unwrap();
    assert_eq!(codex.exit_command.as_deref(), Some("/quit"));
    assert!(
        codex.exit_keys.is_empty(),
        "no exit keys for codex any more"
    );
}

#[test]
fn the_pi_profile_knows_the_provider_error() {
    // §12 item 6: MiniMax answered `529 … overloaded_error` and pi gave up
    // with `Error: Retry failed after 3 attempts: 529`. That is a provider
    // error, not a spent quota.
    let pi = builtin("pi").unwrap();
    assert!(pi.error_markers.contains(&"overloaded_error".to_string()));
    assert!(pi.error_markers.contains(&"Retry failed after".to_string()));
    let overload = "Error: 529 {\"error\":{\"message\":\"overloaded_error\"}}";
    assert!(classify_screen(&pi, overload).contains(&ScreenFlag::Error));
    // It is not a spent quota: §9.2 reads the source to tell the two apart.
    assert!(!classify_screen(&pi, overload).contains(&ScreenFlag::Quota));
    // The other kinds have no such sign yet.
    for kind in ["codex", "claude", "opencode", "grok"] {
        assert!(builtin(kind).unwrap().error_markers.is_empty(), "{kind}");
    }
}

#[test]
fn a_profile_without_the_new_fields_is_still_a_profile() {
    // A profile the config defines starts from the defaults: no session
    // command, no compact, no usage log.
    let profile = Profile::for_new();
    assert_eq!(profile.new_session, "");
    assert_eq!(profile.compact, "");
    assert_eq!(profile.usage_log, None);
    assert!(profile.error_markers.is_empty());
}
