//! T-457: Codex 0.165's startup screens.
//!
//! The two screens are the ones captured in M5 (gear/voyages/M6/backlog.md,
//! another project): after `/new` Codex asks where the new conversation
//! should run, and on a resume from another directory it asks which working
//! directory to use. Herdr reports the agent done/idle on both, so the profile
//! has to read them as blocked and answer them.

use gear_herdr::{
    answer_dialog, builtin, classify_screen, fake::Call, fake::FakeHerdr, AgentStatus, HerdrAct,
    Outcome, ScreenFlag,
};

/// The screen Codex 0.165 shows after `/new` (M5 capture): the title, the two
/// choices and their descriptions. The first choice — the current worktree —
/// is preselected, so Enter keeps the seat where it works.
const CODEX_NEW_SCREEN: &str = "\
  Where should the new conversation run?
  1. Use current Git worktree
     Keep using the current working directory
  2. Create new Git worktree
     Create a separate checkout of this repository in another directory
";

/// The screen Codex 0.165 shows when a resume starts in a directory other
/// than the session's (M5 capture): which working directory to use. The
/// seat's own directory is the one Codex was started in.
const CODEX_RESUME_SCREEN: &str = "\
  Choose which working directory to use when the current directory and saved session differ
  1. Current = your current working directory
  2. Use session directory (/worktrees/gear/home-heat)
";

/// A plain composer: no dialog, nothing to answer.
const CODEX_COMPOSER: &str = "\
  permissions: YOLO mode
  > 
  gpt-6.1-sol high · 01a2b3c4 · /worktrees/gear/home-heat · Context 0% used · weekly 94% left
";

#[test]
fn the_new_conversation_dialog_is_blocked_and_keeps_the_current_worktree() {
    let codex = builtin("codex").expect("codex is built in");
    // Herdr reports the agent done/idle while the dialog waits: the screen is
    // the only place it shows, so it must read as blocked.
    assert!(
        classify_screen(&codex, CODEX_NEW_SCREEN).contains(&ScreenFlag::Blocked),
        "the /new dialog is a blocked screen"
    );
    let dialog = codex
        .dialog(CODEX_NEW_SCREEN)
        .expect("the /new dialog is a codex dialog");
    // Enter takes the preselected "Use current Git worktree".
    assert_eq!(dialog.keys, vec!["enter".to_string()]);
    // A fork asks the same question, with the same choices.
    let fork = CODEX_NEW_SCREEN.replace(
        "Where should the new conversation run?",
        "Where should the forked conversation run?",
    );
    assert!(classify_screen(&codex, &fork).contains(&ScreenFlag::Blocked));
    assert!(codex.dialog(&fork).is_some());
}

#[test]
fn the_resume_directory_dialog_is_blocked_and_answered() {
    let codex = builtin("codex").expect("codex is built in");
    assert!(
        classify_screen(&codex, CODEX_RESUME_SCREEN).contains(&ScreenFlag::Blocked),
        "the resume directory dialog is a blocked screen"
    );
    let dialog = codex
        .dialog(CODEX_RESUME_SCREEN)
        .expect("the resume dialog is a codex dialog");
    // The dialog preselected the current directory — the seat's own — and
    // Enter takes it.
    assert_eq!(dialog.keys, vec!["enter".to_string()]);
}

#[test]
fn a_resume_uses_the_seats_own_directory_without_asking() {
    // T-457: Codex 0.165's own `tui.resume_cwd` setting. `current` makes a
    // resume use the directory Codex was started in — the seat's worktree —
    // so the dialog never shows (the answer above stays as the fallback).
    let codex = builtin("codex").expect("codex is built in");
    assert!(
        codex
            .unattended_args
            .iter()
            .any(|arg| arg == "tui.resume_cwd=\"current\""),
        "{:?}",
        codex.unattended_args
    );
}

#[test]
fn a_plain_composer_is_no_dialog_and_nothing_is_sent() {
    let codex = builtin("codex").expect("codex is built in");
    assert!(codex.dialog(CODEX_COMPOSER).is_none());
    assert!(!classify_screen(&codex, CODEX_COMPOSER).contains(&ScreenFlag::Blocked));
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "codex", AgentStatus::Idle);
    herdr.set_screen("w1:p1", CODEX_COMPOSER);
    assert_eq!(answer_dialog(&mut herdr, "w1:p1", &codex), None);
    assert!(
        !herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::SendKeys { .. })),
        "no key is sent into a plain composer"
    );
}

#[test]
fn the_answer_keys_reach_the_pane() {
    let codex = builtin("codex").expect("codex is built in");
    for screen in [CODEX_NEW_SCREEN, CODEX_RESUME_SCREEN] {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "codex", AgentStatus::Done);
        herdr.set_screen("w1:p1", screen);
        assert!(
            matches!(
                answer_dialog(&mut herdr, "w1:p1", &codex),
                Some(Outcome::Ok(()))
            ),
            "{screen}"
        );
        assert!(
            herdr.calls.iter().any(
                |call| matches!(call, Call::SendKeys { keys, .. } if keys == &["enter".to_string()])
            ),
            "{screen}"
        );
    }
}

#[test]
fn a_slash_command_is_typed_in_one_call() {
    // T-457: the session command leaves as one `pane.send_text`, not one
    // keystroke per character.
    let codex = builtin("codex").expect("codex is built in");
    let mut herdr = FakeHerdr::new();
    assert!(matches!(
        herdr.pane_type("w1:p1", &codex.new_session),
        Outcome::Ok(())
    ));
    let typed: Vec<String> = herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::PaneType { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(typed, vec!["/new".to_string()]);
}
