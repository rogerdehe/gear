//! Project config (design/01, conventions 17 §1–§2, §11). Pure: parse TOML
//! text, apply defaults, validate what can be checked without Herdr or git.
//! Checks that need the outside world (session exists, repo/branch exist,
//! profile verified) live in `gear admin init`, not here.

use gear_model::{ErrorCode, GearError, GearResult};
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
        Self {
            integration: "dev".to_string(),
            release: "main".to_string(),
            staging: String::new(),
            review_max_rounds: 3,
        }
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
        Self {
            tick_s: 4,
            cooldown_s: 90,
            sample_min: 20,
            review_timer_min: 45,
        }
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
    toml::from_str::<ProjectConfig>(text)
        .map_err(|e| GearError::new(ErrorCode::BadFormat, e.to_string()))
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
    let mut issues = Vec::new();

    // ---- project
    if !is_name(&cfg.project.id) {
        issues.push(issue(
            "project.id",
            format!(
                "project id {:?} must match [a-z][a-z0-9-]*",
                cfg.project.id
            ),
        ));
    }
    if !agent_name_fits(&cfg.project.id, "captain") {
        issues.push(issue(
            "project.id",
            format!(
                "agent name {:?} is longer than 32 characters",
                agent_name(&cfg.project.id, "captain")
            ),
        ));
    }

    // ---- quota
    if cfg.lanes.len() as u32 > cfg.quota.lanes {
        issues.push(issue(
            "quota.lanes",
            format!(
                "{} lanes are configured but the quota allows {}",
                cfg.lanes.len(),
                cfg.quota.lanes
            ),
        ));
    }

    // ---- captain: nothing to check here (pane, kind and profile are checked
    // against Herdr by `gear admin init`).

    // ---- lanes, in order
    let mut first_lane_with_name: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, lane) in cfg.lanes.iter().enumerate() {
        let field = format!("lanes[{index}].name");
        if !is_name(&lane.name) {
            issues.push(issue(
                &field,
                format!("lane name {:?} must match [a-z][a-z0-9-]*", lane.name),
            ));
        }
        if !agent_name_fits(&cfg.project.id, &lane.name) {
            issues.push(issue(
                &field,
                format!(
                    "agent name {:?} is longer than 32 characters",
                    agent_name(&cfg.project.id, &lane.name)
                ),
            ));
        }
        match first_lane_with_name.get(lane.name.as_str()) {
            Some(first) => issues.push(issue(
                &field,
                format!("lane name {:?} is already used by lanes[{first}]", lane.name),
            )),
            None => {
                first_lane_with_name.insert(lane.name.as_str(), index);
            }
        }

        if lane.duty.trim().is_empty() {
            issues.push(issue(
                &format!("lanes[{index}].duty"),
                "a lane must state its duty (conventions 17 §1)".to_string(),
            ));
        }

        let owns_field = format!("lanes[{index}].owns");
        let owns: Vec<&str> = lane.owns.iter().map(|entry| entry.trim()).collect();
        if owns.is_empty() {
            issues.push(issue(
                &owns_field,
                "a lane must state what it owns (conventions 17 §1)".to_string(),
            ));
        } else if owns.iter().any(|entry| entry.is_empty()) {
            issues.push(issue(
                &owns_field,
                "an owns entry is empty after trimming".to_string(),
            ));
        } else if let Some(other) = cfg.lanes[..index]
            .iter()
            .find(|earlier| owns_overlap(&earlier.owns, &lane.owns))
        {
            issues.push(issue(
                &owns_field,
                format!("overlaps the paths owned by lane {:?}", other.name),
            ));
        }
    }

    // ---- conventions
    let conventions = &cfg.conventions;
    let integration = conventions.integration.trim();
    let release = conventions.release.trim();
    let staging = conventions.staging.trim();
    if integration.is_empty() {
        issues.push(issue(
            "conventions.integration",
            "the integration branch must not be empty".to_string(),
        ));
    }
    if release.is_empty() {
        issues.push(issue(
            "conventions.release",
            "the release branch must not be empty".to_string(),
        ));
    }
    if !integration.is_empty() && !release.is_empty() && integration == release {
        issues.push(issue(
            "conventions.release",
            format!("the release branch must differ from the integration branch {integration:?}"),
        ));
    }
    // A blank staging branch means "no staging branch".
    if !staging.is_empty() {
        let clash = if staging == integration {
            Some(integration)
        } else if staging == release {
            Some(release)
        } else {
            None
        };
        if let Some(other) = clash {
            issues.push(issue(
                "conventions.staging",
                format!("the staging branch must differ from {other:?}"),
            ));
        }
    }

    // ---- timing
    if cfg.timing.cooldown_s <= cfg.timing.tick_s {
        issues.push(issue(
            "timing.cooldown_s",
            format!(
                "the cooldown ({} s) must be longer than the tick ({} s)",
                cfg.timing.cooldown_s, cfg.timing.tick_s
            ),
        ));
    }

    issues
}

// ---------------------------------------------------------------- helpers

fn issue(field: &str, problem: String) -> ConfigIssue {
    ConfigIssue {
        field: field.to_string(),
        problem,
    }
}

/// Project and lane names: `[a-z][a-z0-9-]*` (conventions 17 §2).
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The Herdr agent name of a seat: `<project>-<seat>`.
fn agent_name(project: &str, seat: &str) -> String {
    format!("{project}-{seat}")
}

/// Herdr agent names are at most 32 characters long.
fn agent_name_fits(project: &str, seat: &str) -> bool {
    agent_name(project, seat).len() <= 32
}

/// Two path lists overlap when a path is equal to another or one is its
/// prefix on a `/` boundary: `src/a/` and `src/a/b.rs` overlap,
/// `src/a` and `src/ab` do not. Entries are compared after trimming.
fn owns_overlap(earlier: &[String], later: &[String]) -> bool {
    earlier
        .iter()
        .any(|a| later.iter().any(|b| paths_overlap(a.trim(), b.trim())))
}

fn paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if let Some(rest) = b.strip_prefix(a) {
        return rest.starts_with('/') || a.ends_with('/');
    }
    if let Some(rest) = a.strip_prefix(b) {
        return rest.starts_with('/') || b.ends_with('/');
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_lane_rule() {
        for good in ["heat", "a", "a-b", "lane-2", "x9"] {
            assert!(is_name(good), "{good}");
        }
        for bad in ["", "Heat", "9oxford", "a b", "a_b", "-a", "heat!"] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn agent_names_are_at_most_32_characters() {
        assert!(agent_name_fits("oxford", "heat"));
        assert!(agent_name_fits("oxford", "captain"));
        assert!(!agent_name_fits("oxford", "a-very-long-lane-name-for-oxford"));
    }

    #[test]
    fn paths_overlap_only_on_path_boundaries() {
        assert!(paths_overlap("src/heat/", "src/heat/model.rs"));
        assert!(paths_overlap("src/heat/model.rs", "src/heat/"));
        assert!(paths_overlap("tests/heat/", "tests/heat/"));
        assert!(paths_overlap("src/a", "src/a/b.rs"));
        assert!(!paths_overlap("src/a", "src/ab"));
        assert!(!paths_overlap("src/heat/", "tests/heat/"));
        assert!(!paths_overlap("src/heat/", "src/pricing/"));
    }
}
