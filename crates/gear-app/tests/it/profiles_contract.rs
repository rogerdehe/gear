//! Contract for the profiles a project configures (T-31): `[profiles.<name>]`
//! defines or overrides a profile, a seat's profile must be able to run
//! unattended, and the kickoff prints a skeleton for a kind GEAR does not know.

use gear_app::admin;
use gear_app::config;
use gear_app::kickoff;
use gear_app::Env;
use gear_model::{ErrorCode, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;

fn config_with(profiles: &str) -> String {
    format!(
        r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[profiles.grok]
kind = "grok"
unattended_args = ["--yolo"]
exit_keys = ["ctrl+c", "ctrl+d"]

[profiles.opencode]
confirm_rounds = 5
{profiles}

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#
    )
}

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", NOW)?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn admin_env() -> Env {
    Env { ctx: None, pane_id: None, admin: true }
}

#[test]
fn a_profile_block_defines_a_profile() {
    let text = config_with("");
    let parsed = config::parse(&text).unwrap();
    let table = config::profile_table(&gear_herdr::builtins(), &parsed).unwrap();

    let grok = table.get("grok").expect("grok is configured");
    assert_eq!(grok.kind, "grok");
    assert_eq!(grok.unattended_args, vec!["--yolo".to_string()]);
    assert_eq!(grok.exit_keys, vec!["ctrl+c".to_string(), "ctrl+d".to_string()]);
}

#[test]
fn a_partial_block_overrides_only_the_given_fields() {
    let text = config_with("");
    let parsed = config::parse(&text).unwrap();
    let table = config::profile_table(&gear_herdr::builtins(), &parsed).unwrap();

    // opencode: only confirm_rounds was given; the built-in's other fields stay.
    let opencode = table.get("opencode").expect("opencode is built in");
    let builtin = gear_herdr::builtin("opencode").unwrap();
    assert_eq!(opencode.confirm_rounds, 5);
    assert_eq!(opencode.unattended_args, builtin.unattended_args);
    assert_eq!(opencode.quota_markers, builtin.quota_markers);
    assert_eq!(opencode.exit_keys, builtin.exit_keys);
    assert_eq!(opencode.kind, builtin.kind);
}

#[test]
fn a_profile_without_unattended_args_is_refused_at_init() {
    let mut s = world();
    // A lane whose profile has no unattended args cannot run unattended.
    // A kind with no built-in profile: the block is the whole profile.
    let text = config_with("").replace(
        r#"[profiles.grok]
kind = "grok"
unattended_args = ["--yolo"]
exit_keys = ["ctrl+c", "ctrl+d"]"#,
        r#"[profiles.omp]
kind = "omp""#,
    ).replace(r#"profile = "opencode""#, r#"profile = "omp""#);

    let err = admin::init_with_home(&mut s, &admin_env(), &text,Some(crate::skill_home()), false, NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("unattended"), "{}", err.detail);
    // Nothing was registered.
    assert!(s.read(|tx| tx.get_project("toy")).unwrap().is_none());
}

#[test]
fn an_unknown_profile_is_refused_at_init() {
    let mut s = world();
    let text = config_with("").replace(r#"profile = "opencode""#, r#"profile = "grokline""#);
    let err = admin::init_with_home(&mut s, &admin_env(), &text,Some(crate::skill_home()), false, NOW).unwrap_err();
    assert_eq!(err.code, ErrorCode::BadFormat, "{err:?}");
    assert!(err.detail.contains("grokline"), "{}", err.detail);
}

#[test]
fn a_configured_profile_is_accepted_at_init() {
    let mut s = world();
    let text = config_with("").replace(r#"profile = "opencode""#, r#"profile = "grok""#);
    let r = admin::init_with_home(&mut s, &admin_env(), &text,Some(crate::skill_home()), false, NOW).unwrap();
    assert_eq!(r.project, "toy");
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!(seat.profile, "grok");
}

#[test]
fn the_kickoff_prints_a_skeleton_for_an_unknown_kind() {
    let inventory = kickoff::Inventory {
        installed: vec![kickoff::Installed {
            kind: "omp".into(),
            executable: "/usr/local/bin/omp".into(),
            models: vec!["omp-1".into()],
            note: None,
            skill: false,
        }],
        missing: vec![],
    };
    let text = kickoff::skeleton(&inventory);
    assert!(text.contains("[profiles.omp]"), "{text}");
    assert!(text.contains("# kind = \"omp\""), "{text}");
    assert!(text.contains("# unattended_args"), "{text}");
    // A kind with a built-in profile needs no block.
    let known = kickoff::Inventory {
        installed: vec![kickoff::Installed {
            kind: "opencode".into(),
            executable: "/usr/local/bin/opencode".into(),
            models: vec![],
            note: None,
            skill: true,
        }],
        missing: vec![],
    };
    assert!(!kickoff::skeleton(&known).contains("[profiles.opencode]"));
}

/// R2 (review): a new profile that sets no numeric fields gets the operational
/// screen defaults (16 lines, two confirmation rounds), not an all-zero
/// profile that can never see a marker.
#[test]
fn a_new_profile_gets_the_operational_defaults() {
    let text = config_with(
        r#"
[profiles.omp]
kind = "omp"
unattended_args = ["--yolo"]
busy_markers = ["BUSY"]
"#,
    );
    let parsed = config::parse(&text).unwrap();
    let table = config::profile_table(&gear_herdr::builtins(), &parsed).unwrap();
    let omp = table.get("omp").expect("omp is configured");
    assert_eq!(omp.tail_lines, 16, "the screen window has a real default");
    assert_eq!(omp.confirm_rounds, 2, "a flag needs two observations");
    assert_eq!(omp.busy_markers, vec!["BUSY".to_string()]);
    // A built-in keeps its own values when the block leaves them out.
    let opencode = table.get("opencode").expect("built in");
    assert_eq!(opencode.tail_lines, gear_herdr::builtin("opencode").unwrap().tail_lines);
}

#[test]
fn a_profile_can_configure_its_footer_quota_bar() {
    let text = config_with(r#"
[profiles.claude.quota_bar]
footer_prefix = "⏵⏵ bypass permissions on"
separator_prefix = "─"
windows = ["5h", "7d"]
"#);
    let parsed = config::parse(&text).unwrap();
    let table = config::profile_table(&gear_herdr::builtins(), &parsed).unwrap();
    assert_eq!(table["claude"].quota_bar, Some(gear_herdr::QuotaBar::claude()));
    assert!(table["codex"].quota_bar.is_none(), "unverified status formats stay disabled");
}
