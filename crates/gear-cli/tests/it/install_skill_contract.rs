//! Contract for `gear admin install-skill --agent <cli>` (T-30): the skill
//! (SKILL.md and its references) is written where that agent CLI loads its
//! skills, and the command says where it wrote.

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::scratch;

/// A `gear` call in a scratch home: never the real store (T-41).
fn gear(home: &Path, args: &[&str]) -> Output {
    scratch::gear(home, args).output().expect("run gear")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

/// A scratch home under the temp dir (no new dependency).
fn scratch_home(tag: &str) -> PathBuf {
    scratch::home(tag)
}

/// The skills directory of an agent CLI, under the given home.
fn skills_dir(home: &Path, agent: &str) -> PathBuf {
    match agent {
        "claude" => home.join(".claude").join("skills").join("gear"),
        "codex" => home.join(".codex").join("skills").join("gear"),
        "opencode" => home
            .join(".config")
            .join("opencode")
            .join("skills")
            .join("gear"),
        "grok" => home.join(".grok").join("skills").join("gear"),
        // T-113: pi's agent directory is `~/.pi/agent`; its user skills live
        // in `<agent-dir>/skills` (pi 1.4.0, docs/configuration.md).
        "pi" => home.join(".pi").join("agent").join("skills").join("gear"),
        other => panic!("unknown agent {other}"),
    }
}

/// The skill file of the repo, the source of the installed text.
fn repo_skill(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills/gear")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_skill_is_written_for_every_agent() {
    for agent in ["claude", "codex", "opencode", "grok", "pi"] {
        let home = scratch_home(agent);
        let o = gear(&home, &["admin", "install-skill", "--agent", agent]);
        assert!(o.status.success(), "{agent}: {}", stderr(&o));
        let out = stdout(&o);
        assert!(out.contains(agent), "{agent}: {out}");

        let dir = skills_dir(&home, agent);
        assert!(out.contains(&dir.display().to_string()), "{agent}: {out}");

        // The entry point and every reference are there, with the text the
        // binary prints.
        for rel in [
            "SKILL.md",
            "references/conventions.md",
            "references/course.md",
            "references/decompose.md",
            "references/review.md",
            "references/captain.md",
            "references/agents/opencode.md",
            "references/agents/codex.md",
            "references/agents/claude.md",
            "references/agents/grok.md",
            "references/agents/pi.md",
            "references/adopt.md",
            "references/github.md",
            // The templates a captain copies into a project (T-85).
            "templates/adopt.md",
            "templates/standard.md",
            "templates/handover.md",
        ] {
            let path = dir.join(rel);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(text, repo_skill(rel), "{agent}: {rel}");
            assert!(out.contains(&path.display().to_string()), "{agent}: {out}");
        }

        std::fs::remove_dir_all(&home).ok();
    }
}

#[test]
fn an_unknown_agent_is_bad_format() {
    let home = scratch_home("unknown");
    let o = gear(&home, &["admin", "install-skill", "--agent", "emacs"]);
    assert_eq!(o.status.code(), Some(1));
    let e = stderr(&o);
    assert!(
        e.starts_with("ERROR BAD_FORMAT: unknown agent \"emacs\""),
        "{e}"
    );
    assert!(e.contains("claude"), "{e}");
    // Nothing was written.
    assert!(!home.join(".claude").exists(), "{}", home.display());
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn an_agent_never_installs_into_the_users_home() {
    let home = scratch_home("agent");
    let o = scratch::gear(&home, &["admin", "install-skill", "--agent", "claude"])
        .env("GEAR_CTX", "acme:captain:1")
        .output()
        .expect("run gear");
    assert_eq!(o.status.code(), Some(1));
    let e = stderr(&o);
    assert!(e.starts_with("ERROR FORBIDDEN: "), "{e}");
    assert!(!home.join(".claude").exists(), "{}", home.display());
    std::fs::remove_dir_all(&home).ok();
}
