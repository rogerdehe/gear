//! `gear admin kickoff` (PRD §8a): what this machine offers, for the captain
//! to turn into a proposal the user corrects in one pass.
//!
//! Read-only and silent: it writes nothing, starts nothing and touches no
//! Herdr state. It only reads PATH, `opencode models` and
//! `~/.codex/models_cache.json` — the inventory a kickoff proposes from
//! (design/01: "no silent discovery", the user confirms every choice).

use std::path::PathBuf;
use std::process::Command;

use gear_model::{ErrorCode, GearError, GearResult};

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
                    });
                }
                None => inventory.missing.push((*kind).to_string()),
            }
        }
        inventory
    }
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
fn models_of(kind: &str) -> (Vec<String>, Option<String>) {
    match kind {
        // OpenCode prints its model ids, one per line.
        "opencode" => match Command::new("opencode").arg("models").output() {
            Ok(out) if out.status.success() => {
                (parse_opencode_models(&String::from_utf8_lossy(&out.stdout)), None)
            }
            Ok(out) => (
                Vec::new(),
                Some(format!("`opencode models` exited {}", out.status)),
            ),
            Err(e) => (Vec::new(), Some(format!("cannot run `opencode models`: {e}"))),
        },
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
        // The other CLIs do not print their models.
        _ => (Vec::new(), Some("no model list from this CLI".to_string())),
    }
}

/// Where Codex keeps the models it knows.
fn codex_cache() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
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
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        GearError::new(ErrorCode::BadFormat, format!("models_cache.json: {e}"))
    })?;
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
            (0, None) => "no models".to_string(),
            (1, _) => "1 model".to_string(),
            (n, _) => format!("{n} models"),
        };
        lines.push(format!(
            "  {:<10} {} — {count}",
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
pub fn skeleton(inventory: &Inventory) -> String {
    let lane = lane_kind(inventory);
    let captain = captain_kind(inventory);
    let models: &[String] = lane.map(|installed| installed.models.as_slice()).unwrap_or(&[]);
    let lanes = lane_chain(models);
    let review = review_chain(models, &lanes);
    let lane_kind = lane.map(|installed| installed.kind.as_str()).unwrap_or("TODO");
    let captain_kind = captain.map(|installed| installed.kind.as_str()).unwrap_or("TODO");

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

    let mut lines = vec![
        "# gear.toml — fill in the TODOs, then register it:".to_string(),
        "#   GEAR_ADMIN=1 gear admin init gear/gear.toml".to_string(),
        String::new(),
        "[project]".to_string(),
        "id = \"TODO\"                  # [a-z][a-z0-9-]*: the project's short name".to_string(),
        "herdr_session = \"TODO\"       # the Herdr session it runs in".to_string(),
        "repo = \"TODO\"                # the repository the lanes work in".to_string(),
        String::new(),
        "[quota]".to_string(),
        "lanes = 3                     # long-lived lanes".to_string(),
        "temps = 2                     # temp workers (reviewers) that may run at once".to_string(),
        "includes_captain = false".to_string(),
        String::new(),
        "[captain]".to_string(),
        "pane = \"TODO\"                # the pane the captain runs in (w<n>:p<n>)".to_string(),
        format!("kind = \"{captain_kind}\""),
        format!("profile = \"{captain_kind}\""),
        String::new(),
        "[[lanes]]                     # one block per long-lived lane (17 §1)".to_string(),
        "name = \"lane-1\"               # [a-z][a-z0-9-]*; its duty is one sentence".to_string(),
        "duty = \"TODO: what this lane is for\"".to_string(),
        "owns = [\"TODO/\"]               # the paths it writes; lanes must not overlap".to_string(),
        "not = []".to_string(),
        format!("kind = \"{lane_kind}\""),
        format!("profile = \"{lane_kind}\""),
        String::new(),
        "[temps]".to_string(),
        format!("kind = \"{lane_kind}\""),
        format!("profile = \"{lane_kind}\""),
        String::new(),
        "[models]                      # the user confirms these at kickoff (17 §0)".to_string(),
        format!(
            "lanes = [{}]   # fast and cheap: tasks are cut small",
            chain(&lanes)
        ),
        format!(
            "review = [{}]   # stronger, and never the writer's model",
            chain(&review)
        ),
        "on_quota = { lanes = \"fallback\", review = \"fallback\" }   # then wait for the reset"
            .to_string(),
        "quota_reset_h = 5".to_string(),
    ];
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
        lines.push("# busy_markers = []             # the screen text of a running turn".to_string());
        lines.push("# quota_markers = []            # the screen text of a provider limit".to_string());
        lines.push("# exit_keys = [\"ctrl+c\"]        # the keys that make it exit cleanly".to_string());
        lines.push("# tail_lines = 16                # the lines of the screen that are matched".to_string());
        lines.push("# confirm_rounds = 2             # consecutive observations before a flag counts".to_string());
        lines.push("# resume_args = [\"-s\", \"{session}\"]   # optional: how a session is resumed".to_string());
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
        GearError::new(ErrorCode::BadFormat, format!("cannot serialize the inventory: {e}"))
    })
}
