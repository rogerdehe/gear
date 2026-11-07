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

/// The file an installed skill copy carries the version that wrote it (T-681):
/// a captain's page names every copy older than the binary.
pub const VERSION_FILE: &str = ".gear-version";

/// The agent CLIs that load skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAgent {
    Claude,
    Codex,
    OpenCode,
    Grok,
    Pi,
    Omp,
    Agy,
}

impl SkillAgent {
    pub const ALL: &'static [SkillAgent] = &[
        SkillAgent::Claude,
        SkillAgent::Codex,
        SkillAgent::OpenCode,
        SkillAgent::Grok,
        SkillAgent::Pi,
        SkillAgent::Omp,
        SkillAgent::Agy,
    ];

    /// The name the user writes.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillAgent::Claude => "claude",
            SkillAgent::Codex => "codex",
            SkillAgent::OpenCode => "opencode",
            SkillAgent::Grok => "grok",
            SkillAgent::Pi => "pi",
            SkillAgent::Omp => "omp",
            SkillAgent::Agy => "agy",
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
            "omp" => Ok(SkillAgent::Omp),
            "agy" => Ok(SkillAgent::Agy),
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
    /// omp (oh-my-pi, a pi fork) reads `<agent-dir>/skills/<name>/SKILL.md`
    /// like pi, and `omp config path` names its agent directory
    /// `~/.omp/agent` (omp 18.12.6), so the skill goes to
    /// `~/.omp/agent/skills/<name>/SKILL.md`.
    /// agy (Antigravity CLI 1.4.12) loads global skills from
    /// `~/.gemini/antigravity-cli/skills/<name>/SKILL.md` (agy's `/skills`
    /// and its customizations guide: workspace skills live under
    /// `_agents/skills/`, global ones under the CLI's config directory), so
    /// the skill goes to `~/.gemini/antigravity-cli/skills/<name>/SKILL.md`.
    pub fn skills_root(self, home: &Path) -> PathBuf {
        match self {
            SkillAgent::Claude => home.join(".claude").join("skills"),
            SkillAgent::Codex => home.join(".codex").join("skills"),
            SkillAgent::OpenCode => home.join(".config").join("opencode").join("skills"),
            SkillAgent::Grok => home.join(".grok").join("skills"),
            SkillAgent::Pi => home.join(".pi").join("agent").join("skills"),
            SkillAgent::Omp => home.join(".omp").join("agent").join("skills"),
            SkillAgent::Agy => home.join(".gemini").join("antigravity-cli").join("skills"),
        }
    }

    /// Where this machine's CLI reads the skill: [`Self::skills_root`], except
    /// that Codex reads `$CODEX_HOME/skills` when `CODEX_HOME` is set. A copy
    /// under `~/.codex` would never reach an agent started with it.
    pub fn skills_dir(self, home: &Path) -> PathBuf {
        let codex_home = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty());
        self.skills_dir_with(home, codex_home.as_deref().map(Path::new))
    }

    /// [`Self::skills_dir`] with the `CODEX_HOME` given.
    pub fn skills_dir_with(self, home: &Path, codex_home: Option<&Path>) -> PathBuf {
        match (self, codex_home) {
            (SkillAgent::Codex, Some(codex_home)) => codex_home.join("skills"),
            _ => self.skills_root(home),
        }
    }
}

/// The installed skill copies older than `version` (T-681): one line per
/// agent whose copy carries an older version, an unparseable one, or none.
pub fn stale_copies(home: &Path, codex_home: Option<&Path>, version: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for agent in SkillAgent::ALL {
        let dir = agent.skills_dir_with(home, codex_home).join(SKILL_NAME);
        if !dir.join("SKILL.md").is_file() {
            continue;
        }
        let old = std::fs::read_to_string(dir.join(VERSION_FILE))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());
        let stale = match &old {
            None => true,
            Some(old) => match (version_parts(old), version_parts(version)) {
                (Some(old), Some(version)) => old < version,
                _ => old != version,
            },
        };
        if !stale {
            continue;
        }
        let old = old.unwrap_or_else(|| "an unknown version".to_string());
        lines.push(format!(
            "skill copy for {} is from {old}, gear is {version}: run gear admin install-skill --agent {}",
            agent.as_str(),
            agent.as_str()
        ));
    }
    lines
}

/// A version as its dot-separated numbers, `None` when it is not one.
fn version_parts(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
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
    install_into(&agent.skills_root(home))
}

/// Install the skill into a CLI's skills directory `root` (the caller has
/// resolved it, for Codex with `CODEX_HOME`: [`SkillAgent::skills_dir`]).
pub fn install_into(root: &Path) -> GearResult<Vec<PathBuf>> {
    let dir = root.join(SKILL_NAME);
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
    // T-681: the version that wrote this copy, beside the skill (not part of
    // the file list: the counts stay as they are).
    let version_path = dir.join(VERSION_FILE);
    std::fs::write(&version_path, format!("{}\n", env!("CARGO_PKG_VERSION"))).map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot write {}: {e}", version_path.display()),
        )
    })?;
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
    fn codex_reads_its_skill_from_codex_home_when_it_is_set() {
        let home = Path::new("/home/u");
        assert_eq!(
            SkillAgent::Codex.skills_dir_with(home, Some(Path::new("/data/codex-home"))),
            Path::new("/data/codex-home/skills")
        );
        assert_eq!(
            SkillAgent::Codex.skills_dir_with(home, None),
            home.join(".codex").join("skills")
        );
        assert_eq!(
            SkillAgent::Claude.skills_dir_with(home, Some(Path::new("/data/codex-home"))),
            home.join(".claude").join("skills")
        );
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
        // omp's agent directory is `~/.omp/agent` (`omp config path`, omp
        // 18.12.6), and its user skills live in `<agent-dir>/skills` like pi's.
        assert_eq!(
            SkillAgent::Omp.skills_root(&home),
            home.join(".omp").join("agent").join("skills")
        );
        // agy's global skills are `~/.gemini/antigravity-cli/skills`
        // (agy 1.4.12 `/skills` and customizations guide).
        assert_eq!(
            SkillAgent::Agy.skills_root(&home),
            home.join(".gemini").join("antigravity-cli").join("skills")
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_install_writes_the_version_beside_the_skill() {
        let home = scratch_home("version");
        let written = install(SkillAgent::Pi, &home).unwrap();
        let dir = SkillAgent::Pi.skills_root(&home).join(SKILL_NAME);
        assert_eq!(
            std::fs::read_to_string(dir.join(VERSION_FILE))
                .unwrap()
                .trim(),
            env!("CARGO_PKG_VERSION")
        );
        assert!(std::fs::read_to_string(dir.join("SKILL.md"))
            .unwrap()
            .starts_with("---"));
        assert_eq!(written.len(), TOPICS.len() + TEMPLATES.len());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_copy_older_than_the_binary_is_named() {
        let home = scratch_home("older");
        install(SkillAgent::Pi, &home).unwrap();
        let dir = SkillAgent::Pi.skills_root(&home).join(SKILL_NAME);
        std::fs::write(dir.join(VERSION_FILE), "0.6.1\n").unwrap();
        assert_eq!(
            stale_copies(&home, None, "0.6.114"),
            vec![
                "skill copy for pi is from 0.6.1, gear is 0.6.114: run gear admin install-skill --agent pi"
                    .to_string()
            ]
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_current_or_newer_copy_is_not_named() {
        let home = scratch_home("current");
        install(SkillAgent::Pi, &home).unwrap();
        let dir = SkillAgent::Pi.skills_root(&home).join(SKILL_NAME);
        for version in ["0.6.114", "0.7.0"] {
            std::fs::write(dir.join(VERSION_FILE), format!("{version}\n")).unwrap();
            assert!(stale_copies(&home, None, "0.6.114").is_empty(), "{version}");
        }
        // Numeric, not text order: 99 is older than 114.
        std::fs::write(dir.join(VERSION_FILE), "0.6.99\n").unwrap();
        assert_eq!(stale_copies(&home, None, "0.6.114").len(), 1);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_copy_without_a_version_is_named_unknown() {
        let home = scratch_home("unknown");
        install(SkillAgent::Pi, &home).unwrap();
        let dir = SkillAgent::Pi.skills_root(&home).join(SKILL_NAME);
        std::fs::remove_file(dir.join(VERSION_FILE)).unwrap();
        let lines = stale_copies(&home, None, "0.6.114");
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("is from an unknown version"),
            "{}",
            lines[0]
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn no_copy_no_line() {
        let home = scratch_home("empty");
        assert!(stale_copies(&home, None, "0.6.114").is_empty());
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
