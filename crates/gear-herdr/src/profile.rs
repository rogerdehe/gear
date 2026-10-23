//! Per-CLI observation profile (design/04).

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScreenFlag {
    /// Still running a turn/tool despite an idle/done status.
    Busy,
    RateLimit,
    Quota,
    Auth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Herdr agent kind: `opencode`, `claude`, `codex`, `pi`…
    pub kind: String,
    pub busy_markers: Vec<String>,
    pub ratelimit_markers: Vec<String>,
    pub quota_markers: Vec<String>,
    pub auth_markers: Vec<String>,
    /// Only the last `tail_lines` lines are matched.
    pub tail_lines: u32,
    /// A flag counts only after this many consecutive observations.
    pub confirm_rounds: u32,
    /// CLI flags that turn off interactive permission prompts (opencode:
    /// `--auto`). Passed on every start and resume. Required: an agent that
    /// stops on a permission dialog cannot be driven unattended, so a profile
    /// with this empty is never `verified`.
    pub unattended_args: Vec<String>,
    /// Herdr reports `agent_session` for this kind.
    pub session_ref: bool,
    /// Args to resume a session; `{session}` is replaced. None = unsupported.
    pub resume_args: Option<Vec<String>>,
    /// Keys that make the agent exit cleanly before a release.
    pub exit_keys: Vec<String>,
    /// Probe-verified. Unverified profiles may not take over real projects.
    pub verified: bool,
}

/// Flags present in the last `profile.tail_lines` lines of `tail`.
/// Matching is literal substring per line (no regex). Pure function.
///
/// Only the tail window is inspected: an error that scrolled out of it is not
/// a current fact. Empty markers are ignored (they would match every line).
pub fn classify_screen(profile: &Profile, tail: &str) -> BTreeSet<ScreenFlag> {
    let lines: Vec<&str> = tail.lines().collect();
    let start = lines.len().saturating_sub(profile.tail_lines as usize);
    let window = &lines[start..];

    let mut flags = BTreeSet::new();
    for line in window {
        if matches_any(&profile.busy_markers, line) {
            flags.insert(ScreenFlag::Busy);
        }
        if matches_any(&profile.ratelimit_markers, line) {
            flags.insert(ScreenFlag::RateLimit);
        }
        if matches_any(&profile.quota_markers, line) {
            flags.insert(ScreenFlag::Quota);
        }
        if matches_any(&profile.auth_markers, line) {
            flags.insert(ScreenFlag::Auth);
        }
    }
    flags
}

/// Reject a prompt template that contains any marker of `profile`
/// (old keeper incident: the nudge text tripped its own rate-limit detector).
/// Returns the offending marker in the error.
pub fn validate_template(profile: &Profile, template: &str) -> Result<(), String> {
    let lists = [
        ("busy", &profile.busy_markers),
        ("ratelimit", &profile.ratelimit_markers),
        ("quota", &profile.quota_markers),
        ("auth", &profile.auth_markers),
    ];
    for (kind, markers) in lists {
        for marker in markers {
            if marker.is_empty() {
                continue;
            }
            if template.contains(marker.as_str()) {
                return Err(format!("template must not contain the {kind} marker: {marker}"));
            }
        }
    }
    Ok(())
}

/// Literal substring match of `line` against a marker list.
fn matches_any(markers: &[String], line: &str) -> bool {
    markers.iter().any(|m| !m.is_empty() && line.contains(m.as_str()))
}

/// Built-in profile for an agent kind, from the verified facts in
/// `gear --skill agent:<kind>` (design/04, /16). Only kinds with a verified SOP
/// are built in: today `opencode` —
/// busy `esc interrupt`; quota `Session failed`, `Token Plan`, `(2056)`,
/// `Free usage limit reached`; no rate-limit/auth markers yet; tail 16 lines,
/// confirm 2 rounds; unattended `--auto`; session_ref false; resume
/// `["-s", "{session}"]`; exit `["ctrl+c"]`; verified. Any other kind → None.
pub fn builtin(kind: &str) -> Option<Profile> {
    match kind {
        "opencode" => Some(Profile {
            kind: "opencode".into(),
            busy_markers: vec!["esc interrupt".into()],
            ratelimit_markers: vec![],
            quota_markers: vec![
                "Session failed".into(),
                "Token Plan".into(),
                "(2056)".into(),
                "Free usage limit reached".into(),
            ],
            auth_markers: vec![],
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--auto".into()],
            session_ref: false,
            resume_args: Some(vec!["-s".into(), "{session}".into()]),
            exit_keys: vec!["ctrl+c".into()],
            verified: true,
        }),
        // Claude Code as captain (seen on 2026-10-22: Herdr reports its
        // working/idle and its session id). No quota markers: the captain's
        // tail is conversation text that may quote them; GEAR does not manage
        // the captain's quota.
        "claude" => Some(Profile {
            kind: "claude".into(),
            busy_markers: vec!["esc to interrupt".into()],
            ratelimit_markers: vec![],
            quota_markers: vec![],
            auth_markers: vec![],
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--dangerously-skip-permissions".into()],
            session_ref: true,
            resume_args: Some(vec!["--resume".into(), "{session}".into()]),
            exit_keys: vec!["ctrl+c".into(), "ctrl+c".into()],
            verified: true,
        }),
        // Only kinds with a verified SOP are built in (design/04).
        _ => None,
    }
}
