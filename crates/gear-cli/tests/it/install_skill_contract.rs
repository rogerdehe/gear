//! Contract for `gear admin install-skill --agent <cli>` (T-30): the skill
//! (SKILL.md and its references) is written where that agent CLI loads its
//! skills, and the command says where it wrote.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn gear(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(args)
        .env("HOME", home)
        .env_remove("GEAR_CTX")
        // Never the real store: a `gear` call starts an engine on it
        // (design/14). The install writes no store at all.
        .env_remove("GEAR_DB")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

/// A scratch home under the temp dir (no new dependency).
fn scratch_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gear-install-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The skills directory of an agent CLI, under the given home.
fn skills_dir(home: &Path, agent: &str) -> PathBuf {
    match agent {
        "claude" => home.join(".claude").join("skills").join("gear"),
        "codex" => home.join(".codex").join("skills").join("gear"),
        "opencode" => home.join(".config").join("opencode").join("skills").join("gear"),
        "grok" => home.join(".grok").join("skills").join("gear"),
        other => panic!("unknown agent {other}"),
    }
}

/// The skill file of the repo, the source of the installed text.
fn repo_skill(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/gear").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_skill_is_written_for_every_agent() {
    for agent in ["claude", "codex", "opencode", "grok"] {
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
            "references/decompose.md",
            "references/review.md",
            "references/captain.md",
            "references/agents/opencode.md",
            "references/agents/codex.md",
            "references/agents/claude.md",
            "references/agents/grok.md",
        ] {
            let path = dir.join(rel);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if rel == "references/conventions.md" {
                // The conventions ship as the design text of this version.
                assert!(text.contains("# 17 Default working conventions"), "{rel}");
            } else {
                assert_eq!(text, repo_skill(rel), "{agent}: {rel}");
            }
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
    assert!(e.starts_with("ERROR BAD_FORMAT: unknown agent \"emacs\""), "{e}");
    assert!(e.contains("claude"), "{e}");
    // Nothing was written.
    assert!(!home.join(".claude").exists(), "{}", home.display());
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn an_agent_never_installs_into_the_users_home() {
    let home = scratch_home("agent");
    let o = Command::new(env!("CARGO_BIN_EXE_gear"))
        .args(["admin", "install-skill", "--agent", "claude"])
        .env("HOME", &home)
        .env("GEAR_CTX", "acme:captain:1")
        .env_remove("GEAR_DB")
        .output()
        .expect("run gear");
    assert_eq!(o.status.code(), Some(1));
    let e = stderr(&o);
    assert!(e.starts_with("ERROR FORBIDDEN: "), "{e}");
    assert!(!home.join(".claude").exists(), "{}", home.display());
    std::fs::remove_dir_all(&home).ok();
}
