//! Per-CLI observation profile (design/04).

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScreenFlag {
    RateLimit,
    Quota,
    Auth,
    /// The provider answered with an error (M4 §12 item 6): the seat waits
    /// for the provider, and the engine reads the quota source to tell a
    /// spent quota from a provider error (§9.2).
    Error,
    /// The provider is in a back-off: the agent waits for it (T-75). The
    /// agent is not stuck, so no stall is raised for it.
    Backoff,
    /// The agent's own guardrails blocked a command or a path (T-113): the
    /// agent goes on, but the captain is told what was refused.
    Blocked,
}

/// A screen a CLI opens that must be answered before its composer takes
/// text (T-457). Codex 0.165 asks `Where should the new conversation run?`
/// after `/new`, and on a resume from another directory it asks which
/// working directory to use. Herdr reports the agent done/idle while the
/// dialog waits, so nothing but the answer moves it on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dialog {
    /// The lines that identify the dialog. They are matched like every other
    /// marker: inside the profile's tail window, one matching line is enough.
    pub markers: Vec<String>,
    /// The keys that answer it, in order. A dialog preselects the choice the
    /// seat needs (the current worktree; the current directory), so Enter
    /// takes it.
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    /// Herdr agent kind: `opencode`, `claude`, `codex`, `pi`…
    pub kind: String,
    /// Kept so that an old config still loads; it decides nothing any more
    /// (§10.2: whether an agent is busy is Herdr's state, never the screen).
    pub busy_markers: Vec<String>,
    pub ratelimit_markers: Vec<String>,
    pub quota_markers: Vec<String>,
    /// Footer-only captain quota detection. Conversation markers never apply.
    pub quota_bar: Option<crate::QuotaBar>,
    pub auth_markers: Vec<String>,
    /// The signs on screen that the provider is in a back-off (T-75): the
    /// agent waits for the provider, so it is not stuck. Empty for the kinds
    /// whose back-off signs are not recorded yet (M4).
    pub backoff_markers: Vec<String>,
    /// The lines that say the agent's own guardrails refused a command or a
    /// path (pi: `@aliou/pi-guardrails`, T-113). The agent goes on after such
    /// a refusal, so nothing else would tell the captain that the lane was
    /// told to do something it may not do. Empty for the kinds without
    /// guardrails.
    pub blocked_markers: Vec<String>,
    /// The signs on screen that the provider answered with an error (§12 item
    /// 6): the agent cannot go on, and the engine reads the quota source to
    /// tell a spent quota from a provider error (§9.2).
    pub error_markers: Vec<String>,
    /// The startup dialogs this CLI opens (T-457). A dialog's screen is read
    /// as `Blocked` — Herdr still reports the agent done/idle — and the
    /// profile's answer keys take it. Empty for the kinds whose dialogs are
    /// not recorded.
    pub dialogs: Vec<Dialog>,
    /// The slash command that starts a new session in this CLI, typed into
    /// the pane (§2, §3.4). Empty = the CLI has none.
    pub new_session: String,
    /// The slash command that compacts the session, typed into the pane
    /// (§2, §3.4). Empty = the CLI has none.
    pub compact: String,
    /// Which reader of §4.3 reads this CLI's records: `pi-jsonl`,
    /// `codex-rollout`, `claude-jsonl`, `opencode-db`. None = unknown.
    pub usage_log: Option<String>,
    /// Only the last `tail_lines` lines are matched.
    pub tail_lines: u32,
    /// A flag counts only after this many consecutive observations.
    pub confirm_rounds: u32,
    /// CLI flags that turn off interactive permission prompts (opencode:
    /// `--auto`). Passed on every start and resume. Required: an agent that
    /// stops on a permission dialog cannot be driven unattended, so a profile
    /// with this empty is never `verified`.
    pub unattended_args: Vec<String>,
    /// The CLI flags that carry the role rules into the agent as a system
    /// prompt (T-195): the rules of the role the seat works in, given at
    /// every start. `{rules_file}` is the rules file's absolute path,
    /// `{rules_toml}` the rules text as one TOML basic string. Empty for the
    /// kinds that take the rules another way (their packets carry them).
    pub instructions_args: Vec<String>,
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
///
/// There is no busy class (§10.2): whether an agent is busy is Herdr's
/// state, never the text on its screen.
pub fn classify_screen(profile: &Profile, tail: &str) -> BTreeSet<ScreenFlag> {
    let lines: Vec<&str> = tail.lines().collect();
    let start = lines.len().saturating_sub(profile.tail_lines as usize);
    let window = &lines[start..];

    let mut flags = BTreeSet::new();
    for line in window {
        if matches_any(&profile.ratelimit_markers, line) {
            flags.insert(ScreenFlag::RateLimit);
        }
        if matches_any(&profile.quota_markers, line) {
            flags.insert(ScreenFlag::Quota);
        }
        if matches_any(&profile.auth_markers, line) {
            flags.insert(ScreenFlag::Auth);
        }
        if matches_any(&profile.error_markers, line) {
            flags.insert(ScreenFlag::Error);
        }
        if matches_any(&profile.backoff_markers, line) {
            flags.insert(ScreenFlag::Backoff);
        }
        if matches_any(&profile.blocked_markers, line) {
            flags.insert(ScreenFlag::Blocked);
        }
    }
    // A startup dialog waits on an answer while Herdr reports the agent
    // done/idle (T-457): it is a blocked screen, not a finished turn.
    if profile
        .dialogs
        .iter()
        .any(|dialog| window.iter().any(|line| matches_any(&dialog.markers, line)))
    {
        flags.insert(ScreenFlag::Blocked);
    }
    flags
}

/// The start args with `{repo_root}` replaced (T-76): the folder the agent
/// runs in — the worktree the engine made for one of the project's repos. A
/// profile without the placeholder keeps its args as they are. `{session}` is
/// filled by the callers that resume a session.
///
/// The folder is written **as the filesystem resolves it**: Codex canonicalises
/// its working directory before it looks a folder up, so a symlinked path
/// (`/var` in a temp dir, a linked checkout) would not match (checked live,
/// 0.164.0). A path that does not exist is written as it came.
pub fn render_args(args: &[String], repo_root: &str) -> Vec<String> {
    let folder = std::fs::canonicalize(repo_root)
        .ok()
        .filter(|_| args.iter().any(|arg| arg.contains("{repo_root}")))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo_root.to_string());
    args.iter()
        .map(|arg| arg.replace("{repo_root}", &folder))
        .collect()
}

/// Reject a prompt template that contains any marker of `profile`
/// (old keeper incident: the nudge text tripped its own rate-limit detector).
/// Returns the offending marker in the error.
///
/// The busy markers are not among them any more: they decide nothing
/// (§10.2), so a template that holds one is harmless.
pub fn validate_template(profile: &Profile, template: &str) -> Result<(), String> {
    let lists = [
        ("ratelimit", &profile.ratelimit_markers),
        ("quota", &profile.quota_markers),
        ("auth", &profile.auth_markers),
        ("error", &profile.error_markers),
    ];
    for (kind, markers) in lists {
        for marker in markers {
            if marker.is_empty() {
                continue;
            }
            if template.contains(marker.as_str()) {
                return Err(format!(
                    "template must not contain the {kind} marker: {marker}"
                ));
            }
        }
    }
    Ok(())
}

/// Literal substring match of `line` against a marker list.
fn matches_any(markers: &[String], line: &str) -> bool {
    markers
        .iter()
        .any(|m| !m.is_empty() && line.contains(m.as_str()))
}

/// Built-in profile for an agent kind, from the verified facts in
/// `gear --skill agent:<kind>` (design/04, /16). Only kinds with a verified SOP
/// are built in: today `opencode`, `claude`, `codex`, `grok` and `pi` —
/// opencode: quota `Session failed`, `Token Plan`, `(2056)`,
/// `Free usage limit reached`; no rate-limit/auth markers yet; tail 16 lines,
/// confirm 2 rounds; unattended `--auto`; session_ref false; resume
/// `["-s", "{session}"]`; exit `["ctrl+c"]`; new session `/new`; compact
/// `/compact`; usage log `opencode-db`; verified.
/// codex: quota `Usage limit reached` and the
/// other spent-limit texts of codex 0.164.0 (T-115) — the "only N% left"
/// warning is not one of them; no rate-limit/auth markers yet; tail 16
/// lines, confirm 2
/// rounds; unattended `--dangerously-bypass-approvals-and-sandbox` and
/// `-c check_for_update_on_startup=false`; the startup dialogs of codex
/// 0.165 (T-457) — `/new` asks `Where should the new conversation run?`
/// (answer Enter: the current worktree) and a resume from another directory
/// asks which working directory to use (answer Enter: the current one, and
/// `-c tui.resume_cwd="current"` keeps that dialog from showing); those
/// screens read as Blocked although Herdr reports done/idle;
/// session_ref true (Herdr reports the session uuid); resume
/// `["resume", "{session}"]`; exit by the typed `/quit` command
/// (`exit_command`, no keys — §12 item 4); new session `/new`; compact
/// `/compact`; usage log `codex-rollout`; verified.
/// grok: unattended `--always-approve`; session_ref
/// true; resume `["-r", "{session}"]`; exit by the `/exit` command
/// (`exit_command`, no keys). The session fields are empty: grok is not one
/// of the kinds of §2 and its live checks were never run (M4).
/// pi: quota `insufficient_quota`;
/// rate limit `rate_limit_error`; auth `No API key found for`; back-off
/// `Retrying (`; provider error `overloaded_error` and `Retry failed after`
/// (§12 item 6); guardrails `Command auto-denied:` and
/// `is blocked (outside working directory)`; unattended `--approve`;
/// session_ref true (Herdr reports the session file); resume
/// `["--session", "{session}"]`; exit by the `/quit` command
/// (`exit_command`, no keys); new session `/new`; compact `/compact`;
/// usage log `pi-jsonl`.
/// omp (oh-my-pi, a pi fork): quota `insufficient_quota`,
/// `exceeded your current quota` and `usage_limit_reached`; rate limit
/// `rate_limit_error`, `rate_limit_reached`, `too_many_requests` and
/// `request_throttled`; auth `authentication_error` and
/// `No API key found for`; back-off `Retrying (`; provider error
/// `overloaded_error` and `Retry failed after`; unattended
/// `--auto-approve`; session_ref false (Herdr 0.9.1 reports no
/// `agent_session` for omp); resume `["--resume", "{session}"]`; exit by
/// the `/quit` command; new session `/new`; compact `/compact`; usage log
/// `pi-jsonl` (omp writes pi's session files).
///
/// The busy markers of the older profiles are still there, so that an old
/// config loads; they decide nothing (§10.2).
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

    /// Is the provider in a back-off? (T-75)
    ///
    /// The screen shows one of the profile's back-off markers: the agent
    /// waits for the provider, and that is not a stall. Only the tail window
    /// counts, like every other marker: a sign that scrolled out of it is not
    /// a current fact.
    pub fn backoff(&self, screen: &str) -> bool {
        classify_screen(self, screen).contains(&ScreenFlag::Backoff)
    }

    /// Did the agent's own guardrails refuse something? (T-113)
    ///
    /// The refusal is a line in the transcript (`Command auto-denied: …`,
    /// `Access to <path> is blocked (outside working directory).`). The
    /// agent goes on, so the screen is the only place it shows. Only the
    /// tail window counts, like every other marker.
    pub fn blocked(&self, screen: &str) -> bool {
        classify_screen(self, screen).contains(&ScreenFlag::Blocked)
    }

    /// The startup dialog that is open on `screen`, if any (T-457).
    ///
    /// Only the tail window counts, like every other marker. The answer keys
    /// of the returned dialog are the ones to send; see
    /// [`answer_dialog`](crate::answer_dialog).
    pub fn dialog(&self, screen: &str) -> Option<&Dialog> {
        let lines: Vec<&str> = screen.lines().collect();
        let start = lines.len().saturating_sub(self.tail_lines as usize);
        let window = &lines[start..];
        self.dialogs
            .iter()
            .find(|dialog| window.iter().any(|line| matches_any(&dialog.markers, line)))
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
    /// The CLI flags that carry the role rules into the agent as a system
    /// prompt (T-195).
    pub instructions_args: Option<Vec<String>>,
    pub busy_markers: Option<Vec<String>>,
    pub quota_markers: Option<Vec<String>>,
    pub quota_bar: Option<crate::QuotaBar>,
    pub ratelimit_markers: Option<Vec<String>>,
    pub auth_markers: Option<Vec<String>>,
    /// The signs on screen that the provider is in a back-off (T-75).
    pub backoff_markers: Option<Vec<String>>,
    /// The lines that say the agent's own guardrails refused something
    /// (T-113).
    pub blocked_markers: Option<Vec<String>>,
    /// The signs on screen that the provider answered with an error (§12
    /// item 6).
    pub error_markers: Option<Vec<String>>,
    /// The slash command that starts a new session (§2).
    pub new_session: Option<String>,
    /// The slash command that compacts the session (§2); empty = none.
    pub compact: Option<String>,
    /// Which reader of §4.3 reads this CLI's records (§2): `pi-jsonl`,
    /// `codex-rollout`, `claude-jsonl`, `opencode-db`.
    pub usage_log: Option<String>,
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
        if let Some(args) = &patch.instructions_args {
            profile.instructions_args = args.clone();
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
        if let Some(markers) = &patch.backoff_markers {
            profile.backoff_markers = markers.clone();
        }
        if let Some(markers) = &patch.blocked_markers {
            profile.blocked_markers = markers.clone();
        }
        if let Some(markers) = &patch.error_markers {
            profile.error_markers = markers.clone();
        }
        if let Some(command) = &patch.new_session {
            profile.new_session = command.clone();
        }
        if let Some(command) = &patch.compact {
            profile.compact = command.clone();
        }
        if let Some(log) = &patch.usage_log {
            profile.usage_log = Some(log.clone());
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
    for kind in ["opencode", "claude", "codex", "grok", "pi", "omp", "agy"] {
        if let Some(profile) = builtin(kind) {
            table.insert(kind.to_string(), profile);
        }
    }
    table
}

/// The args that carry the role rules into a start (T-195).
///
/// `{rules_file}` is the rules file's absolute path; `{rules_toml}` is the
/// rules text as one TOML basic string — the form a `codex -c` value takes, so
/// a text with a quote or a newline survives the round trip.
pub fn render_rules(args: &[String], rules_file: &str, rules_text: &str) -> Vec<String> {
    let rules_toml = toml_one_line(rules_text);
    args.iter()
        .map(|arg| {
            arg.replace("{rules_file}", rules_file)
                .replace("{rules_toml}", &rules_toml)
        })
        .collect()
}

/// The rules as a TOML basic string on one line. Herdr refuses an agent
/// argument with a newline ("cannot be encoded safely for the target shell"),
/// and `toml::Value::String` writes multi-line text as a multi-line string.
pub fn toml_one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The Codex rules argument: a short pointer to the rules file.
pub const CODEX_RULES_ARG: &str = "developer_instructions=\"Your GEAR role rules are in the file {rules_file}. Read that file before anything else and follow it for the whole session.\"";

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
            backoff_markers: vec![],
            blocked_markers: vec![],
            error_markers: vec![],
            dialogs: vec![],
            new_session: "/new".into(),
            compact: "/compact".into(),
            usage_log: Some("opencode-db".into()),
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--auto".into()],
            // T-195: opencode takes the rules in its packet, not as a system
            // prompt.
            instructions_args: vec![],
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
            backoff_markers: vec![],
            blocked_markers: vec![],
            error_markers: vec![],
            dialogs: vec![],
            new_session: "/clear".into(),
            compact: "/compact".into(),
            usage_log: Some("claude-jsonl".into()),
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--dangerously-skip-permissions".into()],
            // T-195: claude reads the rules from a file.
            instructions_args: vec!["--append-system-prompt-file".into(), "{rules_file}".into()],
            session_ref: true,
            resume_args: Some(vec!["--resume".into(), "{session}".into()]),
            exit_keys: vec!["ctrl+c".into(), "ctrl+c".into()],
            exit_command: None,
            verified: true,
        }),
        // Codex (verified 2026-10-22, codex-cli 0.163.0 — the SOP
        // `gear --skill agent:codex`): busy `esc to interrupt` (the screen
        // shows `• Working (<time> • esc to interrupt)`); unattended
        // `--dangerously-bypass-approvals-and-sandbox`
        // and `-c check_for_update_on_startup=false` — without the second one
        // Codex shows its "Update available" dialog at start and the agent
        // sits blocked, unseen (2026-10-25, codex 0.163.0 → 0.164.0);
        // Herdr reports the session uuid, so session_ref is true; resume is
        // the subcommand `codex resume <uuid>`.
        //
        // T-75: the provider back-off. While Codex retries a request (a 429
        // from the provider, a dropped stream) its own source
        // (`codex-rs/core/src/responses_retry.rs`, codex-cli 0.164.0) notifies
        // the UI with `Reconnecting... {retry}/{max_retries}` (and
        // `Reconnecting... waiting for network` for the unbounded connection
        // retries), and the TUI shows it as the status header
        // (`codex-rs/tui/src/chatwidget/streaming.rs`, `on_stream_error`).
        // The agent waits for the provider for minutes without any output:
        // that is a back-off, not a stall. The other kinds keep an empty list
        // — their back-off signs are not recorded yet (M4).
        //
        // T-115: the spent limit. Codex says it in its own words (the
        // installed binary, codex-cli 0.164.0): the modal titled
        // `Usage limit reached` over
        // `Your included usage is exhausted. Choose an option below to
        // continue.`, the error line `Usage limit reached. You've reached
        // your usage limit. Increase your limits to continue using codex.`,
        // the workspace forms `You've reached your workspace credit limit` /
        // `Your workspace is out of credits` / `You hit your spend cap`, and
        // the sentence the TUI prints when the plan is spent: `You’ve hit
        // your usage limit.` — followed by an upgrade line (Plus, Pro), a
        // credit link, a request to the workspace admin, or nothing. All of
        // them are stops: the agent cannot work until the limit resets.
        //
        // The spent-limit sentence has a **curly** apostrophe in the binary
        // (`You’ve`), so the marker starts after it (`hit your usage limit`):
        // one marker for every form, straight or curly (T-115 review F1).
        //
        // The **warning** is not a stop and is not a marker: `Heads up, you
        // have less than 5% of your weekly limit left. Run /status for a
        // breakdown.` (read live, 2026-10-26) and the status line
        // `⚠ weekly limit: only 3% left · resets at 3:30 AM on 1 Nov ·
        // /status` say the limit is nearly spent; the agent still works.
        "codex" => Some(Profile {
            kind: "codex".into(),
            busy_markers: vec!["esc to interrupt".into()],
            ratelimit_markers: vec![],
            quota_markers: vec![
                "Usage limit reached".into(),
                "You've reached your usage limit".into(),
                "hit your usage limit".into(),
                "Your included usage is exhausted".into(),
                "You've reached your workspace credit limit".into(),
                "Your workspace is out of credits".into(),
                "You hit your spend cap".into(),
            ],
            quota_bar: None,
            auth_markers: vec![],
            backoff_markers: vec!["Reconnecting...".into()],
            blocked_markers: vec![],
            error_markers: vec![],
            // T-457: Codex 0.165's startup screens (the screens captured in
            // M5, gear/voyages/M6/backlog.md). `/new` asks
            // `Where should the new conversation run?` with
            // `1. Use current Git worktree` (keep using the current working
            // directory) and `2. Create new Git worktree`; the first choice
            // is preselected, so Enter keeps the seat in its own worktree.
            // A resume from another directory asks which working directory
            // to use (`Current = your current working directory` /
            // `Use session directory (<path>)`); the start args keep that
            // dialog from showing at all (`tui.resume_cwd="current"`), and
            // Enter takes the preselected current directory if it does.
            // Herdr reports the agent done/idle on both screens, so
            // classify_screen reads them as Blocked.
            dialogs: vec![
                Dialog {
                    markers: vec![
                        "Where should the new conversation run?".into(),
                        "Where should the forked conversation run?".into(),
                    ],
                    keys: vec!["enter".into()],
                },
                Dialog {
                    markers: vec![
                        "Choose which working directory to use".into(),
                        "Current = your current working directory".into(),
                    ],
                    keys: vec!["enter".into()],
                },
            ],
            new_session: "/new".into(),
            compact: "/compact".into(),
            usage_log: Some("codex-rollout".into()),
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec![
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "-c".into(),
                "check_for_update_on_startup=false".into(),
                // T-76: the folder the agent runs in is trusted, so Codex's
                // first-run "Trust this folder?" dialog never blocks a seat.
                // `{repo_root}` is the worktree the engine made for one of the
                // project's repos (trial G08). The inline table is the form
                // Codex's `-c` honours: the dotted `projects."<path>".trust_level`
                // spelling is silently ignored there (checked live, 0.164.0).
                "-c".into(),
                "projects={\"{repo_root}\"={trust_level=\"trusted\"}}".into(),
                // T-457: a resume from another directory asks which working
                // directory to use; `current` makes Codex use the directory
                // it was started in — the seat's own worktree — and never
                // show the dialog (codex-cli 0.165.0, `tui.resume_cwd`).
                "-c".into(),
                "tui.resume_cwd=\"current\"".into(),
            ],
            // T-195: codex takes the rules as a developer instruction, in one
            // TOML value.
            // A pointer, not the text: Herdr types the command into the shell and
            // the terminal cuts a typed line near 1024 bytes (M4 05:50: every
            // reviewer start broke off inside the rules).
            instructions_args: vec!["-c".into(), CODEX_RULES_ARG.into()],
            session_ref: true,
            resume_args: Some(vec!["resume".into(), "{session}".into()]),
            // M4 §12 item 4: the typed command `/quit` exits Codex, and
            // ctrl+c twice is not safe (the second one reached the shell
            // after Codex had gone and broke the next typed start). The
            // exit is a command now, not keys.
            exit_keys: vec![],
            exit_command: Some("/quit".into()),
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
            backoff_markers: vec![],
            blocked_markers: vec![],
            error_markers: vec![],
            dialogs: vec![],
            new_session: String::new(),
            compact: String::new(),
            usage_log: None,
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--always-approve".into()],
            // T-195: grok takes the rules in its packet.
            instructions_args: vec![],
            session_ref: true,
            resume_args: Some(vec!["-r".into(), "{session}".into()]),
            exit_keys: vec![],
            exit_command: Some("/exit".into()),
            verified: true,
        }),
        // Pi (verified 2026-10-26, pi 1.4.0 — the SOP
        // `gear --skill agent:pi`): busy ` Working` (the spinner line the TUI
        // draws while a turn runs: `── ⠋ Working ────`; the spinner glyph
        // changes, the word does not); unattended `--approve` (pi's own
        // project-trust override — without it a lane in a fresh worktree
        // waits on a trust prompt); Herdr reports the session **file**, so
        // session_ref is true and the resume is `--session <file>` (pi's own
        // hint after an exit names the session id, but `--session` takes the
        // path too — checked live, 1.4.0). It exits on the `/quit` command
        // (`exit_command`, no keys): the shortcut line says
        // `escape interrupt · ctrl+c/ctrl+d clear/exit`, and ctrl+d only
        // exits while the editor is empty.
        //
        // The provider signs, taken from pi's own output (1.4.0):
        //   quota     `429: {"message":"You exceeded your current quota, …",
        //                     "type":"insufficient_quota", …}`
        //   rate limit `429: {"message":"Rate limit reached for …",
        //                     "type":"rate_limit_error", …}`
        //   auth      `Error: No API key found for anthropic.`
        //   back-off  `── ⠙ Retrying (3/3) in 7s... (escape to cancel) ────`
        //
        // The guardrails of `@aliou/pi-guardrails` refuse a command or a path
        // and the agent goes on: `Command auto-denied: <why>` (permission-gate)
        // and `Access to <path> is blocked (outside working directory).`
        // (path-access). The captain is told (T-113).
        "pi" => Some(Profile {
            kind: "pi".into(),
            busy_markers: vec![" Working".into()],
            ratelimit_markers: vec![
                "rate_limit_error".into(),
                "Rate limit reached".into(),
                "429:".into(),
            ],
            quota_markers: vec![
                "insufficient_quota".into(),
                "exceeded your current quota".into(),
            ],
            quota_bar: None,
            auth_markers: vec!["No API key found for".into()],
            backoff_markers: vec!["Retrying (".into()],
            blocked_markers: vec![
                "Command auto-denied:".into(),
                "is blocked (outside working directory)".into(),
            ],
            // M4 §12 item 6: the provider answered with an overload error
            // (`529 … overloaded_error`) and pi gave up after three tries
            // (`Error: Retry failed after 3 attempts: 529`). That is a
            // provider error, not a spent quota: §9.2 reads the quota source
            // to tell the two apart.
            error_markers: vec!["overloaded_error".into(), "Retry failed after".into()],
            dialogs: vec![],
            new_session: "/new".into(),
            compact: "/compact".into(),
            usage_log: Some("pi-jsonl".into()),
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--approve".into()],
            // T-195: pi reads the rules from a file.
            instructions_args: vec!["--append-system-prompt".into(), "{rules_file}".into()],
            session_ref: true,
            resume_args: Some(vec!["--session".into(), "{session}".into()]),
            exit_keys: vec![],
            exit_command: Some("/quit".into()),
            verified: true,
        }),
        // omp (oh-my-pi 18.12.6, a pi fork; checked 2026-10-31 against
        // `omp --help`, omp's own config and a live start through Herdr
        // 0.9.1 — the SOP is `gear --skill agent:pi` until omp has its own):
        // busy ` Working…` (the spinner line the TUI draws while a turn
        // runs: `⎋ Working…`); unattended `--auto-approve` (omp's own flag;
        // `--approval-mode yolo` is its other spelling and is not passed);
        // session_ref false (Herdr 0.9.1 reports no `agent_session` for an
        // omp pane — checked live with `herdr agent list`); resume
        // `--resume <session>` (omp's own hint after ctrl+d; omp has no
        // `--session` flag); exit on the `/quit` command (`exit_command`, no
        // keys — live: `/quit` from the command palette ends omp; ctrl+d
        // exits only while the composer is empty).
        //
        // The provider signs are omp's own (18.12.6: the live 401 screen
        // `✘ 401 {"type":"error","error":{"type":"authentication_error",…}}`
        // and the binary's string table): auth `authentication_error`,
        // `No API key found for` and `No credential available for provider`;
        // quota `insufficient_quota`, `exceeded your current quota` and
        // `usage_limit_reached`; rate limit `rate_limit_error`,
        // `rate_limit_reached`, `too_many_requests` and
        // `request_throttled`; back-off `Retrying (`; provider error
        // `overloaded_error` and `Retry failed after`. omp has no
        // pi-guardrails markers; `--auto-approve` keeps approval dialogs
        // away, so `blocked_markers` and `dialogs` stay empty.
        "omp" => Some(Profile {
            kind: "omp".into(),
            busy_markers: vec![" Working…".into()],
            ratelimit_markers: vec![
                "rate_limit_error".into(),
                "rate_limit_reached".into(),
                "too_many_requests".into(),
                "request_throttled".into(),
            ],
            quota_markers: vec![
                "insufficient_quota".into(),
                "exceeded your current quota".into(),
                "usage_limit_reached".into(),
            ],
            quota_bar: None,
            auth_markers: vec![
                "authentication_error".into(),
                "No API key found for".into(),
                "No credential available for provider".into(),
            ],
            backoff_markers: vec!["Retrying (".into()],
            blocked_markers: vec![],
            error_markers: vec!["overloaded_error".into(), "Retry failed after".into()],
            dialogs: vec![],
            new_session: "/new".into(),
            compact: "/compact".into(),
            // T-496: omp writes pi's session files under
            // `~/.omp/agent/sessions/<path>/*.jsonl`; a real 18.12.6 session
            // carries the same `message`/`usage` records, so the pi-jsonl
            // reader reads it (fixture: crates/gear-usage/tests/it/fixtures).
            usage_log: Some("pi-jsonl".into()),
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--auto-approve".into()],
            // T-195: omp takes the rules as text or an existing file, like pi.
            instructions_args: vec!["--append-system-prompt".into(), "{rules_file}".into()],
            session_ref: false,
            resume_args: Some(vec!["--resume".into(), "{session}".into()]),
            exit_keys: vec![],
            exit_command: Some("/quit".into()),
            verified: true,
        }),
        // agy (Antigravity CLI, a Google agent; checked 2026-10-31 against
        // `agy --help` and a live start in a Herdr pane, agy 1.4.12 — the
        // packet names 1.2.13, this machine has 1.4.12): unattended
        // `--dangerously-skip-permissions` (agy's own flag; it does not
        // answer the project-trust dialog, which the profile's dialog
        // markers do); model `--model <id>` and reasoning `--effort
        // low|medium|high|xhigh|max` (the effort is the chain entry's args,
        // like pi's thinking level); resume `--conversation <id>`. Herdr
        // reports the conversation id as `agent_session` after the first
        // turn (live: kind `id`, source `herdr:antigravity_cli`), so
        // session_ref is true. It exits on ctrl+c twice (live: the first
        // shows `press ctrl+c again to exit`, the second returns to the
        // shell); there is no `/quit` command. Its screens: busy
        // `Generating...` with `esc to cancel`; the startup project-trust
        // dialog (`Do you trust the contents of this project?`, Enter takes
        // the preselected `Yes, I trust this folder`); the usage panel's
        // `Quota exhausted`; a retried request shows `<what>, retrying in
        // <n>s`. agy has no system-prompt flag, so it takes the role rules
        // the way grok and opencode do — in its packet (instructions_args
        // empty). It has no `/new` or `/compact` slash command, so both are
        // empty.
        "agy" => Some(Profile {
            kind: "agy".into(),
            busy_markers: vec!["Generating...".into()],
            ratelimit_markers: vec![],
            quota_markers: vec!["Quota exhausted".into()],
            quota_bar: None,
            auth_markers: vec![],
            backoff_markers: vec![", retrying in ".into()],
            blocked_markers: vec![],
            error_markers: vec![],
            dialogs: vec![Dialog {
                markers: vec!["Do you trust the contents of this project?".into()],
                keys: vec!["enter".into()],
            }],
            new_session: String::new(),
            compact: String::new(),
            // T-498: a conversation's transcript under
            // `~/.gemini/antigravity-cli/brain/<conversation>/.system_generated/
            // logs/transcript.jsonl` carries its token counts.
            usage_log: Some("agy-jsonl".into()),
            tail_lines: 16,
            confirm_rounds: 2,
            unattended_args: vec!["--dangerously-skip-permissions".into()],
            instructions_args: vec![],
            session_ref: true,
            resume_args: Some(vec!["--conversation".into(), "{session}".into()]),
            exit_keys: vec!["ctrl+c".into(), "ctrl+c".into()],
            exit_command: None,
            verified: true,
        }),
        // Only kinds with a verified SOP are built in (design/04).
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Herdr refuses an agent argument with a newline: the rules go in as a
    /// one-line TOML string that reads back to the same text.
    #[test]
    fn rules_render_on_one_line_and_parse_back() {
        let text =
            "Rule one: \"quote\" and \\ backslash.\n\tTab, `code` and $HOME.\r\nVoyage done: x\n";
        let args = render_rules(
            &["-c".into(), "developer_instructions={rules_toml}".into()],
            "/r.md",
            text,
        );
        assert!(
            !args[1].contains('\n') && !args[1].contains('\r'),
            "{:?}",
            args[1]
        );
        let value: toml::Table = toml::from_str(&args[1]).unwrap();
        assert_eq!(value["developer_instructions"].as_str().unwrap(), text);
    }

    /// The status line Codex shows while it retries a request, from its own
    /// source (`codex-rs/core/src/responses_retry.rs`, codex-cli 0.164.0):
    /// the retry notification is `Reconnecting... {retry}/{max_retries}` and
    /// the TUI renders it as the status header, above the composer.
    const CODEX_RETRYING: &str =
        "• Working (12s • esc to interrupt)\n• Reconnecting... 2/5 (14s • esc to interrupt)";

    /// T-195: the rules of the role go into the agent as a system prompt
    /// where the CLI allows it; the kinds that take them another way carry
    /// none.
    #[test]
    fn the_built_in_profiles_carry_the_rules_where_the_cli_allows_it() {
        assert_eq!(
            builtin("pi").unwrap().instructions_args,
            vec![
                "--append-system-prompt".to_string(),
                "{rules_file}".to_string()
            ]
        );
        assert_eq!(
            builtin("claude").unwrap().instructions_args,
            vec![
                "--append-system-prompt-file".to_string(),
                "{rules_file}".to_string()
            ]
        );
        assert_eq!(
            builtin("codex").unwrap().instructions_args,
            vec!["-c".to_string(), CODEX_RULES_ARG.to_string()]
        );
        assert!(builtin("opencode").unwrap().instructions_args.is_empty());
        assert!(builtin("grok").unwrap().instructions_args.is_empty());
    }

    /// T-495: omp starts unattended with its own flag and takes the role
    /// rules as a system prompt file; it exits by the `/quit` command and
    /// its back-off and quota signs are the ones its own screens show.
    #[test]
    fn the_omp_profile_starts_unattended_with_the_rules() {
        let omp = builtin("omp").unwrap();
        assert_eq!(omp.kind, "omp");
        assert_eq!(omp.unattended_args, vec!["--auto-approve".to_string()]);
        assert_eq!(
            omp.instructions_args,
            vec![
                "--append-system-prompt".to_string(),
                "{rules_file}".to_string()
            ]
        );
        assert_eq!(omp.exit_command.as_deref(), Some("/quit"));
        assert!(omp.exit_keys.is_empty());
        // omp has no `--session`: the resume is omp's own `--resume`.
        assert_eq!(
            omp.resume_args,
            Some(vec!["--resume".to_string(), "{session}".to_string()])
        );
        assert!(!omp.session_ref, "Herdr reports no agent_session for omp");
        assert!(omp.verified);
        // The signs omp's own screens carry (18.12.6).
        assert!(omp.backoff("⎋ Retrying (2/3) in 5s…"));
        let quota = "✘ 429 {\"type\":\"error\",\"error\":{\"type\":\"insufficient_quota\"}}";
        assert!(classify_screen(&omp, quota).contains(&ScreenFlag::Quota));
        let limited = "✘ 429 {\"error\":{\"type\":\"rate_limit_error\"}}";
        assert!(classify_screen(&omp, limited).contains(&ScreenFlag::RateLimit));
        let auth = "✘ 401 {\"error\":{\"type\":\"authentication_error\"}}";
        assert!(classify_screen(&omp, auth).contains(&ScreenFlag::Auth));
        assert!(classify_screen(
            &omp,
            "Error: Retry failed after 3 attempts: 529 overloaded_error"
        )
        .contains(&ScreenFlag::Error));
        assert!(builtins().contains_key("omp"));
    }

    /// T-497: agy starts unattended with its own flag, takes the model as
    /// `--model` and the effort from the chain entry, and resumes with
    /// `--conversation`; Herdr reports its conversation id after a turn. A
    /// fresh worktree opens agy's project-trust dialog, which the profile
    /// answers with Enter (the preselected "Yes, I trust this folder"), and
    /// agy exits on ctrl+c twice. It has no system-prompt flag, so the rules
    /// reach it in its packet like grok's and opencode's.
    #[test]
    fn the_agy_profile_starts_unattended_and_resumes_with_its_conversation() {
        let agy = builtin("agy").unwrap();
        assert_eq!(agy.kind, "agy");
        assert_eq!(
            agy.unattended_args,
            vec!["--dangerously-skip-permissions".to_string()]
        );
        assert!(
            agy.instructions_args.is_empty(),
            "agy has no system-prompt flag: the rules go in its packet"
        );
        assert!(agy.session_ref, "Herdr reports agy's conversation id");
        assert_eq!(
            agy.resume_args,
            Some(vec!["--conversation".to_string(), "{session}".to_string()])
        );
        assert_eq!(
            agy.exit_keys,
            vec!["ctrl+c".to_string(), "ctrl+c".to_string()]
        );
        assert!(agy.exit_command.is_none());
        assert_eq!(agy.usage_log.as_deref(), Some("agy-jsonl"));
        assert!(agy.verified);

        // The project-trust dialog of a fresh worktree, as agy draws it.
        let trust =
            "Do you trust the contents of this project?\n\n> Yes, I trust this folder\n  No, exit";
        let dialog = agy.dialog(trust).expect("the trust dialog");
        assert_eq!(dialog.keys, vec!["enter".to_string()]);

        // The signs agy's own screens carry (1.4.12): a retried request and
        // the usage panel's spent quota.
        assert!(agy.backoff("Rate limited, retrying in 5s"));
        assert!(classify_screen(&agy, "Quota exhausted").contains(&ScreenFlag::Quota));
    }
    /// T-195: the rules file's path goes into the args.
    #[test]
    fn the_rules_file_is_rendered_into_the_args() {
        let pi = builtin("pi").unwrap();
        let args = render_rules(
            &pi.instructions_args,
            "/tmp/gear/rules.md",
            "the rules of the lane",
        );
        assert_eq!(
            args,
            vec![
                "--append-system-prompt".to_string(),
                "/tmp/gear/rules.md".to_string()
            ]
        );
    }

    /// T-195: the codex value is one TOML string: a text with a quote and a
    /// newline comes back as the same text.
    #[test]
    fn the_codex_rules_arg_is_a_short_toml_pointer_to_the_file() {
        let codex = builtin("codex").unwrap();
        let text = "the lane reports:\n  \"progress\" and \"blocked\"\n".repeat(200);
        let args = render_rules(&codex.instructions_args, "/tmp/gear/rules/review.md", &text);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        // One TOML pair naming the file, short enough to be typed into a
        // shell whatever the rules' length (a typed line is cut near 1024 bytes).
        let parsed: toml::Table = toml::from_str(&args[1]).expect("the argument is TOML");
        let value = parsed
            .get("developer_instructions")
            .and_then(toml::Value::as_str)
            .unwrap();
        assert!(value.contains("/tmp/gear/rules/review.md"), "{value}");
        assert!(
            args[1].len() < 300 && !args[1].contains('\n'),
            "{}",
            args[1]
        );
    }

    /// T-195: a project's config may set the args.
    #[test]
    fn a_config_patch_overrides_the_rules_args() {
        let pi = builtin("pi").unwrap();
        let patch = ProfilePatch {
            instructions_args: Some(vec![
                "--system-prompt".to_string(),
                "{rules_file}".to_string(),
            ]),
            ..ProfilePatch::default()
        };
        let patched = pi.patched(&patch, "pi");
        assert_eq!(
            patched.instructions_args,
            vec!["--system-prompt".to_string(), "{rules_file}".to_string()]
        );
        // A patch without the field keeps the profile's own.
        let untouched = pi.patched(&ProfilePatch::default(), "pi");
        assert_eq!(untouched.instructions_args, pi.instructions_args);
    }

    #[test]
    fn the_codex_back_off_is_recognised() {
        let codex = builtin("codex").unwrap();
        assert!(codex.backoff(CODEX_RETRYING));
        // The unbounded connection retry says the same thing.
        assert!(codex.backoff("• Reconnecting... waiting for network (3m • esc to interrupt)"));
        // A 429 retried for minutes shows the same header, with the error in
        // the status details.
        assert!(codex.backoff(
            "• Reconnecting... 3/5 (1m 12s • esc to interrupt)\n  └ Rate limit exceeded: Rate limit reached for gpt-5.1"
        ));
    }

    #[test]
    fn ordinary_output_is_not_a_back_off() {
        let codex = builtin("codex").unwrap();
        assert!(!codex.backoff("• Working (12s • esc to interrupt)\n• Ran cargo test"));
        assert!(!codex.backoff("Reconnected. All set.\n• Working (1s • esc to interrupt)"));
        // The bare word is not the sign: the retry status is its own line.
        assert!(!codex.backoff("we are reconnecting the socket in a test"));
        assert!(!codex.backoff(""));
    }

    #[test]
    fn only_the_codex_kind_has_back_off_signs_yet() {
        for kind in ["opencode", "claude", "grok"] {
            let profile = builtin(kind).unwrap();
            assert!(profile.backoff_markers.is_empty(), "{kind}");
            assert!(!profile.backoff(CODEX_RETRYING), "{kind}");
        }
        assert_eq!(
            builtin("codex").unwrap().backoff_markers,
            vec!["Reconnecting...".to_string()]
        );
    }

    #[test]
    fn a_back_off_outside_the_tail_window_is_not_current() {
        let mut codex = builtin("codex").unwrap();
        codex.tail_lines = 2;
        // The sign scrolled out of the window: the seat is not in a back-off
        // any more (a later screen is the fact).
        assert!(!codex.backoff("Reconnecting... 2/5\nok\nok"));
        assert!(codex.backoff("ok\nReconnecting... 2/5"));
    }

    #[test]
    fn a_project_may_set_its_own_back_off_markers() {
        let patch = ProfilePatch {
            backoff_markers: Some(vec!["waiting for provider".into()]),
            ..ProfilePatch::default()
        };
        let profile = builtin("opencode").unwrap().patched(&patch, "opencode");
        assert!(profile.backoff("waiting for provider: 30 s"));
        // The built-in is untouched.
        assert!(!builtin("opencode")
            .unwrap()
            .backoff("waiting for provider: 30 s"));
    }

    /// T-76: the codex start args carry the folder the agent runs in, so its
    /// first-run "Trust this folder?" dialog never blocks a seat.
    #[test]
    fn the_codex_start_args_name_the_folder_it_runs_in() {
        let codex = builtin("codex").unwrap();
        let args = render_args(&codex.unattended_args, "/worktrees/gear/home-heat");
        assert_eq!(
            args,
            vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-c".to_string(),
                "check_for_update_on_startup=false".to_string(),
                "-c".to_string(),
                "projects={\"/worktrees/gear/home-heat\"={trust_level=\"trusted\"}}".to_string(),
                // T-457: the resume keeps to the seat's own directory.
                "-c".to_string(),
                "tui.resume_cwd=\"current\"".to_string(),
            ]
        );
        // No placeholder survives, and no kind without it changes.
        assert!(!args.iter().any(|arg| arg.contains("{repo_root}")));
        let claude = builtin("claude").unwrap();
        assert_eq!(
            render_args(&claude.unattended_args, "/x"),
            claude.unattended_args
        );
    }

    #[test]
    fn the_flag_and_the_method_agree() {
        let codex = builtin("codex").unwrap();
        assert!(classify_screen(&codex, CODEX_RETRYING).contains(&ScreenFlag::Backoff));
        assert!(!classify_screen(&codex, "• Ran cargo test").contains(&ScreenFlag::Backoff));
    }
}
