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
        text: include_str!("../../../skills/gear/references/conventions.md"),
    },
    Topic {
        name: "course",
        file: "references/course.md",
        text: include_str!("../../../skills/gear/references/course.md"),
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
        name: "departure",
        file: "references/departure.md",
        text: include_str!("../../../skills/gear/references/departure.md"),
    },
    Topic {
        name: "adopt",
        file: "references/adopt.md",
        text: include_str!("../../../skills/gear/references/adopt.md"),
    },
    Topic {
        name: "github",
        file: "references/github.md",
        text: include_str!("../../../skills/gear/references/github.md"),
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
    Topic {
        name: "agent:grok",
        file: "references/agents/grok.md",
        text: include_str!("../../../skills/gear/references/agents/grok.md"),
    },
    Topic {
        name: "agent:pi",
        file: "references/agents/pi.md",
        text: include_str!("../../../skills/gear/references/agents/pi.md"),
    },
];

/// The templates that ship with the skill (T-85): files the captain copies into
/// a project's `gear/` directory, installed next to the topics but not printed
/// as a topic — `gear --skill adopt` explains them.
pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "templates/adopt.md",
        include_str!("../../../skills/gear/templates/adopt.md"),
    ),
    (
        "templates/standard.md",
        include_str!("../../../skills/gear/templates/standard.md"),
    ),
    (
        "templates/handover.md",
        include_str!("../../../skills/gear/templates/handover.md"),
    ),
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
    Grok,
    Pi,
}

impl SkillAgent {
    pub const ALL: &'static [SkillAgent] = &[
        SkillAgent::Claude,
        SkillAgent::Codex,
        SkillAgent::OpenCode,
        SkillAgent::Grok,
        SkillAgent::Pi,
    ];

    /// The name the user writes.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillAgent::Claude => "claude",
            SkillAgent::Codex => "codex",
            SkillAgent::OpenCode => "opencode",
            SkillAgent::Grok => "grok",
            SkillAgent::Pi => "pi",
        }
    }

    /// Parse the agent name; anything else is `BadFormat`.
    pub fn parse(name: &str) -> GearResult<Self> {
        match name {
            "claude" => Ok(SkillAgent::Claude),
            "codex" => Ok(SkillAgent::Codex),
            "opencode" => Ok(SkillAgent::OpenCode),
            "grok" => Ok(SkillAgent::Grok),
            "pi" => Ok(SkillAgent::Pi),
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
    /// OpenCode `~/.config/opencode/skills/<name>/SKILL.md`, Grok
    /// `~/.grok/skills/<name>/SKILL.md` (its user-guide, `08-skills.md`).
    /// Pi reads `<agent-dir>/skills/<name>/SKILL.md` and its agent directory
    /// is `~/.pi/agent` (pi 1.4.0, `docs/configuration.md`: "User skills and
    /// supporting files" live in `<agent-dir>/skills/`), so the skill goes to
    /// `~/.pi/agent/skills/<name>/SKILL.md`.
    pub fn skills_root(self, home: &Path) -> PathBuf {
        match self {
            SkillAgent::Claude => home.join(".claude").join("skills"),
            SkillAgent::Codex => home.join(".codex").join("skills"),
            SkillAgent::OpenCode => home.join(".config").join("opencode").join("skills"),
            SkillAgent::Grok => home.join(".grok").join("skills"),
            SkillAgent::Pi => home.join(".pi").join("agent").join("skills"),
        }
    }
}

/// Is the GEAR skill installed for `agent` under `home` (T-83)? The CLI loads
/// `<skills root>/gear/SKILL.md`; a missing file means the agent never learns
/// how to work as a seat. The home is explicit: the caller knows whose home it
/// is (the user's, for `gear admin init`).
pub fn installed(agent: SkillAgent, home: &Path) -> bool {
    agent
        .skills_root(home)
        .join(SKILL_NAME)
        .join("SKILL.md")
        .is_file()
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
    // The templates a captain copies into a project (T-85).
    for (file, text) in TEMPLATES {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GearError::new(
                    ErrorCode::BadFormat,
                    format!("cannot create {}: {e}", parent.display()),
                )
            })?;
        }
        std::fs::write(&path, text).map_err(|e| {
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
        crate::testtmp::temp(&format!("gear-skill-{tag}"))
    }

    #[test]
    fn every_topic_is_installed_as_a_file() {
        let home = scratch_home("install");
        let written = install(SkillAgent::Claude, &home).unwrap();
        assert_eq!(written.len(), TOPICS.len() + TEMPLATES.len());
        for topic in TOPICS {
            let path = home
                .join(".claude")
                .join("skills")
                .join("gear")
                .join(topic.file);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                topic.text,
                "{}",
                path.display()
            );
        }
        // The templates travel with the skill too (T-85): a captain copies
        // them into a project's `gear/` directory.
        for (file, text) in TEMPLATES {
            let path = home.join(".claude").join("skills").join("gear").join(file);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                *text,
                "{}",
                path.display()
            );
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
        assert_eq!(
            SkillAgent::Grok.skills_root(&home),
            home.join(".grok").join("skills")
        );
        // Pi's agent directory is `~/.pi/agent`; its user skills live in
        // `<agent-dir>/skills` (pi 1.4.0, docs/configuration.md).
        assert_eq!(
            SkillAgent::Pi.skills_root(&home),
            home.join(".pi").join("agent").join("skills")
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
