//! Per-CLI observation profile (design/04).

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScreenFlag {
    /// Still running a turn/tool despite an idle/done status.
    Busy,
    RateLimit,
    Quota,
    Auth,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    /// Herdr agent kind: `opencode`, `claude`, `codex`, `pi`…
    pub kind: String,
    pub busy_markers: Vec<String>,
    pub ratelimit_markers: Vec<String>,
    pub quota_markers: Vec<String>,
    /// Footer-only captain quota detection. Conversation markers never apply.
    pub quota_bar: Option<crate::QuotaBar>,
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
    /// A command that makes the agent exit, sent as a prompt (grok: `/exit`,
    /// because ctrl+c and ctrl+d do not exit it). When set, `exit_keys` is not
    /// used: the CLI takes a command, not a key.
    pub exit_command: Option<String>,
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
/// are built in: today `opencode`, `claude`, `codex` and `grok` —
/// opencode: busy `esc interrupt`; quota `Session failed`, `Token Plan`, `(2056)`,
/// `Free usage limit reached`; no rate-limit/auth markers yet; tail 16 lines,
/// confirm 2 rounds; unattended `--auto`; session_ref false; resume
/// `["-s", "{session}"]`; exit `["ctrl+c"]`; verified.
/// codex: busy `esc to interrupt`; no quota/rate-limit/auth markers yet (the
/// SOP says the limits are still to be recorded); tail 16 lines, confirm 2
/// rounds; unattended `--dangerously-bypass-approvals-and-sandbox` and
/// `-c check_for_update_on_startup=false`;
/// session_ref true (Herdr reports the session uuid); resume
/// `["resume", "{session}"]`; exit `["ctrl+c"]`; verified.
/// grok: busy `Ctrl+c:cancel`; unattended `--always-approve`; session_ref
/// true; resume `["-r", "{session}"]`; exit by the `/exit` command
/// (`exit_command`, no keys).
/// Any other kind → None.
impl Profile {
    /// The starting point for a profile the config defines (T-31): the screen
    /// window of design/04 — the last 16 lines, a flag confirmed after two
    /// consecutive observations. A derived `Default` (all zero) would read no
    /// line and never see a marker (review R2).
    pub fn for_new() -> Profile {
        Profile {
            tail_lines: 16,
            confirm_rounds: 2,
            ..Profile::default()
        }
    }
}

/// The fields a project config may set for a profile (T-31). Every field is
/// optional: a field left out keeps the value the profile already has (the
/// built-in's, when the name is a built-in).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    /// The Herdr agent kind (e.g. `grok`). Defaults to the profile's name.
    pub kind: Option<String>,
    /// The CLI flags that turn off interactive permission prompts.
    pub unattended_args: Option<Vec<String>>,
    pub busy_markers: Option<Vec<String>>,
    pub quota_markers: Option<Vec<String>>,
    pub quota_bar: Option<crate::QuotaBar>,
    pub ratelimit_markers: Option<Vec<String>>,
    pub auth_markers: Option<Vec<String>>,
    /// The keys that make the agent exit cleanly.
    pub exit_keys: Option<Vec<String>>,
    /// The args to resume a session; `{session}` is replaced.
    pub resume_args: Option<Vec<String>>,
    /// Herdr reports `agent_session` for this kind.
    pub session_ref: Option<bool>,
    /// Only the last `tail_lines` lines are matched.
    pub tail_lines: Option<u32>,
    /// A flag counts only after this many consecutive observations.
    pub confirm_rounds: Option<u32>,
}

impl Profile {
    /// The profile with the patch applied: the given fields win, the others
    /// keep their value.
    pub fn patched(&self, patch: &ProfilePatch, name: &str) -> Profile {
        let mut profile = self.clone();
        if let Some(kind) = &patch.kind {
            profile.kind = kind.clone();
        }
        if let Some(args) = &patch.unattended_args {
            profile.unattended_args = args.clone();
        }
        if let Some(markers) = &patch.busy_markers {
            profile.busy_markers = markers.clone();
        }
        if let Some(bar) = &patch.quota_bar {
            profile.quota_bar = Some(bar.clone());
        }
        if let Some(markers) = &patch.quota_markers {
            profile.quota_markers = markers.clone();
        }
        if let Some(markers) = &patch.ratelimit_markers {
            profile.ratelimit_markers = markers.clone();
        }
        if let Some(markers) = &patch.auth_markers {
            profile.auth_markers = markers.clone();
        }
        if let Some(keys) = &patch.exit_keys {
            profile.exit_keys = keys.clone();
        }
        if let Some(args) = &patch.resume_args {
            profile.resume_args = Some(args.clone());
        }
        if let Some(session_ref) = patch.session_ref {
            profile.session_ref = session_ref;
        }
        if let Some(lines) = patch.tail_lines {
            profile.tail_lines = lines;
        }
        if let Some(rounds) = patch.confirm_rounds {
            profile.confirm_rounds = rounds;
        }
        // A profile without a kind of its own runs the kind it is named after.
        if profile.kind.is_empty() {
            profile.kind = name.to_string();
        }
        profile
    }
}

/// The built-in profiles by name (T-31): the kinds GEAR knows how to drive
/// without the user configuring anything.
pub fn builtins() -> BTreeMap<String, Profile> {
    let mut table = BTreeMap::new();
    for kind in ["opencode", "claude", "codex", "grok"] {
        if let Some(profile) = builtin(kind) {
            table.insert(kind.to_string(), profile);
        }
    }
    table
}

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
            quota_bar: None,
            auth_markers: vec![],
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--auto".into()],
            session_ref: false,
            resume_args: Some(vec!["-s".into(), "{session}".into()]),
            exit_keys: vec!["ctrl+c".into()],
            exit_command: None,
            verified: true,
        }),
        // Claude Code as captain (seen on 2026-10-22: Herdr reports its
        // working/idle and its session id). Captain quota comes only from
        // the verified footer bar, never from conversation quota markers.
        "claude" => Some(Profile {
            kind: "claude".into(),
            busy_markers: vec!["esc to interrupt".into()],
            ratelimit_markers: vec![],
            quota_markers: vec![],
            quota_bar: Some(crate::QuotaBar::claude()),
            auth_markers: vec![],
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--dangerously-skip-permissions".into()],
            session_ref: true,
            resume_args: Some(vec!["--resume".into(), "{session}".into()]),
            exit_keys: vec!["ctrl+c".into(), "ctrl+c".into()],
            exit_command: None,
            verified: true,
        }),
        // Codex (verified 2026-10-22, codex-cli 0.163.0 — the SOP
        // `gear --skill agent:codex`): busy `esc to interrupt` (the screen
        // shows `• Working (<time> • esc to interrupt)`); no quota markers
        // yet — the SOP says the five-hour and weekly limits are still to be
        // recorded; unattended `--dangerously-bypass-approvals-and-sandbox`
        // and `-c check_for_update_on_startup=false` — without the second one
        // Codex shows its "Update available" dialog at start and the agent
        // sits blocked, unseen (2026-10-25, codex 0.163.0 → 0.164.0);
        // Herdr reports the session uuid, so session_ref is true; resume is
        // the subcommand `codex resume <uuid>`.
        "codex" => Some(Profile {
            kind: "codex".into(),
            busy_markers: vec!["esc to interrupt".into()],
            ratelimit_markers: vec![],
            quota_markers: vec![],
            quota_bar: None,
            auth_markers: vec![],
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec![
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "-c".into(),
                "check_for_update_on_startup=false".into(),
            ],
            session_ref: true,
            resume_args: Some(vec!["resume".into(), "{session}".into()]),
            // Two: after a turn the first ctrl+c only interrupts ("Conversation
            // interrupted"); the second exits (seen 2026-10-23, L-63).
            exit_keys: vec!["ctrl+c".into(), "ctrl+c".into()],
            exit_command: None,
            verified: true,
        }),
        // Grok (verified 2026-10-23, grok 1.0.62 — the SOP
        // `gear --skill agent:grok`): busy `Ctrl+c:cancel`; unattended
        // `--always-approve`; Herdr reports the session, so session_ref is
        // true; resume is `-r <session>`. It exits on the `/exit` command —
        // ctrl+c and ctrl+d do not exit it — so the profile carries an
        // `exit_command`.
        "grok" => Some(Profile {
            kind: "grok".into(),
            busy_markers: vec!["Ctrl+c:cancel".into()],
            ratelimit_markers: vec![],
            quota_markers: vec![],
            quota_bar: None,
            auth_markers: vec![],
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--always-approve".into()],
            session_ref: true,
            resume_args: Some(vec!["-r".into(), "{session}".into()]),
            exit_keys: vec![],
            exit_command: Some("/exit".into()),
            verified: true,
        }),
        // Only kinds with a verified SOP are built in (design/04).
        _ => None,
    }
}
