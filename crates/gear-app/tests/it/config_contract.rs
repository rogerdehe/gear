//! Contract for gear_app::config (T-7). Pure: no store, no Herdr.

use gear_app::config::{
    departs_at, hours_end, hours_next_start, off_hours_until, parse, validate, ConfigIssue,
    Conventions, Models, QuotaPolicy, Repo, Timing, WaitPolicy,
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

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#;

/// The gate line of [`MINIMAL`]: the tests that name other conventions put
/// their lines next to it, in the same section.
const GATE_LINE: &str = "gate = \"make test\"";

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
        // T-255: `gate` has no default; the fixture names one.
        Conventions {
            integration: "dev".into(),
            release: "main".into(),
            staging: "".into(),
            review_max_rounds: 3,
            max_task_min: 60,
            branch_max_age_min: 120,
            branch_max_per_lane: 3,
            push_interval_min: 20,
            push_reply_min: 10,
            // T-255: `gate` has no default; the fixture names one.
            gate: "make test".into(),
            task_branch: "{lane}/t{n}".into(),
            review_branch: "review/t{n}".into(),
            seat_branch: "{project}/{seat}".into(),
            remote: "origin".into(),
            voyage_dir: "gear/voyages/{milestone}".into(),
            // T-299: the commit messages are configuration too; the fixture
            // names none, so GEAR's own form stands.
            merge_message: "merge({task}): {source}".into(),
            review_message: "review({task}): {what}".into(),
            plan_message: "{milestone}: {what}".into(),
            // T-413: no scratch directory named; the seat's worktree holds it.
            scratch_dir: None,
        }
    );
    assert_eq!(Conventions::default().gate, "", "the gate has no default");
    assert_eq!(
        cfg.timing,
        Timing {
            tick_s: 4,
            cooldown_s: 90,
            sample_min: 20,
            review_timer_min: 45,
            captain_item_stale_min: 15,
            lane_busy_idle_min: 10,
            delivery_grace_s: 180,
            handin_base_window_min: 30
        }
    );
    assert!(cfg.profiles.is_empty());
    assert_eq!(validate(&cfg), vec![]);
}

#[test]
fn partial_sections_keep_other_defaults() {
    let text = format!("{}\n[timing]\ncooldown_s = 120\n\n[profiles.opencode]\nbusy_markers = [\"esc interrupt\"]\n", MINIMAL.replace("gate = \"make test\"", "gate = \"make test\"\nstaging = \"staging\""));
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
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[1].name"],
        "duplicate lane name"
    );

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.project.id = "9harbor".into();
    assert_eq!(fields(&validate(&cfg)), vec!["project.id"]);
}

#[test]
fn duty_and_owns_are_required() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].duty = "   ".into();
    cfg.lanes[1].owns = vec![];
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[0].duty", "lanes[1].owns"]
    );
}

#[test]
fn owns_overlap_on_path_boundaries_only() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[1].owns = vec!["src/heat/model.rs".into()];
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[1].owns"],
        "prefix on / boundary overlaps"
    );

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].owns = vec!["src/a".into()];
    cfg.lanes[1].owns = vec!["src/ab".into()];
    assert_eq!(validate(&cfg), vec![], "src/a and src/ab do not overlap");

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[1].owns = vec!["tests/heat/".into()];
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[1].owns"],
        "equal paths overlap"
    );
}

#[test]
fn quota_conventions_and_timing() {
    // The lane quota is not a config finding (T-304): only the seats know which
    // lanes are retired, so `admin::lane_quota_problem` reports it.
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.quota.lanes = 1;
    cfg.conventions.release = "dev".into();
    cfg.timing.cooldown_s = 4;
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["conventions.release", "timing.cooldown_s"]
    );

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
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[1].owns"],
        "trimmed owns overlap"
    );

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.lanes[0].owns = vec!["src/heat/".into(), "   ".into()];
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["lanes[0].owns"],
        "blank owns entry"
    );

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.conventions.integration = "dev ".into();
    cfg.conventions.release = "dev".into();
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["conventions.release"],
        "trimmed branch names compared"
    );

    let mut cfg = parse(MINIMAL).unwrap();
    cfg.conventions.staging = "  ".into();
    assert_eq!(validate(&cfg), vec![], "blank staging means no staging");
}

// ---- [schedule] (T-448): a voyage departs at an agreed time.

const SCHEDULE: &str = r#"
[schedule]
depart_at = "2026-10-30T14:00:00-05:00"
hours = "09:00-18:00"
"#;

#[test]
fn a_scheduled_departure_parses_and_holds_until_its_time() {
    let cfg = parse(&format!("{MINIMAL}{SCHEDULE}")).unwrap();
    assert_eq!(validate(&cfg), vec![], "a departure and hours are valid");
    let schedule = cfg.schedule.as_ref().expect("the section parses");
    assert_eq!(
        schedule.depart_at.as_deref(),
        Some("2026-10-30T14:00:00-05:00")
    );
    assert_eq!(schedule.hours.as_deref(), Some("09:00-18:00"));
    assert_eq!(schedule.notice_min, 15, "the notice default");
    let at = gear_app::plan::parse_rfc3339("2026-10-30T14:00:00-05:00").unwrap();
    assert_eq!(departs_at(&cfg, at - 1), Some(at), "before the departure");
    assert_eq!(departs_at(&cfg, at), None, "at it");
    assert_eq!(departs_at(&cfg, at + 1), None, "after it");

    let cfg = parse(&format!("{MINIMAL}\n[schedule]\nnotice_min = 5\n")).unwrap();
    assert_eq!(cfg.schedule.as_ref().map(|s| s.notice_min), Some(5));
    assert_eq!(validate(&cfg), vec![]);
    assert_eq!(departs_at(&cfg, 0), None, "no departure time named");
}

#[test]
fn a_config_without_a_schedule_parses_as_today() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.schedule, None, "the section is optional");
    assert_eq!(departs_at(&cfg, 0), None);
    assert_eq!(validate(&cfg), vec![], "today's config stays valid");
}

#[test]
fn a_bad_departure_or_hours_is_an_init_issue() {
    let bad_time = parse(&format!(
        "{MINIMAL}\n[schedule]\ndepart_at = \"tomorrow\"\n"
    ))
    .unwrap();
    assert_eq!(fields(&validate(&bad_time)), vec!["schedule.depart_at"]);
    assert!(!validate(&bad_time)[0].problem.is_empty());

    let same = parse(&format!("{MINIMAL}\n[schedule]\nhours = \"10:00-10:00\"\n")).unwrap();
    assert_eq!(fields(&validate(&same)), vec!["schedule.hours"]);

    for bad in [
        "09:00-24:00",
        "9:00-18:00",
        "09:00",
        "18:00-09:60",
        "09:00-18:00-20:00",
    ] {
        let cfg = parse(&format!("{MINIMAL}\n[schedule]\nhours = {bad:?}\n")).unwrap();
        assert_eq!(fields(&validate(&cfg)), vec!["schedule.hours"], "{bad:?}");
    }
    for good in ["09:00-18:00", "00:00-23:59", "18:00-09:00"] {
        let cfg = parse(&format!("{MINIMAL}\n[schedule]\nhours = {good:?}\n")).unwrap();
        assert_eq!(validate(&cfg), vec![], "{good:?}");
    }
}

/// A local time on 2027-03-10, as the machine's clock sees it (T-450).
fn local(hour: i32, minute: i32) -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = 127;
    tm.tm_mon = 2;
    tm.tm_mday = 10;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_isdst = -1;
    (unsafe { libc::mktime(&mut tm) }) as i64
}

#[test]
fn the_working_hours_say_when_the_fleet_rests_and_comes_back() {
    let cfg = parse(&format!("{MINIMAL}\n[schedule]\nhours = \"10:00-18:00\"\n")).unwrap();
    let end = hours_end(&cfg, local(12, 0)).expect("the end of the working day");
    assert_eq!(gear_board::idle::clock(end), "18:00");
    assert_eq!(
        off_hours_until(&cfg, local(12, 0)),
        None,
        "inside the hours"
    );
    let start = off_hours_until(&cfg, local(18, 0)).expect("the next start");
    assert_eq!(gear_board::idle::clock(start), "10:00");
    assert_eq!(
        hours_next_start(&cfg, local(17, 0)),
        Some(start),
        "the notice names it"
    );
    assert!(start > local(18, 0), "the next start is after the end");

    // An overnight window works from its start to its end on the next day.
    let night = parse(&format!("{MINIMAL}\n[schedule]\nhours = \"18:00-09:00\"\n")).unwrap();
    assert_eq!(
        off_hours_until(&night, local(12, 0)),
        Some(local(18, 0)),
        "resting by day"
    );
    assert_eq!(
        hours_end(&night, local(20, 0))
            .map(gear_board::idle::clock)
            .as_deref(),
        Some("09:00")
    );

    let none = parse(MINIMAL).unwrap();
    assert_eq!(off_hours_until(&none, local(12, 0)), None);
    assert_eq!(hours_end(&none, local(12, 0)), None);
}

// ---- `[conventions].scratch_dir` (T-413): where a seat's temp files go.
// Absent, the seat's own worktree holds them.

#[test]
fn scratch_dir_defaults_to_none_and_parses_a_path() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.conventions.scratch_dir, None, "the default is none");
    assert_eq!(validate(&cfg), vec![], "no scratch_dir is valid");

    let cfg = parse(&MINIMAL.replace(
        GATE_LINE,
        "gate = \"make test\"\nscratch_dir = \"~/gear-scratch\"",
    ))
    .unwrap();
    assert_eq!(
        cfg.conventions.scratch_dir.as_deref(),
        Some("~/gear-scratch")
    );
    assert_eq!(validate(&cfg), vec![], "a scratch_dir is valid");
}

// ---- `[conventions].max_task_min` (T-279): no task is planned longer than
// the project's cap, 60 minutes by default.

#[test]
fn max_task_min_defaults_to_sixty_and_a_project_may_raise_it() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.conventions.max_task_min, 60, "the default cap");
    assert_eq!(validate(&cfg), vec![], "the default is valid");

    let cfg =
        parse(&MINIMAL.replace(GATE_LINE, "gate = \"make test\"\nmax_task_min = 90")).unwrap();
    assert_eq!(cfg.conventions.max_task_min, 90);
    assert_eq!(validate(&cfg), vec![], "a project may plan longer tasks");
}

#[test]
fn a_max_task_min_of_zero_is_refused() {
    // A cap of zero refuses every task: the captain plans each task's time,
    // and no estimate is under it.
    let cfg = parse(&MINIMAL.replace(GATE_LINE, "gate = \"make test\"\nmax_task_min = 0")).unwrap();
    assert_eq!(fields(&validate(&cfg)), vec!["conventions.max_task_min"]);
    assert!(!validate(&cfg)[0].problem.is_empty());
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
    assert_eq!(
        parse(&format!(
            "{MINIMAL}\n[models]\non_quota = {{ lanes = \"later\" }}\n"
        ))
        .unwrap_err()
        .code,
        ErrorCode::BadFormat
    );
}

#[test]
fn wait_policy_parses_and_defaults_to_keep() {
    // T-325: without `on_wait` a parked lane keeps its agent.
    let cfg = parse(&format!("{MINIMAL}{MODELS}")).unwrap();
    assert_eq!(cfg.models.on_wait, WaitPolicy::Keep);
    let cfg = parse(&format!("{MINIMAL}\n[models]\non_wait = \"release\"\n")).unwrap();
    assert_eq!(cfg.models.on_wait, WaitPolicy::Release);
    assert_eq!(
        parse(&format!("{MINIMAL}\n[models]\non_wait = \"sometimes\"\n"))
            .unwrap_err()
            .code,
        ErrorCode::BadFormat
    );
}

#[test]
fn models_rules() {
    // L-74: fallback with no next model is refused (it would only park)
    let single = MODELS.replace(
        r#"review = ["deepseek/deepseek-v4.1-flash", "deepseek/deepseek-v4-pro"]"#,
        r#"review = ["deepseek/deepseek-v4.1-flash"]"#,
    );
    assert_eq!(
        fields(&validate(&parse(&format!("{MINIMAL}{single}")).unwrap())),
        vec!["models.review"]
    );
    // valid as given
    assert!(validate(&parse(&format!("{MINIMAL}{MODELS}")).unwrap()).is_empty());
    // review must hold a model different from every lane's primary
    let same = MODELS.replace(
        r#"review = ["deepseek/deepseek-v4.1-flash", "deepseek/deepseek-v4-pro"]"#,
        r#"review = ["minimax-cn/MiniMax-M3.1-Flash"]"#,
    );
    // (a one-model review chain with fallback is also refused: L-74)
    assert_eq!(
        fields(&validate(&parse(&format!("{MINIMAL}{same}")).unwrap())),
        vec!["models.review", "models.review"]
    );
    // no blank or repeated entries in a chain (after trim)
    let blank = MODELS.replace(r#""deepseek/deepseek-v4-pro""#, r#""  ""#);
    assert_eq!(
        fields(&validate(&parse(&format!("{MINIMAL}{blank}")).unwrap())),
        vec!["models.review"]
    );
    let dup = MODELS.replace(
        r#""deepseek/deepseek-v4-pro""#,
        r#""deepseek/deepseek-v4.1-flash ""#,
    );
    assert_eq!(
        fields(&validate(&parse(&format!("{MINIMAL}{dup}")).unwrap())),
        vec!["models.review"]
    );
    // wait needs a window
    let zero = MODELS.replace("quota_reset_h = 4", "quota_reset_h = 0");
    assert_eq!(
        fields(&validate(&parse(&format!("{MINIMAL}{zero}")).unwrap())),
        vec!["models.quota_reset_h"]
    );
    // empty chains = models not managed by GEAR: valid
    assert!(validate(&parse(MINIMAL).unwrap()).is_empty());
}

#[test]
fn captain_stale_period_is_configurable_and_nonzero() {
    let cfg = parse(&format!(
        "{MINIMAL}\n[timing]\ncaptain_item_stale_min = 30\n"
    ))
    .unwrap();
    assert_eq!(cfg.timing.captain_item_stale_min, 30);
    assert!(validate(&cfg).is_empty());
    let mut cfg = cfg;
    cfg.timing.captain_item_stale_min = 0;
    assert_eq!(
        fields(&validate(&cfg)),
        vec!["timing.captain_item_stale_min"]
    );
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
    assert_eq!(
        cfg.models.lanes[1].args(),
        ["-c", "model_reasoning_effort=medium"]
    );
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
    assert_eq!(
        fields(&validate(&parse(&own_cli).unwrap())),
        vec!["models.lanes"]
    );

    // `models.lanes` is shared by every lane: a claude lane is enough.
    let claude_lane = MINIMAL.replace(
        "kind = \"opencode\"\nprofile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
        "kind = \"claude\"\nprofile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
    );
    let shared = format!(
        "{claude_lane}\n[models]\nlanes = [\"anthropic/claude-sonnet-5-5\", \"claude-sonnet-5-5\"]\n"
    );
    assert_eq!(
        fields(&validate(&parse(&shared).unwrap())),
        vec!["models.lanes"]
    );

    // A lane's own chain is checked against that lane's kind.
    let lane_own = claude_lane.replace(
        "profile = \"opencode\"\n\n[[lanes]]\nname = \"pricing\"",
        "profile = \"opencode\"\nmodels = [\"anthropic/claude-sonnet-5-5\"]\n\n[[lanes]]\nname = \"pricing\"",
    );
    assert_eq!(
        fields(&validate(&parse(&lane_own).unwrap())),
        vec!["lanes[0].models"]
    );

    // The captain's chain follows the captain's kind, not a lane's.
    let captain = format!("{MINIMAL}\n[models]\ncaptain = [\"anthropic/claude-sonnet-5-5\"]\n");
    assert_eq!(
        validate(&parse(&captain).unwrap()),
        vec![],
        "the captain is a codex"
    );
    let claude_captain = captain.replace("kind = \"codex\"", "kind = \"claude\"");
    assert_eq!(
        fields(&validate(&parse(&claude_captain).unwrap())),
        vec!["models.captain"]
    );
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

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
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

/// The same repos, each with its own gate: `[conventions]` may then name none
/// (T-255).
const REPOS_EACH_GATED: &str = r#"
[[repos]]
name = "api"
path = "crates/api"
gate = "make api-gate"

[[repos]]
name = "web"
path = "apps/web"
integration = "feat/web"
release = "main"
gate = "npm test"
"#;

/// The lane of [`MULTI`] that works in the project root.
const PRICING_LANE: &str = r#"[[lanes]]
name = "pricing"
duty = "price engine"
owns = ["lib/pricing/"]
kind = "opencode"
profile = "opencode"
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
            gate: "make test".into(),
            github: None,
        }
    );
    assert_eq!(validate(&cfg), vec![]);
}

/// T-255: the gate has no default. A project that names none is refused,
/// because the engine would then merge into the integration branch and run
/// nothing before pushing.
#[test]
fn a_project_without_a_gate_is_told_to_name_one() {
    let text = MINIMAL.replace("gate = \"make test\"", "gate = \"\"");
    let issues = validate(&parse(&text).unwrap());
    assert_eq!(fields(&issues), vec!["conventions.gate"]);
    assert!(
        issues[0]
            .problem
            .starts_with("name the command the merge gate runs"),
        "{}",
        issues[0].problem
    );

    // A `[[repos]]` entry without a gate runs `conventions.gate`, so with the
    // conventions gate empty that repo has nothing to run either.
    let text = format!(
        "{}{REPOS}",
        MULTI.replace("gate = \"make test\"", "gate = \"\"")
    );
    assert_eq!(
        fields(&validate(&parse(&text).unwrap())),
        vec!["conventions.gate"]
    );
}

/// T-255: a gate in `[conventions]` is enough, and so is a gate in every
/// repo the project runs work in when `[conventions]` names none (M-4220: the
/// root repo counts only when it is the whole project or a lane works there).
#[test]
fn conventions_gate_or_every_repo_gate_satisfies_the_rule() {
    assert_eq!(
        validate(&parse(MINIMAL).unwrap()),
        vec![],
        "[conventions] gate"
    );

    // Every repo has its own gate, and no lane works in the root: the empty
    // root gate never runs.
    let text = format!(
        "{}{REPOS_EACH_GATED}",
        MULTI
            .replace("gate = \"make test\"\n", "")
            .replace(PRICING_LANE, "")
    );
    assert_eq!(
        validate(&parse(&text).unwrap()),
        vec![],
        "every repo has its own gate"
    );

    // The `pricing` lane works in the root, so the root runs a gate too and
    // `[conventions]` must name it.
    let text = format!(
        "{}{REPOS_EACH_GATED}",
        MULTI.replace("gate = \"make test\"\n", "")
    );
    assert_eq!(
        fields(&validate(&parse(&text).unwrap())),
        vec!["conventions.gate"],
        "a lane works in the root"
    );
}

/// A repo without branches or a gate uses the project's `[conventions]`.
#[test]
fn a_repo_without_its_own_branches_uses_the_conventions() {
    let text = format!(
        "{}{REPOS}",
        MULTI.replace(
            "gate = \"make test\"",
            "integration = \"feat/dev\"\nrelease = \"feat/main\"\ngate = \"cargo test\""
        )
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
    assert_eq!(
        cfg.repo_of_lane("nobody"),
        root,
        "a lane that is not in the config"
    );
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
    cfg.conventions.gate = "go build ./... && go vet ./internal/... 2>&1 | head -20".into();
    assert_eq!(fields(&validate(&cfg)), vec!["conventions.gate"]);

    for gate in ["make gate", "a && b", "a || b", "set -o pipefail; a | b"] {
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
    assert_eq!(
        validate(&cfg),
        vec![],
        "a lane without a repo works in the root"
    );
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
    assert!(
        cfg.quota.sources.is_empty(),
        "no source without the section"
    );
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
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.timing.delivery_grace_s, 45);
    let price = &cfg.prices["openai/model"];
    assert_eq!(
        (
            price.input,
            price.output,
            price.cache_read,
            price.cache_write
        ),
        (1.5, 2.0, 0.25, 0.0)
    );
    assert!(validate(&cfg).is_empty());
    let mut bad = cfg.clone();
    bad.timing.delivery_grace_s = 0;
    bad.prices.get_mut("openai/model").unwrap().cache_read = -1.0;
    let issues = validate(&bad);
    let fields = fields(&issues);
    assert!(fields.contains(&"timing.delivery_grace_s"), "{issues:?}");
    assert!(
        fields.contains(&"prices.openai/model.cache_read"),
        "{issues:?}"
    );
    assert_eq!(
        parse(&text.replace("input=1.5", "input=1")).unwrap().prices["openai/model"].input,
        1.0
    );
    assert!(parse(&text.replace("output=2.0", "output='bad'"))
        .unwrap_err()
        .detail
        .contains("output"));
}

// T-252: branch names are configuration. The defaults are the names GEAR
// itself uses; a project may name its own, with `{project}`, `{lane}`, `{n}`.

#[test]
fn branch_templates_default_to_the_names_gear_uses() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.conventions.task_branch, gear_model::branch::TASK_BRANCH);
    assert_eq!(
        cfg.conventions.review_branch,
        gear_model::branch::REVIEW_BRANCH
    );
    assert_eq!(validate(&cfg), vec![], "the defaults are valid");

    // T-255 gave the fixture its own `[conventions]` section: the templates
    // join it rather than a second one. T-372: the seat branch keeps its own
    // folder, so it is no ref above the task branches.
    let text = MINIMAL.replace(
        GATE_LINE,
        "gate = \"make test\"\ntask_branch = \"{project}/{lane}/t{n}\"\nreview_branch = \"{project}/review/t{n}\"\nseat_branch = \"{project}/home/{seat}\"",
    );
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.conventions.task_branch, "{project}/{lane}/t{n}");
    assert_eq!(
        validate(&cfg),
        vec![],
        "a project may name its own branches"
    );
}

#[test]
fn a_branch_template_must_be_expandable_and_usable() {
    let with = |field: &str, value: &str| {
        parse(&MINIMAL.replace(
            GATE_LINE,
            &format!("gate = \"make test\"\n{field} = \"{value}\""),
        ))
        .unwrap()
    };
    // No `{n}`: every task would share one branch.
    assert_eq!(
        fields(&validate(&with("task_branch", "{lane}/work"))),
        vec!["conventions.task_branch"]
    );
    assert_eq!(
        fields(&validate(&with("review_branch", "review"))),
        vec!["conventions.review_branch"]
    );
    // A placeholder GEAR does not fill in stays a brace in the name.
    assert_eq!(
        fields(&validate(&with("task_branch", "{team}/t{n}"))),
        vec!["conventions.task_branch"]
    );
    assert_eq!(
        fields(&validate(&with("task_branch", "{lane}/{task}"))),
        vec!["conventions.task_branch"]
    );
    // A name git would refuse: a leading '/', "..", "//", a trailing '/', and
    // a trailing ".lock".
    for bad in [
        "/x/t{n}",
        "a..b/t{n}",
        "{lane}/../t{n}",
        "{lane}//t{n}",
        "{lane}/t{n}/",
        "{lane}/t{n}.lock",
    ] {
        assert_eq!(
            fields(&validate(&with("task_branch", bad))),
            vec!["conventions.task_branch"],
            "{bad:?}"
        );
    }
    // Two templates that name the same branch for the same task.
    assert_eq!(
        fields(&validate(&with("review_branch", "{lane}/t{n}"))),
        vec!["conventions.review_branch"]
    );
}

#[test]
fn a_seat_branch_names_the_project_and_the_seat() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.conventions.seat_branch, gear_model::branch::SEAT_BRANCH);
    assert_eq!(validate(&cfg), vec![], "the default seat branch is valid");

    let with = |value: &str| {
        parse(&MINIMAL.replace(
            GATE_LINE,
            &format!("gate = \"make test\"\nseat_branch = \"{value}\""),
        ))
        .unwrap()
    };
    // No `{seat}`: every seat would share one worktree branch.
    assert_eq!(
        fields(&validate(&with("{project}/work"))),
        vec!["conventions.seat_branch"]
    );
    // The task placeholders belong to the task branches, not to a seat's.
    for bad in ["{project}/{lane}", "{project}/t{n}", "{project}/{team}"] {
        assert_eq!(
            fields(&validate(&with(bad))),
            vec!["conventions.seat_branch"],
            "{bad:?}"
        );
    }
    // A name git would refuse.
    assert_eq!(
        fields(&validate(&with("{project}//{seat}"))),
        vec!["conventions.seat_branch"]
    );
    // A seat branch is free to be any name git takes.
    assert_eq!(validate(&with("{project}-{seat}")), vec![]);
}

// T-372: git cannot hold a ref and a folder of the same name, so a seat
// branch that is a task or review branch (or a folder of one) is refused at
// init: a lane would have to delete its own seat branch to cut a task branch.

#[test]
fn a_seat_branch_that_holds_a_task_or_review_branch_is_refused() {
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.project.id = "p".into();
    cfg.lanes.truncate(1); // the lane "heat"
    cfg.conventions.task_branch = "{project}/{lane}/t{n}".into();
    let issues = validate(&cfg);
    assert_eq!(fields(&issues), vec!["conventions.seat_branch"]);
    assert!(
        issues[0]
            .problem
            .contains("the seat branch p/heat and the task branch p/heat/t1"),
        "{}",
        issues[0].problem
    );
    assert!(
        issues[0]
            .problem
            .contains("e.g. seat_branch = \"{project}/home/{seat}\""),
        "{}",
        issues[0].problem
    );

    // A seat branch in its own folder collides with nothing.
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.project.id = "p".into();
    cfg.conventions.task_branch = "{project}/{lane}/t{n}".into();
    cfg.conventions.seat_branch = "{project}/home/{seat}".into();
    assert_eq!(validate(&cfg), vec![]);

    // The defaults keep the three apart.
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.project.id = "p".into();
    assert_eq!(validate(&cfg), vec![]);

    // A review branch under a reviewer's own seat branch collides too, and the
    // finding names it as one.
    let mut cfg = parse(MINIMAL).unwrap();
    cfg.project.id = "p".into();
    cfg.lanes.truncate(1);
    cfg.conventions.review_branch = "{project}/review-1/t{n}".into();
    let issues = validate(&cfg);
    assert_eq!(fields(&issues), vec!["conventions.seat_branch"]);
    assert!(
        issues[0]
            .problem
            .contains("the seat branch p/review-1 and the review branch p/review-1/t1"),
        "{}",
        issues[0].problem
    );
}

// T-257: the remote and the voyage folder are configuration too. The defaults
// are the names GEAR itself uses; a project may name its own.

#[test]
fn the_remote_and_the_voyage_folder_default_to_the_names_gear_uses() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(cfg.conventions.remote, gear_model::layout::REMOTE);
    assert_eq!(cfg.conventions.voyage_dir, gear_model::layout::VOYAGE_DIR);
    assert_eq!(cfg.conventions.remote, "origin");
    assert_eq!(cfg.conventions.voyage_dir, "gear/voyages/{milestone}");
    assert_eq!(validate(&cfg), vec![], "the defaults are valid");
}

#[test]
fn a_project_names_its_own_remote_and_voyage_folder() {
    let text = MINIMAL.replace(
        GATE_LINE,
        "gate = \"make test\"\nremote = \"upstream\"\nvoyage_dir = \"plans/{project}/{milestone}\"",
    );
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.conventions.remote, "upstream");
    assert_eq!(cfg.conventions.voyage_dir, "plans/{project}/{milestone}");
    assert_eq!(validate(&cfg), vec![], "a project may name its own layout");
}

#[test]
fn a_remote_and_a_voyage_folder_must_be_usable() {
    let with = |field: &str, value: &str| {
        parse(&MINIMAL.replace(
            GATE_LINE,
            &format!("gate = \"make test\"\n{field} = \"{value}\""),
        ))
        .unwrap()
    };
    // No remote to fetch from, and a name with a space in it is two arguments
    // to git.
    for bad in ["", "  ", "up stream", "up\tstream"] {
        assert_eq!(
            fields(&validate(&with("remote", bad))),
            vec!["conventions.remote"],
            "{bad:?}"
        );
    }
    // Without `{milestone}` every voyage shares one folder.
    for bad in ["", "gear/voyages", "gear/voyages/{project}"] {
        assert_eq!(
            fields(&validate(&with("voyage_dir", bad))),
            vec!["conventions.voyage_dir"],
            "{bad:?}"
        );
    }
    // A remote name and a voyage folder that can be used.
    assert_eq!(validate(&with("remote", "upstream")), vec![]);
    assert_eq!(
        validate(&with("voyage_dir", "plans/{project}/{milestone}")),
        vec![]
    );
}

// T-299: commit messages are configuration. The three defaults are the forms
// GEAR itself writes; a project may name its own, with the placeholders the
// writer fills in.

#[test]
fn commit_messages_default_to_the_forms_gear_writes() {
    let cfg = parse(MINIMAL).unwrap();
    assert_eq!(
        cfg.conventions.merge_message,
        gear_model::commit::MERGE_MESSAGE
    );
    assert_eq!(
        cfg.conventions.review_message,
        gear_model::commit::REVIEW_MESSAGE
    );
    assert_eq!(
        cfg.conventions.plan_message,
        gear_model::commit::PLAN_MESSAGE
    );
    assert_eq!(
        (
            cfg.conventions.merge_message.as_str(),
            cfg.conventions.review_message.as_str(),
            cfg.conventions.plan_message.as_str(),
        ),
        (
            "merge({task}): {source}",
            "review({task}): {what}",
            "{milestone}: {what}"
        ),
    );
    assert_eq!(validate(&cfg), vec![], "the defaults are valid");
}

#[test]
fn the_three_commit_messages_parse_and_are_read_like_the_other_conventions() {
    let text = MINIMAL.replace(
        GATE_LINE,
        "gate = \"make test\"\nmerge_message = \"[{task}] merge {source}\"\nreview_message = \"{project} review of {task} (attempt {attempt}): {what}\"\nplan_message = \"PROJ-1 {milestone} {what}\"",
    );
    let cfg = parse(&text).unwrap();
    assert_eq!(cfg.conventions.merge_message, "[{task}] merge {source}");
    assert_eq!(
        cfg.conventions.review_message,
        "{project} review of {task} (attempt {attempt}): {what}"
    );
    assert_eq!(cfg.conventions.plan_message, "PROJ-1 {milestone} {what}");
    assert_eq!(
        validate(&cfg),
        vec![],
        "a project may name its own commit messages"
    );
}

#[test]
fn a_commit_message_template_must_be_usable() {
    let with = |field: &str, value: &str| {
        parse(&MINIMAL.replace(
            GATE_LINE,
            &format!("gate = \"make test\"\n{field} = \"{value}\""),
        ))
        .unwrap()
    };
    // The merge message must name the task; it may name `{source}`.
    assert_eq!(
        fields(&validate(&with("merge_message", "merged"))),
        vec!["conventions.merge_message"]
    );
    assert_eq!(
        fields(&validate(&with("merge_message", ""))),
        vec!["conventions.merge_message"]
    );
    assert_eq!(
        fields(&validate(&with("merge_message", "merge({task}): {branch}"))),
        vec!["conventions.merge_message"]
    );
    assert_eq!(
        validate(&with("merge_message", "[{task}] merge {source}")),
        vec![]
    );
    // The review message must name the task; `{attempt}` and `{what}` are the
    // reviewer's.
    assert_eq!(
        fields(&validate(&with("review_message", "{who} reviewed"))),
        vec!["conventions.review_message"]
    );
    assert_eq!(
        fields(&validate(&with("review_message", "reviewed {attempt}"))),
        vec!["conventions.review_message"]
    );
    assert_eq!(
        validate(&with("review_message", "{task} {attempt} {what}")),
        vec![]
    );
    // The plan message may leave out `{milestone}`; it may not name a
    // placeholder GEAR does not fill in, and it may not be empty.
    assert_eq!(
        fields(&validate(&with("plan_message", ""))),
        vec!["conventions.plan_message"]
    );
    assert_eq!(
        fields(&validate(&with("plan_message", "{task}: {what}"))),
        vec!["conventions.plan_message"]
    );
    assert_eq!(
        validate(&with("plan_message", "PROJ-1 {milestone} {what}")),
        vec![]
    );
    assert_eq!(validate(&with("plan_message", "{what}")), vec![]);
    // Each text names the template and the placeholders GEAR fills in.
    let issues = validate(&with("merge_message", "merge({task}): {branch}"));
    assert!(
        issues[0].problem.contains("merge({task}): {branch}"),
        "{issues:?}"
    );
    assert!(issues[0].problem.contains("{project}"), "{issues:?}");
    assert!(issues[0].problem.contains("{source}"), "{issues:?}");
    let issues = validate(&with("merge_message", "merged"));
    assert!(issues[0].problem.contains("merged"), "{issues:?}");
    assert!(issues[0].problem.contains("{task}"), "{issues:?}");
}
