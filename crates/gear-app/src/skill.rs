//! The GEAR skill: the texts `gear --skill` prints and where each agent CLI
//! loads its skills (T-30).
//!
//! The skill is compiled into the binary, so the printed rules always match
//! the running version. `gear admin install-skill` writes the same texts as
//! files into the skill directory of an agent CLI, so the CLI finds the skill
//! where it looks for one.

use gear_model::{ErrorCode, GearError, GearResult};
use std::path::{Path, PathBuf};

/// One skill topic: the name `gear --skill <name>` takes, the file it is
/// stored as inside the skill directory, and its text.
pub struct Topic {
    pub name: &'static str,
    /// The path inside the installed skill directory.
    pub file: &'static str,
    pub text: &'static str,
}

/// The topics of this binary's skill. `main` is the entry point (the bare
/// `--skill`); every other topic is a reference next to it.
pub const TOPICS: &[Topic] = &[
    Topic {
        name: "main",
        file: "SKILL.md",
        text: include_str!("../../../skills/gear/SKILL.md"),
    },
    Topic {
        name: "conventions",
        file: "references/conventions.md",
        text: include_str!("../../../docs/design/17-conventions.md"),
    },
    Topic {
        name: "decompose",
        file: "references/decompose.md",
        text: include_str!("../../../skills/gear/references/decompose.md"),
    },
    Topic {
        name: "review",
        file: "references/review.md",
        text: include_str!("../../../skills/gear/references/review.md"),
    },
    Topic {
        name: "captain",
        file: "references/captain.md",
        text: include_str!("../../../skills/gear/references/captain.md"),
    },
    Topic {
        name: "agent:opencode",
        file: "references/agents/opencode.md",
        text: include_str!("../../../skills/gear/references/agents/opencode.md"),
    },
    Topic {
        name: "agent:codex",
        file: "references/agents/codex.md",
        text: include_str!("../../../skills/gear/references/agents/codex.md"),
    },
    Topic {
        name: "agent:claude",
        file: "references/agents/claude.md",
        text: include_str!("../../../skills/gear/references/agents/claude.md"),
    },
];

/// The name of the skill: the directory it is installed as, and the name in
/// the frontmatter of `SKILL.md` (the CLI matches the two).
pub const SKILL_NAME: &str = "gear";

/// The agent CLIs that load skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAgent {
    Claude,
    Codex,
    OpenCode,
}

impl SkillAgent {
    pub const ALL: &'static [SkillAgent] =
        &[SkillAgent::Claude, SkillAgent::Codex, SkillAgent::OpenCode];

    /// The name the user writes.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillAgent::Claude => "claude",
            SkillAgent::Codex => "codex",
            SkillAgent::OpenCode => "opencode",
        }
    }

    /// Parse the agent name; anything else is `BadFormat`.
    pub fn parse(name: &str) -> GearResult<Self> {
        match name {
            "claude" => Ok(SkillAgent::Claude),
            "codex" => Ok(SkillAgent::Codex),
            "opencode" => Ok(SkillAgent::OpenCode),
            other => {
                let known = SkillAgent::ALL
                    .iter()
                    .map(|agent| agent.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(GearError::new(
                    ErrorCode::BadFormat,
                    format!("unknown agent {other:?}: the skill installs for {known}"),
                ))
            }
        }
    }

    /// The directory this CLI loads skills from, under the user's home.
    ///
    /// Verified on this machine (2026-10-23): Claude Code reads
    /// `~/.claude/skills/<name>/SKILL.md`, Codex `~/.codex/skills/<name>/SKILL.md`,
    /// OpenCode `~/.config/opencode/skills/<name>/SKILL.md`.
    pub fn skills_root(self, home: &Path) -> PathBuf {
        match self {
            SkillAgent::Claude => home.join(".claude").join("skills"),
            SkillAgent::Codex => home.join(".codex").join("skills"),
            SkillAgent::OpenCode => home.join(".config").join("opencode").join("skills"),
        }
    }
}

/// Install the skill for `agent` under `home`: every topic's text as a file in
/// `<the CLI's skills dir>/gear/`. An existing file is overwritten (the
/// installed copy follows the binary). Returns the files written.
pub fn install(agent: SkillAgent, home: &Path) -> GearResult<Vec<PathBuf>> {
    let dir = agent.skills_root(home).join(SKILL_NAME);
    let mut written = Vec::new();
    for topic in TOPICS {
        let path = dir.join(topic.file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("cannot create {}: {e}", parent.display()),
                )
            })?;
        }
        std::fs::write(&path, topic.text).map_err(|e| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot write {}: {e}", path.display()),
            )
        })?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch home under the temp dir (no new dependency).
    fn scratch_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gear-skill-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_topic_is_installed_as_a_file() {
        let home = scratch_home("install");
        let written = install(SkillAgent::Claude, &home).unwrap();
        assert_eq!(written.len(), TOPICS.len());
        for topic in TOPICS {
            let path = home.join(".claude").join("skills").join("gear").join(topic.file);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), topic.text, "{}", path.display());
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn each_agent_has_its_own_skills_dir() {
        let home = scratch_home("dirs");
        assert_eq!(
            SkillAgent::Claude.skills_root(&home),
            home.join(".claude").join("skills")
        );
        assert_eq!(
            SkillAgent::Codex.skills_root(&home),
            home.join(".codex").join("skills")
        );
        assert_eq!(
            SkillAgent::OpenCode.skills_root(&home),
            home.join(".config").join("opencode").join("skills")
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_unknown_agent_is_bad_format() {
        let err = SkillAgent::parse("emacs").unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat);
        assert!(err.detail.contains("claude"), "{}", err.detail);
        assert!(SkillAgent::parse("claude").is_ok());
    }
}
