//! Contract for `gear --skill` (T-9): prints the version-matched skill text
//! embedded in the binary (design/16). Topics:
//!   (none) → skills/gear/SKILL.md
//!   <name> → skills/gear/references/<name>.md        (e.g. decompose)
//!   agent:<kind> → skills/gear/references/agents/<kind>.md
//! Unknown topic → stderr `ERROR BAD_FORMAT: unknown skill topic <t>; available: <list>`, exit 1.

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::scratch;

/// A `gear` call in a scratch home: never the real store (T-41).
fn gear(home: &Path, args: &[&str]) -> Output {
    scratch::gear(home, args).output().expect("run gear")
}

fn skill_file(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills/gear")
        .join(rel);
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
    let home = scratch::home("skill-bare");
    let o = gear(&home, &["--skill"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), skill_file("SKILL.md"));
}

/// The bare skill names every topic it has: a reader who runs
/// `gear --skill` must find the agent SOPs (T-30, T-113).
#[test]
fn the_bare_skill_names_every_topic() {
    let home = scratch::home("skill-bare-topics");
    let o = gear(&home, &["--skill"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    for topic in [
        "conventions",
        "captain",
        "course",
        "decompose",
        "review",
        "adopt",
        "github",
        "departure",
        "agent:opencode",
        "agent:codex",
        "agent:claude",
        "agent:grok",
        "agent:pi",
    ] {
        assert!(
            text.contains(topic),
            "the bare skill does not name {topic}:\n{text}"
        );
    }
    // The installer is named with every kind it writes for.
    assert!(
        text.contains("gear admin install-skill --agent <claude|codex|opencode|grok|pi>"),
        "{text}"
    );
}

#[test]
fn reference_topics_print_their_file() {
    let home = scratch::home("skill-reference");
    let o = gear(&home, &["--skill", "decompose"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), skill_file("references/decompose.md"));

    let o = gear(&home, &["--skill", "agent:opencode"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), skill_file("references/agents/opencode.md"));
}

#[test]
fn m4_commands_and_decision_rules_are_printed_by_the_skill() {
    let home = scratch::home("skill-m4");
    for (topic, expected) in [
        (
            "main",
            &[
                "--kind progress",
                "--left",
                "--blocker",
                "--kind blocked",
                "--on",
                "--friction",
            ][..],
        ),
        (
            "captain",
            &[
                "gear queue",
                "gear plan show",
                "followup-of",
                "- ★",
                "gear friction close",
                "- release <reviewer>",
            ][..],
        ),
        (
            "course",
            &["gear report voyage", "★ decisions", "user: <text>"][..],
        ),
        ("decompose", &["followup-of: T-<n>", "session"][..]),
        ("review", &["--friction", "exceeds its budget"][..]),
    ] {
        let args = if topic == "main" {
            vec!["--skill"]
        } else {
            vec!["--skill", topic]
        };
        let o = gear(&home, &args);
        assert!(o.status.success(), "{topic}: {}", stderr(&o));
        let text = stdout(&o);
        for needle in expected {
            assert!(text.contains(needle), "{topic} missing {needle}: {text}");
        }
    }
}

/// T-30: the review rules, the captain's guide and the Claude Code SOP are
/// topics of the skill, not files of the gear repo.
#[test]
fn the_new_topics_print_their_file() {
    let home = scratch::home("skill-topics");
    for (topic, rel) in [
        ("review", "references/review.md"),
        ("adopt", "references/adopt.md"),
        ("github", "references/github.md"),
        ("departure", "references/departure.md"),
        ("captain", "references/captain.md"),
        ("agent:claude", "references/agents/claude.md"),
        ("agent:grok", "references/agents/grok.md"),
        ("agent:pi", "references/agents/pi.md"),
    ] {
        let o = gear(&home, &["--skill", topic]);
        assert!(o.status.success(), "{topic}: {}", stderr(&o));
        let text = stdout(&o);
        assert_eq!(text, skill_file(rel), "{topic}");
    }
}

/// T-30: the knowledge ships with the binary. No topic may point at a file
/// of the gear repo — a project that runs GEAR has no such files.
#[test]
fn no_topic_names_a_file_of_the_gear_repo() {
    let home = scratch::home("skill-repo");
    for topic in [
        "main",
        "conventions",
        "course",
        "decompose",
        "review",
        "captain",
        "adopt",
        "github",
        "departure",
        "agent:opencode",
        "agent:codex",
        "agent:claude",
        "agent:grok",
        "agent:pi",
    ] {
        let o = gear(&home, &["--skill", topic]);
        assert!(o.status.success(), "{topic}: {}", stderr(&o));
        let text = stdout(&o);
        assert!(
            !text.contains("ops/"),
            "the {topic} topic names an ops/ path:\n{text}"
        );
        assert!(
            !text.contains("docs/design/"),
            "the {topic} topic names a design path:\n{text}"
        );
    }
}

/// T-469: the skill describes the role charter — written in gear.toml per
/// lane and for the reviewers, given once per conversation — and the captain's
/// guide still carries its hand-fix paragraph.
#[test]
fn the_skill_describes_role_charters() {
    let home = scratch::home("skill-charters");
    for (topic, needles) in [
        (
            "conventions",
            &[
                "Role charters",
                "charter = ",
                "never again in a resumed one",
            ][..],
        ),
        (
            "captain",
            &[
                "one charter per role",
                "charter =",
                "hand-fix:",
                "The same holds for an urgent fix",
            ][..],
        ),
    ] {
        let o = gear(&home, &["--skill", topic]);
        assert!(o.status.success(), "{topic}: {}", stderr(&o));
        let text = stdout(&o);
        for needle in needles {
            assert!(text.contains(needle), "{topic} missing {needle}");
        }
    }
}

#[test]
fn unknown_topic_is_bad_format_and_lists_topics() {
    let home = scratch::home("skill-unknown");
    let o = gear(&home, &["--skill", "nope"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).is_empty());
    let e = stderr(&o);
    assert!(
        e.starts_with("ERROR BAD_FORMAT: unknown skill topic nope; available: "),
        "{e}"
    );
    assert!(
        e.contains("decompose") && e.contains("agent:opencode"),
        "{e}"
    );
}

/// T-41: a `gear` call without `GEAR_DB` opens the store under its `HOME`.
/// The tests give it a scratch home, so the store they write is a scratch one
/// and never the production store.
#[test]
fn the_store_of_a_test_lives_under_its_scratch_home() {
    let home = scratch::home("store-home");
    assert!(!scratch::default_store(&home).exists());

    // The call fails (no seat, no Herdr), but it opened the store first.
    let o = gear(&home, &[]);
    assert!(scratch::default_store(&home).exists(), "{}", stderr(&o));
}
