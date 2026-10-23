//! How a CLI takes the model it starts on (design/01 `[models]`, 17 §0).
//!
//! The knowledge of one CLI lives here and not in [`gear_herdr::Profile`]:
//! a profile describes how a CLI is observed and started, not which model it
//! runs on, and adding a field to it would change a public type.

use gear_app::config::{LaneSection, ProjectConfig};

/// The primary model of a lane (17 §0): the first entry of the lane's own
/// chain, else the first entry of the lane chain (`[models].lanes`). `None`
/// when no chain is configured — then GEAR does not manage the model.
pub(crate) fn lane_primary<'a>(
    lane: &'a LaneSection,
    lane_chain: &'a [String],
) -> Option<&'a str> {
    lane.models
        .first()
        .or_else(|| lane_chain.first())
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
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
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());
    }
    let lane = config.lanes.iter().find(|lane| lane.name == name)?;
    lane_primary(lane, &config.models.lanes).map(|model| model.to_string())
}

/// The model a reviewer runs on: the first of the review chain that differs
/// from the writer's (17 §0: the reviewer never runs the model it reviews).
/// `None` when the chain is empty — then the model is not managed.
pub(crate) fn review_model(config: &ProjectConfig, writer_seat: &str) -> Option<String> {
    let writer = writer_model(config, writer_seat);
    config
        .models
        .review
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .find(|model| Some(*model) != writer.as_deref())
        .map(|model| model.to_string())
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
/// share it, and the reasoning effort with `-c model_reasoning_effort=high`
/// (SOP `gear --skill agent:codex`).
///
/// A CLI without a verified way gets nothing: its model is not managed by GEAR.
pub(crate) fn model_start(kind: &str, model: &str) -> Option<ModelStart> {
    match kind {
        "opencode" => Some(ModelStart {
            env: format!("export OPENCODE_CONFIG_CONTENT='{{\"model\":\"{model}\"}}'"),
            // No `--standalone`: private servers exited on their own twice
            // on 2026-10-23 (the lane disappeared). Without it the model in
            // the environment is not applied by the shared service; switching
            // the model through `/model` by the engine is M2 (L-56).
            args: Vec::new(),
        }),
        "codex" => Some(ModelStart {
            env: String::new(),
            args: vec![
                "-m".to_string(),
                model.to_string(),
                "-c".to_string(),
                "model_reasoning_effort=high".to_string(),
            ],
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_takes_the_model_in_the_environment() {
        let start = model_start("opencode", "minimax-cn/MiniMax-M3.1-Flash").unwrap();
        assert_eq!(
            start.env,
            "export OPENCODE_CONFIG_CONTENT='{\"model\":\"minimax-cn/MiniMax-M3.1-Flash\"}'"
        );
        assert!(start.args.is_empty(), "no --standalone (L-56): {:?}", start.args);
    }

    #[test]
    fn codex_takes_the_model_as_a_flag() {
        let start = model_start("codex", "gpt-6.1-sol").unwrap();
        assert_eq!(start.env, "", "codex needs no environment line");
        assert_eq!(
            start.args,
            vec![
                "-m".to_string(),
                "gpt-6.1-sol".to_string(),
                "-c".to_string(),
                "model_reasoning_effort=high".to_string(),
            ]
        );
    }

    #[test]
    fn a_cli_without_a_verified_way_manages_no_model() {
        assert!(model_start("claude", "gpt-6.1-sol").is_none());
    }
}
