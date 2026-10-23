//! Contract for gear_app::config (T-7). Pure: no store, no Herdr.

use gear_app::config::{parse, validate, ConfigIssue, Conventions, Models, QuotaPolicy, Timing};
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
    assert_eq!(cfg.timing, Timing { tick_s: 4, cooldown_s: 90, sample_min: 20, review_timer_min: 45 });
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

// ---- the two forms of a chain entry (T-35)

/// T-35: a chain entry may name the CLI that runs the model and the
/// arguments that CLI starts with.
#[test]
fn a_model_entry_names_its_cli_and_its_arguments() {
    let text = format!(
        "{MINIMAL}\n[models]\nlanes = [\"minimax-cn/MiniMax-M3.1-Flash\", \
         {{ model = \"gpt-6.1-sol\", cli = \"codex\", args = [\"-c\", \"model_reasoning_effort=medium\"] }}]\n"
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
