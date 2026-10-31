//! `gear admin kickoff` (PRD §8a): what this machine offers, for the captain
//! to turn into a proposal the user corrects in one pass.
//!
//! Read-only and silent: it writes nothing, starts nothing and touches no
//! Herdr state. It only reads PATH, `opencode models` and
//! `~/.codex/models_cache.json` — the inventory a kickoff proposes from
//! (design/01: "no silent discovery", the user confirms every choice).

use std::path::{Path, PathBuf};
use std::process::Command;

use gear_model::{ErrorCode, GearError, GearResult};

/// How deep below the project's root a nested repository is looked for (T-54).
const MAX_REPO_DEPTH: usize = 3;

/// The directories that are never scanned for a nested repository (T-54):
/// build output and dependencies hold none of the user's repos.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules", ".git"];

/// The kinds `herdr agent start --kind` accepts (herdr 0.9.1), each with the
/// executable it is installed as (herdr.dev/docs/agents).
const KINDS: &[(&str, &[&str])] = &[
    ("claude", &["claude"]),
    ("codex", &["codex"]),
    ("opencode", &["opencode"]),
    ("copilot", &["copilot"]),
    ("cursor", &["cursor-agent", "cursor"]),
    ("pi", &["pi"]),
    ("omp", &["omp"]),
    ("droid", &["droid"]),
    ("devin", &["devin"]),
    ("kimi", &["kimi"]),
    ("kilo", &["kilo"]),
    ("hermes", &["hermes"]),
    ("qodercli", &["qodercli"]),
    ("qwen", &["qwen"]),
    ("letta", &["letta"]),
    ("mastracode", &["mastracode"]),
    ("grok", &["grok"]),
    ("agy", &["antigravity-cli", "agy"]),
    ("amp", &["amp"]),
    ("kiro", &["kiro"]),
    ("maki", &["maki"]),
    ("gemini", &["gemini"]),
    ("cline", &["cline"]),
    ("muse", &["muse"]),
];

/// One agent kind that is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub kind: String,
    /// The executable found on PATH.
    pub executable: PathBuf,
    pub models: Vec<String>,
    /// Why there is no model list, when the CLI cannot print one.
    pub note: Option<String>,
    /// Whether the GEAR skill is installed for this kind (T-83, trial G06):
    /// without it the agent does not know how to work as a seat.
    pub skill: bool,
}

/// What this machine offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    pub installed: Vec<Installed>,
    /// The kinds Herdr can start that are not installed here.
    pub missing: Vec<String>,
}

impl Inventory {
    /// Probe the machine: PATH, then the model list of every kind found.
    pub fn probe() -> Inventory {
        let mut inventory = Inventory::default();
        for (kind, executables) in KINDS {
            match on_path(executables) {
                Some(executable) => {
                    let (models, note) = models_of(kind);
                    inventory.installed.push(Installed {
                        kind: (*kind).to_string(),
                        executable,
                        models,
                        note,
                        skill: skill_installed(kind),
                    });
                }
                None => inventory.missing.push((*kind).to_string()),
            }
        }
        inventory
    }
}

/// Is the GEAR skill installed for this kind (T-83)? A kind GEAR has no skill
/// install for cannot be driven unattended, so it counts as not installed.
fn skill_installed(kind: &str) -> bool {
    let Ok(agent) = crate::skill::SkillAgent::parse(kind) else {
        return false;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    crate::skill::installed(agent, std::path::Path::new(&home))
}

/// The executable of `names` that is on PATH, with its full path.
fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The models a kind can use, and the reason when it cannot list them.
pub fn models_of(kind: &str) -> (Vec<String>, Option<String>) {
    match kind {
        // OpenCode prints its model ids, one per line. An empty list right
        // after a start is "not loaded yet" (trial G03), never "no models":
        // look once more before saying so.
        "opencode" => {
            let mut last = (Vec::new(), None);
            for attempt in 0..2 {
                last = match Command::new("opencode").arg("models").output() {
                    Ok(out) if out.status.success() => {
                        let models = parse_opencode_models(&String::from_utf8_lossy(&out.stdout));
                        if !models.is_empty() {
                            return (models, None);
                        }
                        // Nothing yet: say so, and look again once.
                        (models, Some("model list not loaded yet".to_string()))
                    }
                    Ok(out) => (
                        Vec::new(),
                        Some(format!("`opencode models` exited {}", out.status)),
                    ),
                    Err(e) => (
                        Vec::new(),
                        Some(format!("cannot run `opencode models`: {e}")),
                    ),
                };
                if attempt == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
            last
        }
        // Codex caches the models it knows in its cache file.
        "codex" => match std::fs::read_to_string(codex_cache()) {
            Ok(text) => match parse_codex_models(&text) {
                Ok(models) => (models, None),
                Err(e) => (Vec::new(), Some(e.detail)),
            },
            Err(e) => (
                Vec::new(),
                Some(format!("cannot read {}: {e}", codex_cache().display())),
            ),
        },
        // Pi lists its models in a table: `provider  model  context  …`. The
        // id pi's `--model` takes is `provider/model` (T-113).
        "pi" => match Command::new("pi").arg("--list-models").output() {
            Ok(out) if out.status.success() => {
                let models = parse_pi_models(&String::from_utf8_lossy(&out.stdout));
                if models.is_empty() {
                    (
                        Vec::new(),
                        Some("no model listed by `pi --list-models`".to_string()),
                    )
                } else {
                    (models, None)
                }
            }
            Ok(out) => (
                Vec::new(),
                Some(format!("`pi --list-models` exited {}", out.status)),
            ),
            Err(e) => (
                Vec::new(),
                Some(format!("cannot run `pi --list-models`: {e}")),
            ),
        },
        // The other CLIs do not print their models.
        _ => (Vec::new(), Some("no model list from this CLI".to_string())),
    }
}

/// The configured model ids that the CLI running them does not list (T-315):
/// a warning for the captain, never a refusal — a list can be not loaded yet
/// or differ by machine (B8).
///
/// Every lane's chain, the review chain (only when the project names `[temps]`)
/// and the captain chain are checked, in that order. An entry's CLI is the one
/// it names, else the kind of the seat that runs it. `listed` runs a CLI (that
/// can wait), so it is called once per CLI and its answer is kept.
pub fn model_warnings(
    config: &crate::config::ProjectConfig,
    listed: &mut dyn FnMut(&str) -> (Vec<String>, Option<String>),
) -> Vec<String> {
    let mut asker = Asker {
        listed,
        answers: Vec::new(),
        noted: Vec::new(),
    };
    let mut warnings = Vec::new();
    for lane in &config.lanes {
        let chain = crate::config::lane_chain(config, lane);
        asker.check(
            &format!("lane {}", lane.name),
            &lane.kind,
            chain,
            &mut warnings,
        );
    }
    if let Some(temps) = &config.temps {
        asker.check(
            "reviewers",
            &temps.kind,
            &config.models.review,
            &mut warnings,
        );
    }
    asker.check(
        "captain",
        &config.captain.kind,
        &config.models.captain,
        &mut warnings,
    );
    warnings
}

/// The model lists asked so far, and the CLIs whose "not checked" line is
/// already said: one line per CLI, however many entries name it.
struct Asker<'a> {
    listed: &'a mut dyn FnMut(&str) -> (Vec<String>, Option<String>),
    answers: Vec<(String, (Vec<String>, Option<String>))>,
    noted: Vec<String>,
}

impl Asker<'_> {
    /// The models `cli` lists, asking it the first time only.
    fn models_of_cli(&mut self, cli: &str) -> (Vec<String>, Option<String>) {
        if let Some((_, answer)) = self.answers.iter().find(|(asked, _)| asked == cli) {
            return answer.clone();
        }
        let answer = (self.listed)(cli);
        self.answers.push((cli.to_string(), answer.clone()));
        answer
    }

    /// Check one chain: `who` is the seat the line names, and `seat_kind` the
    /// kind that runs an entry naming no CLI.
    fn check(
        &mut self,
        who: &str,
        seat_kind: &str,
        chain: &[crate::config::ModelEntry],
        warnings: &mut Vec<String>,
    ) {
        for entry in chain {
            let cli = entry.cli().unwrap_or(seat_kind);
            let (models, note) = self.models_of_cli(cli);
            if models.is_empty() {
                // A CLI that prints none says nothing; a list that could not be
                // read is worth one line per CLI.
                if let Some(note) = note {
                    if !self.noted.iter().any(|said| said == cli) {
                        self.noted.push(cli.to_string());
                        warnings.push(format!("model ids of {cli} not checked: {note}"));
                    }
                }
                continue;
            }
            if !models.iter().any(|model| model == entry.model()) {
                warnings.push(format!(
                    "{who}: model {} is not among the models {cli} lists",
                    entry.model()
                ));
            }
        }
    }
}

/// Where Codex keeps the models it knows.
fn codex_cache() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".codex/models_cache.json")
}

/// The model ids of `opencode models`: one per line, as printed.
pub fn parse_opencode_models(text: &str) -> Vec<String> {
    let mut models: Vec<String> = Vec::new();
    for line in text.lines() {
        let model = line.trim();
        if model.is_empty() || model.starts_with('#') {
            continue;
        }
        if !models.iter().any(|seen| seen == model) {
            models.push(model.to_string());
        }
    }
    models
}

/// The slugs of `~/.codex/models_cache.json` (`{"models": [{"slug": …}]}`).
pub fn parse_codex_models(text: &str) -> GearResult<Vec<String>> {
    let doc: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| GearError::new(ErrorCode::BadFormat, format!("models_cache.json: {e}")))?;
    let models = doc
        .get("models")
        .and_then(|models| models.as_array())
        .ok_or_else(|| {
            GearError::new(
                ErrorCode::BadFormat,
                "models_cache.json has no models array".to_string(),
            )
        })?;
    let mut slugs: Vec<String> = Vec::new();
    for model in models {
        if let Some(slug) = model.get("slug").and_then(|slug| slug.as_str()) {
            if !slug.is_empty() && !slugs.iter().any(|seen| seen == slug) {
                slugs.push(slug.to_string());
            }
        }
    }
    Ok(slugs)
}

/// The model ids of `pi --list-models`: the first two columns of every row,
/// `provider/model` — the form pi's `--model` takes (T-113).
pub fn parse_pi_models(text: &str) -> Vec<String> {
    let mut models = Vec::new();
    for line in text.lines() {
        let mut columns = line.split_whitespace();
        let (Some(provider), Some(model)) = (columns.next(), columns.next()) else {
            continue;
        };
        if provider == "provider" {
            continue; // the header row
        }
        let id = format!("{provider}/{model}");
        if !models.iter().any(|seen| seen == &id) {
            models.push(id);
        }
    }
    models
}

/// The kind the lanes run: OpenCode when it is installed (GEAR's built-in
/// unattended profile), else the first installed kind that lists models.
fn lane_kind(inventory: &Inventory) -> Option<&Installed> {
    inventory
        .installed
        .iter()
        .find(|installed| installed.kind == "opencode" && !installed.models.is_empty())
        .or_else(|| {
            inventory
                .installed
                .iter()
                .find(|installed| !installed.models.is_empty())
        })
}

/// The captain's kind: Claude when it is installed (the doorbell profile),
/// else the first installed kind.
fn captain_kind(inventory: &Inventory) -> Option<&Installed> {
    inventory
        .installed
        .iter()
        .find(|installed| installed.kind == "claude")
        .or_else(|| inventory.installed.first())
}

/// A fast, cheap model — the shape a lane runs. The markers are a heuristic
/// (the user confirms the chain); "mini" is not one of them, because it also
/// matches the vendor name MiniMax.
fn is_fast(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    ["flash", "haiku", "free"]
        .iter()
        .any(|marker| model.contains(marker))
}

/// The lanes' chain: up to two fast models, else the first two listed.
fn lane_chain(models: &[String]) -> Vec<String> {
    let fast: Vec<&String> = models.iter().filter(|model| is_fast(model)).collect();
    if fast.is_empty() {
        return models.iter().take(2).cloned().collect();
    }
    fast.iter().take(2).map(|model| (*model).clone()).collect()
}

/// The review's chain: up to two models that differ from every lane pick and
/// are not fast — the reviewer never runs the writer's model.
fn review_chain(models: &[String], lanes: &[String]) -> Vec<String> {
    let mut chain: Vec<String> = models
        .iter()
        .filter(|model| !is_fast(model) && !lanes.contains(model))
        .take(2)
        .cloned()
        .collect();
    if chain.is_empty() {
        chain = models
            .iter()
            .filter(|model| !lanes.contains(model))
            .take(2)
            .cloned()
            .collect();
    }
    chain
}

/// The inventory as text: the agents installed, their models, and the
/// gear.toml skeleton.
pub fn render(inventory: &Inventory) -> String {
    format!("{}\n\n{}", report(inventory), skeleton(inventory))
}

/// The agents and their models, as the captain reads them.
pub fn report(inventory: &Inventory) -> String {
    let mut lines = vec![
        "GEAR kickoff — what this machine offers (read-only: nothing is written, nothing is started)"
            .to_string(),
        String::new(),
        "Agents installed (the kinds Herdr can start):".to_string(),
    ];
    for installed in &inventory.installed {
        let count = match (installed.models.len(), &installed.note) {
            (0, Some(note)) => note.clone(),
            // T-83: a list that is empty and says nothing is not loaded yet —
            // never "no models" (trial G03).
            (0, None) => "model list not loaded yet".to_string(),
            (1, _) => "1 model".to_string(),
            (n, _) => format!("{n} models"),
        };
        // Trial G06: a seat whose CLI does not have the skill cannot work as
        // one — say so per kind, with the command that fixes it.
        let skill = if installed.skill {
            " · GEAR skill installed".to_string()
        } else if crate::skill::SkillAgent::parse(&installed.kind).is_ok() {
            format!(
                " · GEAR skill MISSING — run `gear admin install-skill --agent {}`",
                installed.kind
            )
        } else {
            " · no GEAR skill for this kind".to_string()
        };
        lines.push(format!(
            "  {:<10} {} — {count}{skill}",
            installed.kind,
            installed.executable.display()
        ));
        for model in &installed.models {
            lines.push(format!("    {model}"));
        }
    }
    if inventory.installed.is_empty() {
        lines.push("  none — install an agent Herdr can start".to_string());
    }
    lines.push(String::new());
    lines.push(format!(
        "Not on PATH: {}",
        if inventory.missing.is_empty() {
            "none".to_string()
        } else {
            inventory.missing.join(", ")
        }
    ));
    lines.join("\n")
}

/// The gear.toml skeleton: the detected kinds and the proposed model chains,
/// with the TODOs the user fills in. Valid TOML; `gear admin init` reports
/// whatever is still unfinished.
///
/// The repositories below the project's root are listed as commented
/// `[[repos]]` blocks (T-54): the captain confirms the ones this voyage
/// changes.
pub fn skeleton(inventory: &Inventory) -> String {
    skeleton_for(inventory, current_root().as_deref())
}

/// The skeleton for a known project root: `repo` is that root and the
/// repositories below it are listed. A root that is not known (`None`) leaves
/// `repo` as a TODO and lists no repository.
pub fn skeleton_for(inventory: &Inventory, root: Option<&Path>) -> String {
    let lane = lane_kind(inventory);
    let captain = captain_kind(inventory);
    let models: &[String] = lane
        .map(|installed| installed.models.as_slice())
        .unwrap_or(&[]);
    let lanes = lane_chain(models);
    let review = review_chain(models, &lanes);
    let lane_kind = lane
        .map(|installed| installed.kind.as_str())
        .unwrap_or("TODO");
    let captain_kind = captain
        .map(|installed| installed.kind.as_str())
        .unwrap_or("TODO");

    let chain = |models: &[String]| -> String {
        if models.is_empty() {
            "\"TODO\"".to_string()
        } else {
            models
                .iter()
                .map(|model| format!("\"{model}\""))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    // T-234: a chain of one model has nowhere to fall back to — that role
    // waits for the reset instead, and `gear admin init` accepts the config.
    let policy = |chain: &[String]| -> &'static str {
        if chain.len() > 1 {
            "\"fallback\""
        } else {
            "\"wait\""
        }
    };

    // Filled from where kickoff runs when it can (inside a Herdr pane, in the
    // project's repository); otherwise left as TODO.
    let env_or_todo = |name: &str| {
        std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "TODO".into())
    };
    let session = env_or_todo("HERDR_SESSION");
    let pane = env_or_todo("HERDR_PANE_ID");
    let repo = root
        .map(|root| root.display().to_string())
        .unwrap_or_else(|| "TODO".into());
    let repos = root.map(nested_repos).unwrap_or_default();
    let mut lines = vec![
        "# gear.toml — fill in the TODOs, then register it:".to_string(),
        "#   GEAR_ADMIN=1 gear admin init gear/gear.toml".to_string(),
        String::new(),
        "[project]".to_string(),
        "id = \"TODO\"                  # [a-z][a-z0-9-]*: the project's short name".to_string(),
        format!("herdr_session = \"{session}\"   # the Herdr session the agents live in"),
        format!("repo = \"{repo}\"   # the repository the lanes work in"),
        String::new(),
        "[quota]".to_string(),
        "lanes = 3                     # long-lived lanes".to_string(),
        "temps = 2                     # temp workers (reviewers) that may run at once".to_string(),
        "includes_captain = false".to_string(),
        String::new(),
        "[captain]".to_string(),
        format!("pane = \"{pane}\"   # the pane the captain runs in (w<n>:p<n>)"),
        format!("kind = \"{captain_kind}\""),
        format!("profile = \"{captain_kind}\""),
        String::new(),
        "[[lanes]]                     # one block per long-lived lane".to_string(),
        "name = \"lane-1\"               # [a-z][a-z0-9-]*; its duty is one sentence".to_string(),
        "duty = \"TODO: what this lane is for\"".to_string(),
        "owns = [\"TODO/\"]               # the paths it writes; lanes must not overlap"
            .to_string(),
        "not = []".to_string(),
        format!("kind = \"{lane_kind}\""),
        format!("profile = \"{lane_kind}\""),
        pad(
            "# repo = \"TODO\"",
            "# the [[repos]] name this lane works in; empty = the project root",
        ),
    ];
    // The repositories below the project's root, as proposals (T-54): the
    // captain keeps the ones this voyage changes and names a lane after each.
    for nested in &repos {
        lines.push(String::new());
        lines.push(pad(
            "# [[repos]]",
            "# one block per repository below the project's root",
        ));
        lines.push(pad(
            &format!("# name = \"{}\"", nested.name),
            &format!(
                "# [a-z][a-z0-9-]*; a lane binds to it with repo = \"{}\"",
                nested.name
            ),
        ));
        lines.push(pad(
            &format!("# path = \"{}\"", nested.path),
            "# relative to project.repo",
        ));
        lines.push(pad(
            &format!("# integration = \"{}\"", nested.integration),
            "# that repository's integration branch (its release branch must differ)",
        ));
    }
    lines.extend([
        String::new(),
        "[temps]".to_string(),
        format!("kind = \"{lane_kind}\""),
        format!("profile = \"{lane_kind}\""),
        String::new(),
        "[models]                      # full model ids; the user confirms them at kickoff".to_string(),
        format!(
            "lanes = [{}]   # fast and cheap: tasks are cut small",
            chain(&lanes)
        ),
        format!(
            "review = [{}]   # stronger, and never the writer's model",
            chain(&review)
        ),
        format!(
            "on_quota = {{ lanes = {}, review = {} }}   # then wait for the reset",
            policy(&lanes),
            policy(&review)
        ),
        "on_wait = \"keep\"              # \"release\": a lane parked on a quota wait exits until the reset; its worktree and session are kept".to_string(),
        "quota_reset_h = 5".to_string(),
        String::new(),
        "[conventions]".to_string(),
        "integration = \"dev\"           # the branch results are merged into (it must exist)".to_string(),
        "release = \"main\"              # the release branch".to_string(),
        // T-255: the gate has no default, so the skeleton names it as empty
        // and asks the captain for the command this project actually runs.
        "gate = \"\"                     # fill in: the command the merge runs in the integration worktree; it must exit non-zero on any failure (e.g. \"make gate\", \"go test ./...\")".to_string(),
    ]);
    // A kind Herdr can start but GEAR has no built-in profile for (T-31): the
    // user fills the block in — the comment says which fields matter.
    for installed in &inventory.installed {
        if gear_herdr::builtin(&installed.kind).is_some() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!(
            "# [profiles.{}]                 # this kind has no built-in profile: fill it in",
            installed.kind
        ));
        lines.push(format!("# kind = \"{}\"", installed.kind));
        lines.push("# unattended_args = [\"--auto\"]   # required: no permission dialogs, or the agent stops".to_string());
        lines.push(
            "# busy_markers = []             # the screen text of a running turn".to_string(),
        );
        lines.push(
            "# quota_markers = []            # the screen text of a provider limit".to_string(),
        );
        lines.push(
            "# exit_keys = [\"ctrl+c\"]        # the keys that make it exit cleanly".to_string(),
        );
        lines.push(
            "# tail_lines = 16                # the lines of the screen that are matched"
                .to_string(),
        );
        lines.push(
            "# confirm_rounds = 2             # consecutive observations before a flag counts"
                .to_string(),
        );
        lines.push(
            "# resume_args = [\"-s\", \"{session}\"]   # optional: how a session is resumed"
                .to_string(),
        );
    }
    lines.push(String::new());
    lines.join("\n")
}

/// The inventory as JSON, for `gear admin kickoff --json`.
pub fn to_json(inventory: &Inventory) -> GearResult<String> {
    let installed: Vec<serde_json::Value> = inventory
        .installed
        .iter()
        .map(|installed| {
            serde_json::json!({
                "kind": installed.kind,
                "executable": installed.executable.display().to_string(),
                "models": installed.models,
                "note": installed.note,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "installed": installed,
        "missing": inventory.missing,
        "skeleton": skeleton(inventory),
    });
    serde_json::to_string(&doc).map_err(|e| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot serialize the inventory: {e}"),
        )
    })
}

// ---------------------------------------------------------------- the repos

/// A git repository below the project's root (T-54).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedRepo {
    /// The directory's name: the repo's name in the config.
    pub name: String,
    /// The repo's path, relative to the project's root.
    pub path: String,
    /// The branch the repo is on now: its integration branch.
    pub integration: String,
}

/// The column the skeleton's comments start in, so the file reads as one.
const COMMENT_COLUMN: usize = 30;

/// A skeleton line: the value, then its comment in the same column, whatever
/// the value is.
fn pad(line: &str, comment: &str) -> String {
    let width = line.chars().count();
    if width >= COMMENT_COLUMN {
        format!("{line} {comment}")
    } else {
        format!("{line}{}{comment}", " ".repeat(COMMENT_COLUMN - width))
    }
}

/// The repositories below `root` (T-54): the directories that hold a `.git`,
/// at most [`MAX_REPO_DEPTH`] levels down, without `target/`,
/// `node_modules/` and `.git/`.
///
/// The root itself is not one of them: it is the project.
pub fn nested_repos(root: &Path) -> Vec<NestedRepo> {
    let mut repos = Vec::new();
    collect_repos(root, root, 0, &mut repos);
    repos.sort_by(|a, b| a.path.cmp(&b.path));
    repos
}

/// The directories below `dir`, one level down: a repository is a directory
/// that holds a `.git` (a clone's or a worktree's).
fn collect_repos(root: &Path, dir: &Path, depth: usize, out: &mut Vec<NestedRepo>) {
    if depth >= MAX_REPO_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    for path in dirs {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if SKIPPED_DIRS.contains(&name) {
            continue;
        }
        if path.join(".git").exists() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(NestedRepo {
                name: name.to_string(),
                path: relative,
                integration: current_branch(&path),
            });
        }
        collect_repos(root, &path, depth + 1, out);
    }
}

/// The branch a repository is on now, or "TODO" when git cannot say (a
/// detached HEAD, a repository without a commit).
fn current_branch(repo: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output();
    let Ok(output) = output else {
        return "TODO".to_string();
    };
    if !output.status.success() {
        return "TODO".to_string();
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        "TODO".to_string()
    } else {
        branch
    }
}

/// The repository the kickoff runs in, when it runs in one.
fn current_root() -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{self, ProjectConfig};

    /// A config for project `p`: one lane called `heat` on `lane_kind` running
    /// `lane_models`, the review chain `review` on the temps seat (absent
    /// without `temps_kind`), and the captain chain `captain` on `captain_kind`.
    /// The chains are written as one TOML entry each, so a test reads the way
    /// the config does.
    fn project(
        lane_kind: &str,
        lane_models: &str,
        temps_kind: Option<&str>,
        review: &str,
        captain_kind: &str,
        captain: &str,
    ) -> ProjectConfig {
        let mut text = format!(
            "[project]\nid = \"p\"\nherdr_session = \"s\"\nrepo = \"/tmp/p\"\n\
             \n[quota]\nlanes = 1\ntemps = 1\n\
             \n[captain]\npane = \"w1:p1\"\nkind = \"{captain_kind}\"\nprofile = \"{captain_kind}\"\n\
             \n[[lanes]]\nname = \"heat\"\nduty = \"everything\"\nowns = [\"src/\"]\n\
             kind = \"{lane_kind}\"\nprofile = \"{lane_kind}\"\nmodels = [{lane_models}]\n"
        );
        if let Some(temps_kind) = temps_kind {
            text.push_str(&format!(
                "\n[temps]\nkind = \"{temps_kind}\"\nprofile = \"{temps_kind}\"\n"
            ));
        }
        text.push_str(&format!(
            "\n[models]\nreview = [{review}]\ncaptain = [{captain}]\n"
        ));
        config::parse(&text).expect("a config")
    }

    /// T-315: a lane's model that its CLI does not list is a warning naming
    /// the lane; a listed one says nothing.
    #[test]
    fn a_lane_model_its_cli_does_not_list_is_a_warning() {
        let config = project(
            "opencode",
            "\"tok/listed\", \"tok/gone\"",
            None,
            "",
            "codex",
            "",
        );
        let warnings = model_warnings(&config, &mut |cli| {
            assert_eq!(cli, "opencode", "the lane's own kind runs a plain entry");
            (vec!["tok/listed".to_string()], None)
        });
        assert_eq!(
            warnings,
            vec!["lane heat: model tok/gone is not among the models opencode lists"]
        );

        let config = project("opencode", "\"tok/listed\"", None, "", "codex", "");
        let warnings = model_warnings(&config, &mut |_| (vec!["tok/listed".to_string()], None));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// T-315: an entry that names a CLI is checked against that CLI's list,
    /// not against the seat's kind.
    #[test]
    fn an_entry_naming_a_cli_is_checked_against_its_cli() {
        let config = project(
            "opencode",
            "{ model = \"gpt-6.1-sol\", cli = \"codex\" }",
            None,
            "",
            "codex",
            "",
        );
        let warnings = model_warnings(&config, &mut |cli| match cli {
            "codex" => (vec!["gpt-6.1-mini".to_string()], None),
            other => panic!("asked {other}: the entry names codex"),
        });
        assert_eq!(
            warnings,
            vec!["lane heat: model gpt-6.1-sol is not among the models codex lists"]
        );
    }

    /// T-459: a review entry that names a CLI is checked against that CLI's
    /// list, not against the `[temps]` kind the entry overrides.
    #[test]
    fn a_review_entry_naming_a_cli_is_checked_against_its_cli() {
        let config = project(
            "pi",
            "\"tok/deep-1\"",
            Some("pi"),
            "{ model = \"gpt-6.1-terra\", cli = \"codex\" }",
            "codex",
            "",
        );
        let mut asked: Vec<String> = Vec::new();
        let warnings = model_warnings(&config, &mut |cli| {
            asked.push(cli.to_string());
            match cli {
                "pi" => (vec!["tok/deep-1".to_string()], None),
                "codex" => (vec!["gpt-6.1-mini".to_string()], None),
                other => panic!("asked {other}: the review entry names codex"),
            }
        });
        assert_eq!(
            warnings,
            vec!["reviewers: model gpt-6.1-terra is not among the models codex lists"]
        );
        assert_eq!(
            asked,
            vec!["pi", "codex"],
            "the review entry's CLI, not the temps kind"
        );
    }

    /// T-315: a CLI that lists none but says why gives one "not checked" line,
    /// however many entries name it.
    #[test]
    fn a_cli_that_cannot_list_says_so_once() {
        let config = project(
            "opencode",
            "\"a/one\", \"a/two\", \"a/three\"",
            None,
            "",
            "codex",
            "",
        );
        let mut asked: Vec<String> = Vec::new();
        let warnings = model_warnings(&config, &mut |cli| {
            asked.push(cli.to_string());
            (Vec::new(), Some("no model list from this CLI".to_string()))
        });
        assert_eq!(
            warnings,
            vec!["model ids of opencode not checked: no model list from this CLI"]
        );
        assert_eq!(asked, vec!["opencode"], "one line per CLI: asked once");
    }

    /// T-315: a CLI that lists none and says nothing (a CLI that prints none)
    /// says nothing here either.
    #[test]
    fn a_cli_with_no_list_and_no_note_is_silent() {
        let config = project("opencode", "\"a/one\"", None, "", "codex", "");
        let warnings = model_warnings(&config, &mut |_| (Vec::new(), None));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// T-315: each CLI is asked once, across every lane, the review chain and
    /// the captain chain.
    #[test]
    fn each_cli_is_asked_once_across_the_chains() {
        let config = project(
            "pi",
            "\"tok/deep-1\"",
            Some("pi"),
            "\"tok/deep-1\"",
            "codex",
            "\"gpt-6.1-sol\"",
        );
        let mut asked: Vec<String> = Vec::new();
        let warnings = model_warnings(&config, &mut |cli| {
            asked.push(cli.to_string());
            match cli {
                "pi" => (vec!["tok/deep-1".to_string()], None),
                "codex" => (vec!["gpt-6.1-sol".to_string()], None),
                other => panic!("asked {other}, no chain names it"),
            }
        });
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(asked, vec!["pi", "codex"], "once per CLI");
    }

    /// T-315: the review chain's line names the reviewers, and a project
    /// without `[temps]` has no review chain to check.
    #[test]
    fn the_review_chain_names_the_reviewers_and_needs_temps() {
        let config = project(
            "pi",
            "\"tok/deep-1\"",
            Some("pi"),
            "\"tok/deep-2\"",
            "codex",
            "",
        );
        let warnings = model_warnings(&config, &mut |_| (vec!["tok/deep-1".to_string()], None));
        assert_eq!(
            warnings,
            vec!["reviewers: model tok/deep-2 is not among the models pi lists"]
        );

        let config = project("pi", "\"tok/deep-1\"", None, "\"tok/deep-2\"", "codex", "");
        let warnings = model_warnings(&config, &mut |_| (vec!["tok/deep-1".to_string()], None));
        assert!(
            warnings.is_empty(),
            "no [temps], no review chain: {warnings:?}"
        );
    }

    /// T-315: the captain chain's line names the captain.
    #[test]
    fn the_captain_chain_names_the_captain() {
        let config = project("pi", "\"tok/deep-1\"", None, "", "codex", "\"gpt-6.1-sol\"");
        let warnings = model_warnings(&config, &mut |cli| match cli {
            "pi" => (vec!["tok/deep-1".to_string()], None),
            "codex" => (vec!["gpt-6.1-mini".to_string()], None),
            other => panic!("asked {other}"),
        });
        assert_eq!(
            warnings,
            vec!["captain: model gpt-6.1-sol is not among the models codex lists"]
        );
    }
}
