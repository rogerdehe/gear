//! Project config (design/01, conventions 17 §1–§2, §11). Pure: parse TOML
//! text, apply defaults, validate what can be checked without Herdr or git.
//! Checks that need the outside world (session exists, repo/branch exist,
//! profile verified) live in `gear admin init`, not here.

use gear_model::GearResult;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub quota: Quota,
    pub captain: CaptainSection,
    #[serde(default)]
    pub lanes: Vec<LaneSection>,
    #[serde(default)]
    pub temps: Option<TempsSection>,
    #[serde(default)]
    pub conventions: Conventions,
    #[serde(default)]
    pub timing: Timing,
    /// Per-kind profile overrides, kept as raw TOML; merged by the engine (04).
    #[serde(default)]
    pub profiles: BTreeMap<String, toml::Table>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    pub id: String,
    pub herdr_session: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quota {
    pub lanes: u32,
    pub temps: u32,
    #[serde(default)]
    pub includes_captain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptainSection {
    pub pane: String,
    pub kind: String,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSection {
    pub name: String,
    pub duty: String,
    pub owns: Vec<String>,
    #[serde(default)]
    pub not: Vec<String>,
    pub kind: String,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TempsSection {
    pub kind: String,
    pub profile: String,
}

/// Defaults: integration "dev", release "main", staging "" (none), review_max_rounds 3.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Conventions {
    pub integration: String,
    pub release: String,
    pub staging: String,
    pub review_max_rounds: u32,
}

impl Default for Conventions {
    fn default() -> Self {
        todo!("T-7")
    }
}

/// Defaults: tick_s 4, cooldown_s 90, sample_min 20, review_timer_min 45.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Timing {
    pub tick_s: u32,
    pub cooldown_s: u32,
    pub sample_min: u32,
    pub review_timer_min: u32,
}

impl Default for Timing {
    fn default() -> Self {
        todo!("T-7")
    }
}

/// One validation finding. `field` is a dotted path such as `lanes[1].owns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub field: String,
    pub problem: String,
}

/// Parse TOML text. Syntax errors, unknown keys, wrong types and missing
/// required keys → `ErrorCode::BadFormat` with the TOML error message as detail.
pub fn parse(text: &str) -> GearResult<ProjectConfig> {
    todo!("T-7")
}

/// All findings, in field order (project, quota, captain, lanes in order,
/// conventions, timing); empty = valid. Rules:
/// - `project.id`, every lane `name`: `[a-z][a-z0-9-]*` (`gear_model::valid_name`
///   is NOT this rule — check it here);
/// - `<project.id>-<lane.name>` and `<project.id>-captain` ≤ 32 chars (Herdr agent names);
/// - lane names unique; lane `duty` non-empty after trim; `owns` non-empty;
/// - `owns` entries of two different lanes must not overlap: equal, or one is a
///   path prefix of the other on a `/` boundary (`src/a/` vs `src/a/b.rs` overlap;
///   `src/a` vs `src/ab` do not);
/// - number of lanes ≤ `quota.lanes`;
/// - `conventions.integration` and `release` non-empty and different; `staging`
///   if non-empty differs from both;
/// - `timing.cooldown_s > timing.tick_s`.
pub fn validate(cfg: &ProjectConfig) -> Vec<ConfigIssue> {
    todo!("T-7")
}
