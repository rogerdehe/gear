//! The kickoff inventory contract (T-10): the parsers on captured outputs,
//! the gear.toml skeleton, and the report the captain reads.
//!
//! The probes themselves (`opencode models`, the Codex cache) are not run
//! here: the tests feed captured text, so they say the same thing on every
//! machine.

use std::path::PathBuf;

use gear_app::kickoff::{self, Installed, Inventory};

/// A captured `opencode models`: one model id per line, with the blank lines,
/// a comment and a repeat the parser has to survive.
const OPENCODE_MODELS: &str = "\
deepseek/deepseek-flash
deepseek/deepseek-v4-pro
minimax-cn/MiniMax-M3
minimax-cn/MiniMax-M3.1-Flash

# one comment, if the CLI ever prints one
opencode-go/deepseek-v4.1-flash
deepseek/deepseek-flash
  opencode-go/glm-5.3-flash
";

/// A captured `~/.codex/models_cache.json`, trimmed to what the parser reads.
const CODEX_CACHE: &str = r#"{
  "fetched_at": "2026-10-23T04:15:50.630332Z",
  "models": [
    {"slug": "gpt-6.1-sol", "display_name": "GPT-6.1-Sol"},
    {"slug": "gpt-6-astra", "display_name": "GPT-6-Astra"},
    {"display_name": "an entry without a slug"},
    {"slug": ""}
  ]
}"#;

/// An inventory shaped like this machine's: one CLI without a model list,
/// OpenCode with four, and one kind missing.
fn inventory() -> Inventory {
    Inventory {
        installed: vec![
            Installed {
                kind: "claude".into(),
                executable: PathBuf::from("/usr/local/bin/claude"),
                models: Vec::new(),
                note: Some("no model list from this CLI".to_string()),
            },
            Installed {
                kind: "opencode".into(),
                executable: PathBuf::from("/usr/local/bin/opencode"),
                models: vec![
                    "deepseek/deepseek-flash".into(),
                    "deepseek/deepseek-v4-pro".into(),
                    "minimax-cn/MiniMax-M3".into(),
                    "minimax-cn/MiniMax-M3.1-Flash".into(),
                ],
                note: None,
            },
        ],
        missing: vec!["codex".into()],
    }
}

#[test]
fn opencode_models_are_the_ids_as_printed() {
    let models = kickoff::parse_opencode_models(OPENCODE_MODELS);

    assert_eq!(
        models,
        vec![
            "deepseek/deepseek-flash",
            "deepseek/deepseek-v4-pro",
            "minimax-cn/MiniMax-M3",
            "minimax-cn/MiniMax-M3.1-Flash",
            "opencode-go/deepseek-v4.1-flash",
            "opencode-go/glm-5.3-flash",
        ],
        "blank lines, comments and repeats are dropped"
    );
}

#[test]
fn codex_models_are_the_slugs() {
    let models = kickoff::parse_codex_models(CODEX_CACHE).unwrap();

    assert_eq!(models, vec!["gpt-6.1-sol", "gpt-6-astra"]);
}

#[test]
fn a_broken_codex_cache_is_bad_format() {
    for text in ["not json", "{\"nope\": 1}", "[]"] {
        let err = kickoff::parse_codex_models(text).unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{text}");
    }
}

#[test]
fn the_report_names_the_agents_their_models_and_what_is_missing() {
    let text = kickoff::report(&inventory());

    assert!(text.contains("claude"), "{text}");
    assert!(text.contains("/usr/local/bin/opencode"), "{text}");
    assert!(text.contains("4 models"), "{text}");
    assert!(text.contains("no model list from this CLI"), "{text}");
    assert!(text.contains("Not on PATH: codex"), "{text}");
}

#[test]
fn the_skeleton_is_a_config_the_parser_reads() {
    let text = kickoff::skeleton(&inventory());
    let config = gear_app::config::parse(&text).expect("the skeleton is valid gear.toml");

    // The kinds the machine offers, the captain first.
    assert_eq!(config.captain.kind, "claude");
    assert_eq!(config.lanes[0].kind, "opencode");
    assert_eq!(config.temps.as_ref().unwrap().kind, "opencode");
    // The quota and the roles the packet asks for.
    assert_eq!(config.quota.lanes, 3);
    assert_eq!(config.quota.temps, 2);
    // Lanes run fast models; review runs stronger ones, never the writers'.
    let lanes: Vec<&str> = config.models.lanes.iter().map(|entry| entry.model()).collect();
    assert_eq!(
        lanes,
        vec!["deepseek/deepseek-flash", "minimax-cn/MiniMax-M3.1-Flash"]
    );
    let review: Vec<&str> = config.models.review.iter().map(|entry| entry.model()).collect();
    assert_eq!(
        review,
        vec!["deepseek/deepseek-v4-pro", "minimax-cn/MiniMax-M3"]
    );
    assert_eq!(config.models.quota_reset_h, 5);
    assert_eq!(
        config.models.on_quota.lanes,
        gear_app::config::QuotaPolicy::Fallback
    );
}

#[test]
fn an_empty_machine_still_prints_a_skeleton_to_fill_in() {
    let text = kickoff::skeleton(&Inventory::default());
    let config = gear_app::config::parse(&text).expect("valid gear.toml");

    assert_eq!(config.captain.kind, "TODO");
    assert_eq!(config.models.lanes.len(), 1);
    assert_eq!(config.models.lanes[0].model(), "TODO");

    let report = kickoff::report(&Inventory::default());
    assert!(report.contains("none — install an agent Herdr can start"), "{report}");
    assert!(report.contains("Not on PATH: none"), "{report}");
}

#[test]
fn the_json_carries_the_inventory_and_the_skeleton() {
    let text = kickoff::to_json(&inventory()).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(doc["installed"][1]["kind"], "opencode");
    assert_eq!(doc["installed"][1]["models"][0], "deepseek/deepseek-flash");
    assert_eq!(doc["installed"][0]["note"], "no model list from this CLI");
    assert_eq!(doc["missing"][0], "codex");
    assert!(doc["skeleton"].as_str().unwrap().contains("[models]"));
}
