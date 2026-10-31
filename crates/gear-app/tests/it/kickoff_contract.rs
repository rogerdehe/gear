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

/// A captured `pi --list-models` (pi 1.4.0): a table whose first two columns
/// are the provider and the model.
const PI_MODELS: &str = "\
provider     model                       context  max-out  thinking  images
minimax-cn   MiniMax-M3.1-Flash          1M       524.3K   yes       yes
opencode-go  deepseek-v4.1-flash         1M       384K     yes       yes
";

/// A captured `omp models --json` (omp 18.12.6), trimmed to what the parser
/// reads: each chat model carries the selector its `--model` takes.
const OMP_MODELS: &str = r#"{"models":[
  {"provider":"minimax","kind":"chat","id":"MiniMax-M2","selector":"minimax/MiniMax-M2"},
  {"provider":"anthropic","kind":"chat","id":"claude-sonnet-4-5","selector":"anthropic/claude-sonnet-4-5"},
  {"provider":"minimax","kind":"chat","id":"MiniMax-M2","selector":"minimax/MiniMax-M2"}
]}"#;

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
                skill: true,
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
                skill: false,
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

/// T-113: pi lists its models in a table; the id its `--model` takes is
/// `provider/model`.
#[test]
fn pi_models_are_the_provider_and_the_model() {
    let models = kickoff::parse_pi_models(PI_MODELS);

    assert_eq!(
        models,
        vec![
            "minimax-cn/MiniMax-M3.1-Flash",
            "opencode-go/deepseek-v4.1-flash",
        ],
        "the header row is not a model"
    );
    assert!(kickoff::parse_pi_models("").is_empty());
}

/// T-495: omp prints its models as JSON; the id its `--model` takes is the
/// entry's selector.
#[test]
fn omp_models_are_the_selectors() {
    let models = kickoff::parse_omp_models(OMP_MODELS);

    assert_eq!(
        models,
        vec!["minimax/MiniMax-M2", "anthropic/claude-sonnet-4-5"],
        "the duplicate is listed once"
    );
    assert!(kickoff::parse_omp_models("").is_empty());
    assert!(kickoff::parse_omp_models("{\"models\":[]}").is_empty());
    assert!(kickoff::parse_omp_models("not json").is_empty());
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
    // T-83 (trial G06): the report says, per kind, whether the skill is there.
    assert!(text.contains("GEAR skill installed"), "{text}");
    assert!(
        text.contains("GEAR skill MISSING")
            && text.contains("gear admin install-skill --agent opencode"),
        "{text}"
    );
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
    let lanes: Vec<&str> = config
        .models
        .lanes
        .iter()
        .map(|entry| entry.model())
        .collect();
    assert_eq!(
        lanes,
        vec!["deepseek/deepseek-flash", "minimax-cn/MiniMax-M3.1-Flash"]
    );
    let review: Vec<&str> = config
        .models
        .review
        .iter()
        .map(|entry| entry.model())
        .collect();
    assert_eq!(
        review,
        vec!["deepseek/deepseek-v4-pro", "minimax-cn/MiniMax-M3"]
    );
    assert_eq!(config.models.quota_reset_h, 5);
    // Two models per chain: both roles fall back (T-234).
    assert_eq!(
        config.models.on_quota.lanes,
        gear_app::config::QuotaPolicy::Fallback
    );
    assert_eq!(
        config.models.on_quota.review,
        gear_app::config::QuotaPolicy::Fallback
    );
}

/// A machine whose chains hold one model each: OpenCode lists one fast model
/// and one stronger one, so the lanes take the first and the review the
/// second.
fn one_model_per_chain() -> Inventory {
    Inventory {
        installed: vec![Installed {
            kind: "opencode".into(),
            executable: PathBuf::from("/usr/local/bin/opencode"),
            models: vec![
                "deepseek/deepseek-flash".into(),
                "deepseek/deepseek-v4-pro".into(),
            ],
            note: None,
            skill: true,
        }],
        missing: Vec::new(),
    }
}

/// T-234: a role with one model has no next model to fall back to, so the
/// skeleton writes `wait` for it — `gear admin init` used to refuse the
/// skeleton on a single-model machine.
#[test]
fn a_role_with_one_model_waits_instead_of_falling_back() {
    let text = kickoff::skeleton_for(&one_model_per_chain(), None);
    let config = gear_app::config::parse(&text).expect("the skeleton is valid gear.toml");

    assert_eq!(config.models.lanes.len(), 1, "{text}");
    assert_eq!(config.models.review.len(), 1, "{text}");
    assert_eq!(
        config.models.on_quota.lanes,
        gear_app::config::QuotaPolicy::Wait
    );
    assert_eq!(
        config.models.on_quota.review,
        gear_app::config::QuotaPolicy::Wait
    );
    assert!(
        text.contains("on_quota = { lanes = \"wait\", review = \"wait\" }"),
        "{text}"
    );

    // The skeleton is what the user registers: the quota policy must not be
    // one of the issues `gear admin init` reports back.
    let issues: Vec<String> = gear_app::config::validate(&config)
        .into_iter()
        .map(|issue| issue.field)
        .collect();
    assert!(
        !issues
            .iter()
            .any(|field| field == "models.lanes" || field == "models.review"),
        "{issues:?}"
    );
}

/// T-325: the skeleton asks the wait policy once, and its default is `keep`:
/// a lane parked on a quota wait keeps its agent unless the user says otherwise.
#[test]
fn the_skeleton_asks_the_wait_policy_and_keeps_the_agent() {
    let text = kickoff::skeleton(&inventory());
    let config = gear_app::config::parse(&text).expect("the skeleton is valid gear.toml");

    assert!(text.contains("on_wait = \"keep\""), "{text}");
    assert_eq!(config.models.on_wait, gear_app::config::WaitPolicy::Keep);
    // Right after the quota policy, so the two are read together.
    let lines: Vec<&str> = text.lines().collect();
    let quota = lines
        .iter()
        .position(|l| l.starts_with("on_quota = "))
        .unwrap();
    assert!(
        lines[quota + 1].starts_with("on_wait = \"keep\""),
        "{}",
        lines[quota + 1]
    );
}

/// T-42: the skeleton's header tells the user how to register the config:
/// the `gear/` path the README and the captain guide use, and no `GEAR_DB`
/// (the store has a default).
#[test]
fn the_skeleton_header_names_the_config_of_the_project() {
    let text = kickoff::skeleton(&inventory());
    let header: Vec<&str> = text.lines().take(2).collect();
    let header = header.join("\n");

    assert!(
        header.contains("gear admin init gear/gear.toml"),
        "{header}"
    );
    assert!(!header.contains("GEAR_DB"), "{header}");
    // The first line still starts the extraction the README does
    // (`sed -n '/^# gear.toml/,$p'`).
    assert!(
        text.lines().next().unwrap().starts_with("# gear.toml"),
        "{text}"
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
    assert!(
        report.contains("none — install an agent Herdr can start"),
        "{report}"
    );
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

// ---------------------------------------------------------------- the repos

/// A git repository in a directory, on the branch `branch`.
fn repo_at(dir: &std::path::Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "kickoff")
            .env("GIT_AUTHOR_EMAIL", "kickoff@example.com")
            .env("GIT_COMMITTER_NAME", "kickoff")
            .env("GIT_COMMITTER_EMAIL", "kickoff@example.com")
            .output()
            .unwrap_or_else(|e| panic!("cannot run git: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "--initial-branch=main"]);
    // A commit, so the branch has a name to read — never the four processes
    // (`add`, `commit`, `switch`) this used to spend on it (T-128).
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    if branch != "main" {
        git(&["switch", "-q", "-c", branch]);
    }
}

/// T-54: the skeleton lists the repositories below the project's root, with
/// the name of their directory, their path and the branch they are on.
#[test]
fn the_skeleton_lists_the_repositories_below_the_root() {
    let root = tempfile::tempdir().unwrap();
    repo_at(&root.path().join("crates/api"), "dev");
    repo_at(&root.path().join("apps/web"), "main");

    let text = kickoff::skeleton_for(&inventory(), Some(root.path()));

    // The repositories are proposals: the captain keeps the ones the voyage
    // changes.
    assert!(text.contains("# [[repos]]"), "{text}");
    assert!(text.contains("# name = \"api\""), "{text}");
    assert!(text.contains("# path = \"crates/api\""), "{text}");
    assert!(text.contains("# integration = \"dev\""), "{text}");
    assert!(text.contains("# name = \"web\""), "{text}");
    assert!(text.contains("# path = \"apps/web\""), "{text}");
    assert!(text.contains("# integration = \"main\""), "{text}");
    // The project itself is the root, not one of its repositories.
    assert!(!text.contains("# name = \"root\""), "{text}");
    // The skeleton is still a config the parser reads.
    let config = gear_app::config::parse(&text).expect("the skeleton is valid gear.toml");
    assert!(
        config.repos.is_empty(),
        "the repositories are proposals only"
    );
    assert_eq!(config.project.repo, root.path().display().to_string());
}

/// A directory that is not a repository is not listed, and neither is a build
/// directory or a dependency directory.
#[test]
fn only_the_directories_holding_a_git_directory_are_listed() {
    let root = tempfile::tempdir().unwrap();
    // A plain directory.
    std::fs::create_dir_all(root.path().join("crates/api")).unwrap();
    // A build directory and a dependency directory that hold a repository.
    repo_at(&root.path().join("target/build/api"), "dev");
    repo_at(&root.path().join("node_modules/pkg/web"), "dev");

    let repos = kickoff::nested_repos(root.path());
    assert_eq!(repos, vec![], "{repos:?}");

    // A repository one level deeper is listed, with its path below the root.
    repo_at(&root.path().join("crates/api"), "dev");
    let repos = kickoff::nested_repos(root.path());
    assert_eq!(repos.len(), 1, "{repos:?}");
    assert_eq!(repos[0].name, "api");
    assert_eq!(repos[0].path, "crates/api");
    assert_eq!(repos[0].integration, "dev");
}

/// The repositories are looked for at most three levels below the root.
#[test]
fn a_repository_deeper_than_three_levels_is_not_listed() {
    let root = tempfile::tempdir().unwrap();
    repo_at(&root.path().join("a/b/c/too-deep"), "dev");
    assert_eq!(kickoff::nested_repos(root.path()), vec![]);

    repo_at(&root.path().join("a/b/c"), "dev");
    let repos = kickoff::nested_repos(root.path());
    assert_eq!(repos.len(), 1, "{repos:?}");
    assert_eq!(repos[0].path, "a/b/c");
}

/// T-83 (trial G03): an empty list that says nothing is "not loaded yet",
/// never "no models".
#[test]
fn an_empty_model_list_is_not_loaded_yet() {
    let inventory = Inventory {
        installed: vec![Installed {
            kind: "opencode".into(),
            executable: PathBuf::from("/usr/local/bin/opencode"),
            models: Vec::new(),
            note: None,
            skill: true,
        }],
        missing: vec![],
    };
    let text = kickoff::report(&inventory);
    assert!(text.contains("model list not loaded yet"), "{text}");
    assert!(!text.contains("no models"), "{text}");
}
