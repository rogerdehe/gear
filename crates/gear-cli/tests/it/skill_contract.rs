//! Contract for `gear --skill` (T-9): prints the version-matched skill text
//! embedded in the binary (design/16). Topics:
//!   (none) → skills/gear/SKILL.md
//!   <name> → skills/gear/references/<name>.md        (e.g. decompose)
//!   agent:<kind> → skills/gear/references/agents/<kind>.md
//! Unknown topic → stderr `ERROR BAD_FORMAT: unknown skill topic <t>; available: <list>`, exit 1.

use std::path::PathBuf;
use std::process::{Command, Output};

fn gear(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(args)
        .env_remove("GEAR_CTX")
        // Never the real store: lanes run tests with GEAR_DB set, and a
        // `gear` call starts an engine on it (design/14).
        .env_remove("GEAR_DB")
        .output()
        .expect("run gear")
}

fn skill_file(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/gear").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

#[test]
fn bare_skill_prints_skill_md() {
    let o = gear(&["--skill"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), skill_file("SKILL.md"));
}

#[test]
fn reference_topics_print_their_file() {
    let o = gear(&["--skill", "decompose"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), skill_file("references/decompose.md"));

    let o = gear(&["--skill", "agent:opencode"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), skill_file("references/agents/opencode.md"));
}

/// T-30: the review rules, the captain's guide and the Claude Code SOP are
/// topics of the skill, not files of the gear repo.
#[test]
fn the_new_topics_print_their_file() {
    for (topic, rel) in [
        ("review", "references/review.md"),
        ("captain", "references/captain.md"),
        ("agent:claude", "references/agents/claude.md"),
        ("agent:grok", "references/agents/grok.md"),
    ] {
        let o = gear(&["--skill", topic]);
        assert!(o.status.success(), "{topic}: {}", stderr(&o));
        assert_eq!(stdout(&o), skill_file(rel), "{topic}");
    }
}

/// T-30: the knowledge ships with the binary. No topic may point at a file
/// of the gear repo — a project that runs GEAR has no such files.
#[test]
fn no_topic_names_a_file_of_the_gear_repo() {
    for topic in [
        "main",
        "conventions",
        "decompose",
        "review",
        "captain",
        "agent:opencode",
        "agent:codex",
        "agent:claude",
        "agent:grok",
    ] {
        let o = gear(&["--skill", topic]);
        assert!(o.status.success(), "{topic}: {}", stderr(&o));
        let text = stdout(&o);
        assert!(!text.contains("ops/"), "the {topic} topic names an ops/ path:\n{text}");
        assert!(
            !text.contains("docs/design/"),
            "the {topic} topic names a design path:\n{text}"
        );
    }
}

#[test]
fn unknown_topic_is_bad_format_and_lists_topics() {
    let o = gear(&["--skill", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).is_empty());
    let e = stderr(&o);
    assert!(e.starts_with("ERROR BAD_FORMAT: unknown skill topic nope; available: "), "{e}");
    assert!(e.contains("decompose") && e.contains("agent:opencode"), "{e}");
}

#[test]
fn not_yet_commands_fail_cleanly() {
    let o = gear(&["admin", "seat", "confirm", "x"]); // after the walking skeleton
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with("ERROR FORBIDDEN: "), "{}", stderr(&o));
}
