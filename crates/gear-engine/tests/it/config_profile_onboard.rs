//! Contract for the profiles a project configures (T-31): a lane whose
//! profile comes from `[profiles.<name>]` is onboarded with that profile's
//! kind and args — no code change for a new agent kind.

use gear_app::{admin, Env};
use gear_engine::onboard::onboard_queued;
use gear_engine::EngineCfg;
use gear_herdr::fake::{Call, FakeHerdr};
use gear_model::SeatState;
use gear_store::Store;

const T0: i64 = 1_800_000_000;

/// A project with a lane on the configured `grok` profile.
const CONFIG: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "claude"
profile = "claude"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "grok"
profile = "grok"

[profiles.grok]
kind = "grok"
unattended_args = ["--yolo"]
exit_keys = ["ctrl+c", "ctrl+d"]
resume_args = ["-s", "{session}"]

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#;

fn cfg() -> EngineCfg {
    let mut c = EngineCfg { db_path: std::env::temp_dir().join(format!("gear-engine-tests-{}/gear.db", std::process::id())).to_string_lossy().into_owned(), ..Default::default() };
    // The built-ins only: grok comes from the project config.
    for (name, profile) in gear_herdr::builtins() {
        c.profiles.insert(name, profile);
    }
    c
}

#[test]
fn a_configured_profile_onboards_its_lane() {
    // A grok lane writes its trust file under HOME: keep it out of the
    // user's own home, and out of another test's window (HOME is
    // process-wide).
    let home = tempfile::tempdir().unwrap();
    let _home = crate::HOME_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let previous = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());

    let mut s = Store::open_in_memory().unwrap();
    admin::init_with_home(&mut s, &Env { ctx: None, pane_id: None, admin: true }, CONFIG,Some(crate::skill_home()), None, T0).unwrap();
    let mut herdr = FakeHerdr::new();

    let active = onboard_queued(&mut s, &mut herdr, &cfg(), T0 + 1).unwrap();
    assert_eq!(active, vec!["toy:heat".to_string()]);

    // The lane is started as a grok agent with the configured args.
    let starts: Vec<(&str, &str, &Vec<String>)> = herdr
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::AgentStart { name, kind, args, .. } => Some((name.as_str(), kind.as_str(), args)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!((starts[0].0, starts[0].1), ("toy-heat", "grok"));
    assert!(starts[0].2.contains(&"--yolo".to_string()), "{:?}", starts[0].2);

    // The seat is active and bound, as for any lane.
    let seat = s.read(|tx| tx.get_seat("toy:heat")).unwrap().unwrap();
    assert_eq!((seat.state, seat.generation), (SeatState::Active, 1));

    match previous {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}
