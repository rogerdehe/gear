//! Contract for gear_app::config (T-7). Pure: no store, no Herdr.

use gear_app::config::{
    parse, validate, ConfigIssue, Conventions, Models, QuotaPolicy, Repo, Timing,
};
use gear_model::ErrorCode;

const MINIMAL: &str = r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/", "tests/heat/"]
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "pricing"
duty = "price engine"
owns = ["src/pricing/"]
not = ["UI"]
kind = "opencode"
profile = "opencode"
"#;

fn fields(issues: &[ConfigIssue]) -> Vec<&str> {
    issues.iter().map(|i| i.field.as_str()).collect()
}

#[test]
fn minimal_config_parses_with_defaults_and_is_valid() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.project.id, "harbor");
    assert_eq!(cfg.lanes.len(), 2);
    assert_eq!(cfg.lanes[1].not, vec!["UI".to_string()]);
    assert!(!cfg.quota.includes_captain);
    assert_eq!(cfg.temps, None);
    assert_eq!(
        cfg.conventions,
        // T-12 added `gate`; the rest of the defaults are unchanged.
        Conventions {
            integration: "dev".into(),
            release: "main".into(),
            staging: "".into(),
            review_max_rounds: 3,
            gate: "cargo build --workspace --all-targets && cargo test --workspace".into(),
        }
    );
    assert_eq!(cfg.timing, Timing { tick_s: 4, cooldown_s: 90, sample_min: 20, review_timer_min: 45, captain_item_stale_min: 15, lane_busy_idle_min: 10, delivery_grace_s: 180, handin_base_window_min: 30 });
    assert!(cfg.profiles.is_empty());
    assert_eq!(validate(&cfg), vec![]);
}

#[test]
fn partial_sections_keep_other_defaults() {
    let text = format!("{MINIMAL}\n[conventions]\nstaging = \"staging\"\n\n[timing]\ncooldown_s = 120\n\n[profiles.opencode]\nbusy_markers = [\"esc interrupt\"]\n");
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.conventions.integration, "dev");
    assert_eq!(cfg.conventions.staging, "staging");
    assert_eq!(cfg.timing.cooldown_s, 120);
    assert_eq!(cfg.timing.tick_s, 4);
    assert!(cfg.profiles["opencode"].contains_key("busy_markers"));
    assert_eq!(validate(&cfg), vec![]);
}

#[test]
fn syntax_unknown_keys_and_missing_fields_are_bad_format() {
    for bad in [
        "this is not toml =".to_string(),
        format!("{MINIMAL}\n[project2]\nx = 1\n"),
        MINIMAL.replace("duty = \"price engine\"\n", ""),
        MINIMAL.replace("lanes = 2", "lanes = \"two\""),
    ] {
        let err = parse(&bad).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{bad}");
        assert!(!err.detail.is_empty());
    }
}

#[test]
fn names_and_agent_name_length() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].name = "Heat".into();
    cfg.lanes[1].name = "a-very-long-lane-name-for-harbor".into(); // harbor- + 32 > 32
    let issues = validate(&cfg);
    assert_eq!(fields(&issues), vec!["lanes[0].name", "lanes[1].name"]);

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[1].name = "heat".into();
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[1].name"], "duplicate lane name");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.project.id = "9harbor".into();
    assert_eq!(fields(&validate(&cfg)), vec!["project.id"]);
}

#[test]
fn duty_and_owns_are_required() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].duty = "   ".into();
    cfg.lanes[1].owns = vec![];
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[0].duty", "lanes[1].owns"]);
}

#[test]
fn owns_overlap_on_path_boundaries_only() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[1].owns = vec!["src/heat/model.rs".into()];
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[1].owns"], "prefix on / boundary overlaps");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].owns = vec!["src/a".into()];
    cfg.lanes[1].owns = vec!["src/ab".into()];
    assert_eq!(validate(&cfg), vec![], "src/a and src/ab do not overlap");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[1].owns = vec!["tests/heat/".into()];
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[1].owns"], "equal paths overlap");
}

#[test]
fn quota_conventions_and_timing() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.quota.lanes = 1;
    cfg.conventions.release = "dev".into();
    cfg.timing.cooldown_s = 4;
    assert_eq!(fields(&validate(&cfg)), vec!["quota.lanes", "conventions.release", "timing.cooldown_s"]);

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.conventions.staging = "main".into();
    assert_eq!(fields(&validate(&cfg)), vec!["conventions.staging"]);
}

/// T-11: values are compared after trimming; an entry that is empty after
/// trimming is reported where it is (owns entry or branch name).
#[test]
fn whitespace_does_not_hide_overlaps_or_empty_values() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[1].owns = vec![" src/heat/ ".into()];
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[1].owns"], "trimmed owns overlap");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].owns = vec!["src/heat/".into(), "   ".into()];
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[0].owns"], "blank owns entry");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.conventions.integration = "dev ".into();
    cfg.conventions.release = "dev".into();
    assert_eq!(fields(&validate(&cfg)), vec!["conventions.release"], "trimmed branch names compared");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.conventions.staging = "  ".into();
    assert_eq!(validate(&cfg), vec![], "blank staging means no staging");
}

// ---- [models] (17 §0, design/01)

const MODELS: &str = r#"
[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash", "deepseek/deepseek-v4.1-flash"]
review = ["deepseek/deepseek-v4.1-flash", "deepseek/deepseek-v4-pro"]
on_quota = { lanes = "wait" }
quota_reset_h = 4
"#;

#[test]
fn models_parse_with_defaults() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.models, Models::default());
    assert_eq!(cfg.models.quota_reset_h, 5);
    let cfg = parse(&format!("{MINIMAL}{MODELS}")).unwrap();
    assert_eq!(cfg.models.lanes[0].model(), "minimax-cn/MiniMax-M3.1-Flash");
    assert_eq!(cfg.models.on_quota.lanes, QuotaPolicy::Wait);
    assert_eq!(cfg.models.on_quota.review, QuotaPolicy::Fallback);
    assert_eq!(cfg.models.quota_reset_h, 4);
    assert!(cfg.lanes[0].models.is_empty());
    assert_eq!(parse(&format!("{MINIMAL}\n[models]\non_quota = {{ lanes = \"later\" }}\n")).unwrap_err().code, ErrorCode::BadFormat);
}

#[test]
fn models_rules() {
    // L-74: fallback with no next model is refused (it would only park)
    let single = MODELS.replace(r#"review = ["deepseek/deepseek-v4.1-flash", "deepseek/deepseek-v4-pro"]"#, r#"review = ["deepseek/deepseek-v4.1-flash"]"#);
    assert_eq!(fields(&validate(&parse(&format!("{MINIMAL}{single}")).unwrap())), vec!["models.review"]);
    // valid as given
    assert!(validate(&parse(&format!("{MINIMAL}{MODELS}")).unwrap()).is_empty());
    // review must hold a model different from every lane's primary
    let same = MODELS.replace(r#"review = ["deepseek/deepseek-v4.1-flash", "deepseek/deepseek-v4-pro"]"#, r#"review = ["minimax-cn/MiniMax-M3.1-Flash"]"#);
    // (a one-model review chain with fallback is also refused: L-74)
    assert_eq!(fields(&validate(&parse(&format!("{MINIMAL}{same}")).unwrap())), vec!["models.review", "models.review"]);
    // no blank or repeated entries in a chain (after trim)
    let blank = MODELS.replace(r#""deepseek/deepseek-v4-pro""#, r#""  ""#);
    assert_eq!(fields(&validate(&parse(&format!("{MINIMAL}{blank}")).unwrap())), vec!["models.review"]);
    let dup = MODELS.replace(r#""deepseek/deepseek-v4-pro""#, r#""deepseek/deepseek-v4.1-flash ""#);
    assert_eq!(fields(&validate(&parse(&format!("{MINIMAL}{dup}")).unwrap())), vec!["models.review"]);
    // wait needs a window
    let zero = MODELS.replace("quota_reset_h = 4", "quota_reset_h = 0");
    assert_eq!(fields(&validate(&parse(&format!("{MINIMAL}{zero}")).unwrap())), vec!["models.quota_reset_h"]);
    // empty chains = models not managed by GEAR: valid
    assert!(validate(&parse(MINIMAL).unwrap()).is_empty());
}

#[test]
fn captain_stale_period_is_configurable_and_nonzero() {
    let cfg = parse(&format!("{MINIMAL}\n[timing]\ncaptain_item_stale_min = 30\n")).unwrap();
    assert_eq!(cfg.timing.captain_item_stale_min, 30);
    assert!(validate(&cfg).is_empty());
    let mut cfg = cfg;
    cfg.timing.captain_item_stale_min = 0;
    assert_eq!(fields(&validate(&cfg)), vec!["timing.captain_item_stale_min"]);
}

// ---- the two forms of a chain entry (T-35)

/// T-35: a chain entry may name the CLI that runs the model and the
/// arguments that CLI starts with.
#[test]
fn a_model_entry_names_its_cli_and_its_arguments() {
    let text = format!(
        "{MINIMAL}\n[models]\nlanes = [\"minimax-cn/MiniMax-M3.1-Flash\", \
         {{ model = \"gpt-6.1-sol\", cli = \"codex\", args = [\"-c\", \"model_reasoning_effort=medium\"], context_window_k = 500 }}]\n"
    );
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.models.lanes.len(), 2);
    // The plain form: the model id alone, the seat's own CLI runs it.
    assert_eq!(cfg.models.lanes[0].model(), "minimax-cn/MiniMax-M3.1-Flash");
    assert_eq!(cfg.models.lanes[0].cli(), None);
    assert!(cfg.models.lanes[0].args().is_empty());
    // The entry form: the model, the CLI and the arguments.
    assert_eq!(cfg.models.lanes[1].model(), "gpt-6.1-sol");
    assert_eq!(cfg.models.lanes[1].cli(), Some("codex"));
    assert_eq!(cfg.models.lanes[1].args(), ["-c", "model_reasoning_effort=medium"]);
    assert_eq!(cfg.models.lanes[1].context_window_k(), Some(500));
    assert_eq!(cfg.models.lanes[0].context_window_k(), None);
    // It is valid: the review chain still holds a model no lane runs.
    assert!(validate(&cfg).is_empty());
}

/// The old plain form still parses: a chain of model ids.
#[test]
fn the_plain_model_chain_still_parses() {
    let cfg = parse(&format!("{MINIMAL}{MODELS}")).unwrap();
    assert_eq!(cfg.models.lanes.len(), 2);
    assert_eq!(cfg.models.lanes[0].model(), "minimax-cn/MiniMax-M3.1-Flash");
    assert_eq!(cfg.models.lanes[1].model(), "deepseek/deepseek-v4.1-flash");
    assert!(cfg.models.lanes.iter().all(|entry| entry.cli().is_none()));
    assert!(cfg.models.lanes.iter().all(|entry| entry.args().is_empty()));
    // A lane's own chain is a chain of entries too.
    let own = MINIMAL.replace(
        "profile = \"opencode\"\n\n[[lanes]]",
        "profile = \"opencode\"\nmodels = [{ model = \"gpt-6.1-sol\", cli = \"codex\" }]\n\n[[lanes]]",
    );
    let cfg = parse(&own).unwrap();
    assert_eq!(cfg.lanes[0].models.len(), 1);
    assert_eq!(cfg.lanes[0].models[0].cli(), Some("codex"));
}

/// T-235: the claude CLI takes a bare model id or alias
/// (`claude-sonnet-5-5`), never `provider/model` — it refuses the prefixed
/// one, which used to cost a review round to find out at runtime.
#[test]
fn a_claude_seat_gets_a_bare_model_id() {
    let prefixed = format!(
        "{MINIMAL}\n[temps]\nkind = \"claude\"\nprofile = \"claude\"\n\n[models]\nreview = [\"anthropic/claude-sonnet-5-5\"]\non_quota = {{ review = \"wait\" }}\n"
    );
    let issues = validate(&parse(&prefixed).unwrap());
    assert_eq!(fields(&issues), vec!["models.review"]);
    assert_eq!(
        issues[0].problem,
        "the claude CLI takes a bare model id such as claude-sonnet-5-5, not anthropic/claude-sonnet-5-5"
    );

    // The bare id is what the CLI wants.
    let bare = prefixed.replace("anthropic/claude-sonnet-5-5", "claude-sonnet-5-5");
    assert!(validate(&parse(&bare).unwrap()).is_empty());

    // Another CLI keeps its `provider/model` ids: a pi lane is no claude seat.
    let pi_lane = MINIMAL.replace(
        "kind = \"opencode\"\nprofile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
        "kind = \"pi\"\nprofile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
    ) + "\n[models]\nlanes = [\"minimax-cn/MiniMax-M3.1-Flash\"]\non_quota = { lanes = \"wait\" }\n";
    let cfg = parse(&pi_lane).unwrap();
    assert_eq!(cfg.lanes[0].kind, "pi");
    assert_eq!(validate(&cfg), vec![]);
}

/// T-235: an entry's CLI is the one it names, else the kind of the seat that
/// runs it — a lane's own kind, every lane for `models.lanes`, the captain's.
#[test]
fn the_claude_rule_reads_an_entrys_cli_else_the_seats_kind() {
    // An entry that names claude is checked against claude, whoever runs it.
    let own_cli = format!(
        "{MINIMAL}\n[models]\nlanes = [\"gpt-6.1-sol\", {{ model = \"anthropic/claude-sonnet-5-5\", cli = \"claude\" }}]\n"
    );
    assert_eq!(fields(&validate(&parse(&own_cli).unwrap())), vec!["models.lanes"]);

    // `models.lanes` is shared by every lane: a claude lane is enough.
    let claude_lane = MINIMAL.replace(
        "kind = \"opencode\"\nprofile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
        "kind = \"claude\"\nprofile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
    );
    let shared = format!(
        "{claude_lane}\n[models]\nlanes = [\"anthropic/claude-sonnet-5-5\", \"claude-sonnet-5-5\"]\n"
    );
    assert_eq!(fields(&validate(&parse(&shared).unwrap())), vec!["models.lanes"]);

    // A lane's own chain is checked against that lane's kind.
    let lane_own = claude_lane.replace(
        "profile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
        "profile = \"opencode\"\nmodels = [\"anthropic/claude-sonnet-5-5\"]\n\n[[lanes]]\nname = \"pricing\"",
    );
    assert_eq!(fields(&validate(&parse(&lane_own).unwrap())), vec!["lanes[0].models"]);

    // The captain's chain follows the captain's kind, not a lane's.
    let captain = format!("{MINIMAL}\n[models]\ncaptain = [\"anthropic/claude-sonnet-5-5\"]\n");
    assert_eq!(validate(&parse(&captain).unwrap()), vec![], "the captain is a codex");
    let claude_captain = captain.replace("kind = \"codex\"", "kind = \"claude\"");
    assert_eq!(fields(&validate(&parse(&claude_captain).unwrap())), vec!["models.captain"]);
}

/// A broken entry is a bad format, and the detail says where.
#[test]
fn a_broken_model_entry_is_bad_format() {
    for bad in [
        // no model
        "{ cli = \"codex\" }",
        // an unknown field
        "{ model = \"gpt-6.1-sol\", cli = \"codex\", effort = \"high\" }",
        // a wrong type
        "{ model = 7 }",
        // not an entry at all
        "7",
    ] {
        let text = format!("{MINIMAL}\n[models]\nlanes = [{bad}]\n");
        let err = parse(&text).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadFormat, "{bad}");
        assert!(!err.detail.is_empty(), "{bad}");
    }
}

#[test]
fn busy_idle_period_is_configurable_and_nonzero() {
    let cfg = parse(&format!("{MINIMAL}\n[timing]\nlane_busy_idle_min = 5\n")).unwrap();
    assert_eq!(cfg.timing.lane_busy_idle_min, 5);
    assert!(validate(&cfg).is_empty());
    let mut cfg = cfg;
    cfg.timing.lane_busy_idle_min = 0;
    assert_eq!(fields(&validate(&cfg)), vec!["timing.lane_busy_idle_min"]);
}

// ---- [[repos]]: a project made of several git repositories (T-52)

/// A project with three lanes: one in the `api` repo, one in the `web` repo
/// and one in the project root.
const MULTI: &str = r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 3
temps = 1

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
repo = "api"

[[lanes]]
name = "web"
duty = "the web app"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
repo = "web"

[[lanes]]
name = "pricing"
duty = "price engine"
owns = ["lib/pricing/"]
kind = "opencode"
profile = "opencode"
"#;

/// The project's repos. The `api` repo declares no branch and no gate: it
/// uses the project's `[conventions]`.
const REPOS: &str = r#"
[[repos]]
name = "api"
path = "crates/api"

[[repos]]
name = "web"
path = "apps/web"
integration = "feat/web"
release = "main"
gate = "npm test"
"#;

/// A config without `[[repos]]` is a single-repo project: it behaves exactly
/// as before.
#[test]
fn a_project_without_repos_has_only_the_root_repo() {
    let cfg = parse(MINIMAL).unwrap();
    assert!(cfg.repos.is_empty());
    assert!(cfg.lanes.iter().all(|lane| lane.repo.is_none()));
    assert_eq!(
        cfg.root_repo(),
        Repo {
            name: "".into(),
            path: "/repo".into(),
            integration: "dev".into(),
            release: "main".into(),
            gate: "cargo build --workspace --all-targets && cargo test --workspace".into(),
            github: None,
        }
    );
    assert_eq!(validate(&cfg), vec![]);
}

/// A repo without branches or a gate uses the project's `[conventions]`.
#[test]
fn a_repo_without_its_own_branches_uses_the_conventions() {
    let text = format!(
        "{MULTI}{REPOS}\n[conventions]\nintegration = \"feat/dev\"\nrelease = \"feat/main\"\ngate = \"cargo test\"\n"
    );
    let cfg = parse(&text).unwrap();
    let api = cfg.repo_of_lane("heat");
    assert_eq!(api.name, "api");
    assert_eq!(api.integration, "feat/dev");
    assert_eq!(api.release, "feat/main");
    assert_eq!(api.gate, "cargo test");
    // The `web` repo declares its own.
    let web = cfg.repo_of_lane("web");
    assert_eq!(web.integration, "feat/web");
    assert_eq!(web.release, "main");
    assert_eq!(web.gate, "npm test");
    // The root repo keeps the project's.
    assert_eq!(cfg.root_repo().integration, "feat/dev");
    assert_eq!(cfg.root_repo().release, "feat/main");
    assert_eq!(cfg.root_repo().gate, "cargo test");
}

/// A repo's path is relative to `project.repo`: the resolved path is the
/// project's own path joined with it.
#[test]
fn a_repo_path_is_joined_to_the_project_repo() {
    let cfg = parse(&format!("{MULTI}{REPOS}")).unwrap();
    assert_eq!(cfg.repo_of_lane("heat").path, "/repo/crates/api");
    assert_eq!(cfg.repo_of_lane("web").path, "/repo/apps/web");
    // A trailing slash on the project's path is the same path.
    let text = MULTI.replace(r#"repo = "/repo""#, r#"repo = "/repo/""#);
    let cfg = parse(&format!("{text}{REPOS}")).unwrap();
    assert_eq!(cfg.repo_of_lane("heat").path, "/repo/crates/api");
}

/// A lane without a `repo`, and a lane that is not in the config at all, work
/// in the project root.
#[test]
fn a_lane_without_a_repo_works_in_the_project_root() {
    let cfg = parse(&format!("{MULTI}{REPOS}")).unwrap();
    let root = cfg.root_repo();
    assert_eq!(cfg.repo_of_lane("pricing"), root, "a lane without a repo");
    assert_eq!(cfg.repo_of_lane("nobody"), root, "a lane that is not in the config");
}

#[test]
fn repo_names_are_unique_and_well_formed() {
    // The lanes of this config are not bound to a repo, so only the repo's
    // own finding is expected.
    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[1].name = "api".into();
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["repos[1].name"],
        "the repo name is used twice"
    );

    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].name = "Api".into();
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["repos[0].name"],
        "the repo name is not [a-z][a-z0-9-]*"
    );
}

#[test]
fn a_repo_path_is_relative_to_the_project_repo() {
    for (path, why) in [
        ("/elsewhere", "absolute"),
        ("../elsewhere", "leaves the project"),
        ("crates/../elsewhere", "leaves the project"),
        ("", "empty"),
    ] {
        let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
        cfg.repos[0].path = path.into();
        assert_eq!(fields(&validate(&cfg)), vec!["repos[0].path"], "{why}");
    }
}

#[test]
fn a_repo_gate_is_never_left_as_a_todo() {
    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].gate = Some("TODO: the build command".into());
    assert_eq!(fields(&validate(&cfg)), vec!["repos[0].gate"]);

    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].gate = Some("   ".into());
    assert_eq!(fields(&validate(&cfg)), vec!["repos[0].gate"]);
}

/// T-242: a pipeline's exit status is its last command's, so a gate that
/// pipes into another command can pass while the build failed. `||` is not a
/// pipe, and `pipefail` makes the pipeline report the failing stage.
#[test]
fn a_gate_may_not_pipe_away_a_failure() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.conventions.gate =
        "go build ./... && go vet ./internal/... 2>&1 | head -20".into();
    assert_eq!(fields(&validate(&cfg)), vec!["conventions.gate"]);

    for gate in [
        "make gate",
        "a && b",
        "a || b",
        "set -o pipefail; a | b",
    ] {
        let mut cfg = parse(MINIMAL).unwrap();
        cfg.conventions.gate = gate.into();
        assert_eq!(validate(&cfg), vec![], "{gate}");
    }

    // The repo's own gate is checked the same way, under its own name.
    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].gate = Some("cargo test 2>&1 | tail -5".into());
    assert_eq!(fields(&validate(&cfg)), vec!["repos.api.gate"]);

    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].gate = Some("set -o pipefail; cargo test 2>&1 | tail -5".into());
    assert_eq!(validate(&cfg), vec![], "pipefail is enough");
}

#[test]
fn a_repo_branches_are_non_empty_and_different() {
    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].integration = Some("  ".into());
    assert_eq!(fields(&validate(&cfg)), vec!["repos[0].integration"]);

    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].release = Some("".into());
    assert_eq!(fields(&validate(&cfg)), vec!["repos[0].release"]);

    // The repo's own integration branch is the branch it releases from.
    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].integration = Some("main".into());
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["repos[0].release"],
        "the repo's release branch is the project's main"
    );

    // The repo's own release branch is its integration branch.
    let mut cfg = parse(&format!("{MINIMAL}{REPOS}")).unwrap();
    cfg.repos[0].release = Some("dev".into());
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["repos[0].release"],
        "the repo releases into its own integration branch"
    );
}

/// A lane's `repo` must name one of the project's repos.
#[test]
fn a_lane_binds_to_a_repo_that_exists() {
    let mut cfg = parse(&format!("{MULTI}{REPOS}")).unwrap();
    cfg.lanes[0].repo = Some("nope".into());
    assert_eq!(fields(&validate(&cfg)), vec!["lanes[0].repo"]);

    let mut cfg = parse(&format!("{MULTI}{REPOS}")).unwrap();
    cfg.lanes[0].repo = None;
    assert_eq!(validate(&cfg), vec![], "a lane without a repo works in the root");
}

/// `owns` is relative to the lane's repo: the same paths in two repos are
/// two different files, and only lanes of the same repo may not overlap.
#[test]
fn owns_overlap_is_checked_between_lanes_of_the_same_repo() {
    let cfg = parse(&format!("{MULTI}{REPOS}")).unwrap();
    assert_eq!(
        validate(&cfg),
        vec![],
        "the same paths in two repos are two different files"
    );

    let mut cfg = parse(&format!("{MULTI}{REPOS}")).unwrap();
    cfg.lanes[1].repo = Some("api".into());
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[1].owns"],
        "two lanes of one repo own the same paths"
    );
}

// ---- [[quota.sources]] (design §9.1)

const SOURCES: &str = r#"
[[quota.sources]]
provider = "minimax-cn"
reader = "minimax"
key_file = "~/.pi/agent/auth.json"
key_path = "minimax-cn.key"
"#;

#[test]
fn the_quota_sources_parse() {
    let cfg = parse(MINIMAL).unwrap();
    assert!(cfg.quota.sources.is_empty(), "no source without the section");
    let cfg = parse(&format!("{MINIMAL}{SOURCES}")).unwrap();
    assert_eq!(cfg.quota.sources.len(), 1);
    assert_eq!(cfg.quota.sources[0].provider, "minimax-cn");
    assert_eq!(cfg.quota.sources[0].reader, "minimax");
    assert_eq!(cfg.quota.sources[0].key_file, "~/.pi/agent/auth.json");
    assert_eq!(cfg.quota.sources[0].key_path, "minimax-cn.key");
    assert_eq!(validate(&cfg), vec![], "a source the engine knows is valid");
}

#[test]
fn an_unknown_reader_is_a_finding() {
    let text = SOURCES.replace("reader = \"minimax\"", "reader = \"zen\"");
    let cfg = parse(&format!("{MINIMAL}{text}")).unwrap();
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["quota.sources.minimax-cn.reader"]
    );
    let mut cfg = parse(&format!("{MINIMAL}{SOURCES}")).unwrap();
    cfg.quota.sources[0].provider = "  ".into();
    assert_eq!(fields(&validate(&cfg)), vec!["quota.sources.provider"]);
}

#[test]
fn prices_and_delivery_grace_parse_and_validate_with_field_names() {
    let text=format!("{MINIMAL}\n[timing]\ndelivery_grace_s=45\n[prices.\"openai/model\"]\ninput=1.5\noutput=2.0\ncache_read=0.25\ncache_write=0.0\n");
    let cfg=parse(&text).unwrap();
    assert_eq!(cfg.timing.delivery_grace_s,45);
    let price=&cfg.prices["openai/model"];
    assert_eq!((price.input,price.output,price.cache_read,price.cache_write),(1.5,2.0,0.25,0.0));
    assert!(validate(&cfg).is_empty());
    let mut bad=cfg.clone(); bad.timing.delivery_grace_s=0; bad.prices.get_mut("openai/model").unwrap().cache_read=-1.0;
    let issues=validate(&bad); let fields=fields(&issues);
    assert!(fields.contains(&"timing.delivery_grace_s"),"{issues:?}");
    assert!(fields.contains(&"prices.openai/model.cache_read"),"{issues:?}");
    assert_eq!(parse(&text.replace("input=1.5","input=1")).unwrap().prices["openai/model"].input,1.0);
    assert!(parse(&text.replace("output=2.0","output='bad'")).unwrap_err().detail.contains("output"));
}
