//! How a CLI takes the model it starts on (design/01 `[models]`, 17 §0, T-35).
//!
//! The knowledge of one CLI lives here and not in [`gear_herdr::Profile`]:
//! a profile describes how a CLI is observed and started, not which model it
//! runs on, and adding a field to it would change a public type.
//!
//! A chain entry may name the CLI that runs the model and the arguments that
//! CLI starts with (T-35): the model flags come from the CLI, the rest from
//! the entry. A chain may therefore cross CLIs.

use gear_app::config::{LaneSection, ModelEntry, ProjectConfig};

/// The primary model of a lane (17 §0): the first entry of the lane's own
/// chain, else the first entry of the lane chain (`[models].lanes`). `None`
/// when no chain is configured — then GEAR does not manage the model.
pub(crate) fn lane_primary<'a>(
    lane: &'a LaneSection,
    lane_chain: &'a [ModelEntry],
) -> Option<&'a ModelEntry> {
    lane.models
        .first()
        .or_else(|| lane_chain.first())
        .filter(|entry| !entry.model().is_empty())
}

/// The chain a role runs on: the lane's own chain when it has one, else the
/// role chain of the config (T-35). Empty entries are dropped.
pub(crate) fn role_chain<'a>(config: &'a ProjectConfig, lane_name: &str) -> Vec<&'a ModelEntry> {
    let own = config
        .lanes
        .iter()
        .find(|lane| lane.name == lane_name)
        .map(|lane| &lane.models);
    let chain = match own {
        Some(chain) if !chain.is_empty() => chain,
        _ => &config.models.lanes,
    };
    chain
        .iter()
        .filter(|entry| !entry.model().is_empty())
        .collect()
}

/// The chain a reviewer runs on: `[models].review`, empty entries dropped
/// (T-115). Its first entry is the reviewer's starting model, the next ones
/// its quota fallbacks.
pub(crate) fn review_chain(config: &ProjectConfig) -> Vec<&ModelEntry> {
    config
        .models
        .review
        .iter()
        .filter(|entry| !entry.model().is_empty())
        .collect()
}

/// The model the writer of an attempt runs on: the lane's primary model, or
/// the captain's own chain.
pub(crate) fn writer_model(config: &ProjectConfig, writer_seat: &str) -> Option<String> {
    let name = writer_seat.rsplit(':').next().unwrap_or_default();
    if name == "captain" {
        return config
            .models
            .captain
            .first()
            .map(|entry| entry.model().to_string())
            .filter(|model| !model.is_empty());
    }
    let lane = config.lanes.iter().find(|lane| lane.name == name)?;
    lane_primary(lane, &config.models.lanes).map(|entry| entry.model().to_string())
}

/// The model a reviewer runs on: the first of the review chain that differs
/// from the writer's **current** model (17 §0: the reviewer never runs the
/// model it reviews). `None` when the chain is empty — then the model is not
/// managed.
/// The entry a review of a task runs on (T-500): the `review-model:` the
/// captain named for it, when that names an entry of the review chain; else
/// the first entry that differs from the writer's model (T-35). Every review
/// of the task — first review and re-review — runs on the entry this returns.
pub(crate) fn review_entry_for<'a>(
    config: &'a ProjectConfig,
    writer_model: Option<&str>,
    wanted: Option<&str>,
) -> Option<&'a ModelEntry> {
    let chain = review_chain(config);
    if let Some(wanted) = wanted.filter(|model| !model.is_empty()) {
        if let Some(entry) = chain.iter().find(|entry| entry.model() == wanted) {
            return Some(*entry);
        }
    }
    chain
        .into_iter()
        .find(|entry| Some(entry.model()) != writer_model)
}

/// How a CLI takes the model it starts on.
pub(crate) struct ModelStart {
    /// The shell fragment that carries the model in the environment line
    /// (empty when the CLI takes it as a flag).
    pub(crate) env: String,
    /// Start args the model needs.
    pub(crate) args: Vec<String>,
}

/// The model a seat runs on, as its CLI gets it.
///
/// **OpenCode** has no `--model` flag: it takes the model from its config, and
/// the only config source a *client* can carry is the environment variable
/// `OPENCODE_CONFIG_CONTENT` (a JSON object). Verified 2026-10-22 on OpenCode
/// v2.1.2 — see `skills/gear/references/agents/opencode.md`.
///
/// The variable is read by the process that loads the config. Against the
/// shared background service that is the service, whose environment was fixed
/// when it started, so the model would be ignored; a private server
/// (`--standalone`) reads the environment of the pane it is started in, which
/// is where GEAR exports the model.
///
/// **Codex** takes the model per process (`-m <slug>`), so two agents never
/// share it. Its reasoning effort is a property of the chain entry
/// (`-c model_reasoning_effort=…`), never a constant here (T-35).
///
/// A CLI without a verified way gets nothing: its model is not managed by GEAR.
pub(crate) fn model_start(kind: &str, model: &str) -> Option<ModelStart> {
    match kind {
        "opencode" => Some(ModelStart {
            env: format!("export OPENCODE_CONFIG_CONTENT='{{\"model\":\"{model}\"}}'"),
            // No `--standalone`: private servers exited on their own twice
            // on 2026-10-23 (the lane disappeared). Without it the model in the
            // environment is not applied by the shared service; switching the
            // model through `/model` by the engine is M2 (L-56).
            args: Vec::new(),
        }),
        "codex" => Some(ModelStart {
            env: String::new(),
            // The reasoning effort comes from the chain entry (T-35): the
            // same model may be run with a different effort.
            args: vec!["-m".to_string(), model.to_string()],
        }),
        // Grok takes the model as a start flag (`-m <model>`; the SOP
        // `gear --skill agent:grok`).
        "grok" => Some(ModelStart {
            env: String::new(),
            args: vec!["-m".to_string(), model.to_string()],
        }),
        // Claude Code takes the model as a start flag (`--model <model>`).
        "claude" => Some(ModelStart {
            env: String::new(),
            args: vec!["--model".to_string(), model.to_string()],
        }),
        // Pi takes the model as a start flag too (`--model <provider>/<id>`;
        // the SOP `gear --skill agent:pi`). The thinking level is a property
        // of the chain entry (`--thinking <level>`), never a constant here
        // (T-35): the entry's args follow the model flag.
        "pi" => Some(ModelStart {
            env: String::new(),
            args: vec!["--model".to_string(), model.to_string()],
        }),
        // agy (Antigravity CLI 1.4.12) takes the model as a start flag
        // (`--model <id>`; `agy --help`). The reasoning level is a property
        // of the chain entry (`--effort low|medium|high|xhigh|max`), never a
        // constant here (T-35), like pi's `--thinking`.
        "agy" => Some(ModelStart {
            env: String::new(),
            args: vec!["--model".to_string(), model.to_string()],
        }),
        _ => None,
    }
}

/// The start arguments of one chain entry: the CLI's own way of taking the
/// model, then the arguments the entry names (T-35). `None` when the CLI has
/// no verified way to take a model.
pub(crate) fn entry_args(kind: &str, entry: &ModelEntry) -> Option<Vec<String>> {
    start_args(kind, entry.model(), entry.args())
}

/// The start arguments of a model on a CLI: the CLI's own way of taking the
/// model, then the arguments the chain entry names (T-35). `None` when the
/// CLI has no verified way to take a model.
pub(crate) fn start_args(kind: &str, model: &str, args: &[String]) -> Option<Vec<String>> {
    let start = model_start(kind, model)?;
    let mut all = start.args;
    all.extend(args.iter().cloned());
    Some(all)
}

/// The key of a seat's current model in the engine's meta: the model the seat
/// runs on right now (it may differ from the chain's first entry after a
/// fallback, T-35).
pub(crate) fn model_meta_key(seat: &str) -> String {
    format!("engine.model.{seat}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(model: &str, cli: Option<&str>, args: &[&str]) -> ModelEntry {
        ModelEntry::Entry(gear_app::config::ModelSpec {
            model: model.to_string(),
            cli: cli.map(str::to_string),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            context_window_k: None,
        })
    }

    #[test]
    fn opencode_takes_the_model_in_the_environment() {
        let start = model_start("opencode", "minimax-cn/MiniMax-M3.1-Flash").unwrap();
        assert_eq!(
            start.env,
            "export OPENCODE_CONFIG_CONTENT='{\"model\":\"minimax-cn/MiniMax-M3.1-Flash\"}'"
        );
        assert!(
            start.args.is_empty(),
            "no --standalone (L-56): {:?}",
            start.args
        );
    }

    #[test]
    fn codex_takes_the_model_as_a_flag() {
        let start = model_start("codex", "gpt-6.1-sol").unwrap();
        assert_eq!(start.env, "", "codex needs no environment line");
        assert_eq!(
            start.args,
            vec!["-m".to_string(), "gpt-6.1-sol".to_string()],
            "the reasoning effort is the entry's, not the code's (T-35)"
        );
    }

    #[test]
    fn a_cli_without_a_verified_way_manages_no_model() {
        assert!(model_start("emacs", "gpt-6.1-sol").is_none());
    }

    #[test]
    fn pi_takes_the_model_as_a_flag() {
        // T-113: pi `--model <provider>/<id>`; the thinking level comes from
        // the chain entry, not from the code.
        let pi = model_start("pi", "opencode-go/deepseek-v4.1-flash").unwrap();
        assert_eq!(pi.env, "", "pi needs no environment line");
        assert_eq!(
            pi.args,
            vec![
                "--model".to_string(),
                "opencode-go/deepseek-v4.1-flash".to_string()
            ]
        );
        let entry = entry(
            "opencode-go/deepseek-v4.1-flash",
            Some("pi"),
            &["--thinking", "high"],
        );
        assert_eq!(
            entry_args("pi", &entry).unwrap(),
            vec![
                "--model".to_string(),
                "opencode-go/deepseek-v4.1-flash".to_string(),
                "--thinking".to_string(),
                "high".to_string(),
            ]
        );
    }

    #[test]
    fn grok_and_claude_take_the_model_as_a_flag() {
        // T-32: grok `-m <model>`, claude `--model <model>`.
        let grok = model_start("grok", "grok-4.7").unwrap();
        assert_eq!(grok.env, "");
        assert_eq!(grok.args, vec!["-m".to_string(), "grok-4.7".to_string()]);
        let claude = model_start("claude", "claude-opus-4").unwrap();
        assert_eq!(claude.env, "");
        assert_eq!(
            claude.args,
            vec!["--model".to_string(), "claude-opus-4".to_string()]
        );
    }

    #[test]
    fn agy_takes_the_model_and_the_effort_from_the_entry() {
        // T-497: agy `--model <id>`; the reasoning effort is the chain
        // entry's (`--effort low|medium|high|xhigh|max`), like pi's thinking.
        let agy = model_start("agy", "gemini-3.8-flash-high").unwrap();
        assert_eq!(agy.env, "", "agy needs no environment line");
        assert_eq!(
            agy.args,
            vec!["--model".to_string(), "gemini-3.8-flash-high".to_string()]
        );
        let entry = entry("gemini-3.8-flash-high", Some("agy"), &["--effort", "high"]);
        assert_eq!(
            entry_args("agy", &entry).unwrap(),
            vec![
                "--model".to_string(),
                "gemini-3.8-flash-high".to_string(),
                "--effort".to_string(),
                "high".to_string(),
            ]
        );
    }

    /// T-35: the entry's arguments follow the model flag of its CLI.
    #[test]
    fn the_entry_arguments_follow_the_model_flag() {
        let codex = entry(
            "gpt-6.1-sol",
            Some("codex"),
            &["-c", "model_reasoning_effort=medium"],
        );
        assert_eq!(
            entry_args("codex", &codex).unwrap(),
            vec![
                "-m".to_string(),
                "gpt-6.1-sol".to_string(),
                "-c".to_string(),
                "model_reasoning_effort=medium".to_string(),
            ]
        );
        let claude = entry("claude-sonnet-5-5", Some("claude"), &["--effort", "high"]);
        assert_eq!(
            entry_args("claude", &claude).unwrap(),
            vec![
                "--model".to_string(),
                "claude-sonnet-5-5".to_string(),
                "--effort".to_string(),
                "high".to_string(),
            ]
        );
        // The plain form: the CLI's own way, no extra arguments.
        let plain = ModelEntry::Plain("gpt-6.1-sol".to_string());
        assert_eq!(
            entry_args("codex", &plain).unwrap(),
            vec!["-m".to_string(), "gpt-6.1-sol".to_string()]
        );
    }
}
