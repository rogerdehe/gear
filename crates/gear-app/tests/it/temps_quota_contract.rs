//! T-237: a lower `[quota] temps` retires the reviewer seats it no longer
//! allows. The extra temp seats of an older, larger quota each keep their own
//! worktree, and the engine would rotate through all of them.

use gear_app::admin::init_with_home;
use gear_app::Env;
use gear_model::{SeatRole, SeatState};
use gear_store::{SeatRow, Store};

const T0: i64 = 1_800_000_000;
const HARBOR: &str = "harbor";

fn config(temps: usize) -> String {
    format!(
        r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 3
temps = {temps}

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = ["gpt-6.1-sol"]
on_quota = {{ lanes = "wait", review = "wait" }}

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#
    )
}

fn admin() -> Env {
    Env { ctx: None, pane_id: None, admin: true }
}

/// A reviewer seat in the state the engine left it in.
fn reviewer(store: &mut Store, name: &str, state: SeatState, since: i64) {
    store
        .write(|tx| {
            tx.insert_seat(&SeatRow {
                id: format!("{HARBOR}:{name}"),
                project: HARBOR.to_string(),
                role: SeatRole::Temp,
                name: name.to_string(),
                profile: "codex".to_string(),
                generation: 1,
                state,
                state_since: since,
                state_reason: None,
                task_id: None,
            })
        })
        .unwrap();
}

/// The seat's state and the reason it was left in it.
fn state_of(store: &mut Store, name: &str) -> (SeatState, Option<String>) {
    let seat = store.read(|tx| tx.get_seat(&format!("{HARBOR}:{name}"))).unwrap().unwrap();
    (seat.state, seat.state_reason)
}

#[test]
fn a_lower_temps_quota_retires_the_oldest_released_reviewers() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(2), Some(crate::skill_home()), None, T0).unwrap();

    // Four reviewer seats from the time the project had temps = 4.
    reviewer(&mut s, "review-7", SeatState::Active, T0 + 10);
    reviewer(&mut s, "review-9", SeatState::Released, T0 + 20);
    reviewer(&mut s, "review-10", SeatState::Released, T0 + 30);
    reviewer(&mut s, "review-11", SeatState::Released, T0 + 40);

    init_with_home(&mut s, &admin(), &config(2), Some(crate::skill_home()), None, T0 + 50).unwrap();

    // The working reviewer and the newest released one stay.
    assert_eq!(state_of(&mut s, "review-7"), (SeatState::Active, None), "an Active seat is never retired");
    assert_eq!(state_of(&mut s, "review-11").0, SeatState::Released, "the newest seat stays");
    // The two older released ones are retired, and say why.
    for name in ["review-9", "review-10"] {
        let (state, reason) = state_of(&mut s, name);
        assert_eq!(state, SeatState::Retired, "{name}");
        assert_eq!(reason.as_deref(), Some("temps quota lowered to 2"), "{name}");
    }
}

#[test]
fn a_standby_reviewer_beyond_the_quota_goes_releasing() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(2), Some(crate::skill_home()), None, T0).unwrap();
    reviewer(&mut s, "review-7", SeatState::Active, T0 + 10);
    reviewer(&mut s, "review-9", SeatState::Released, T0 + 10);
    // Its agent and its pane are still there: it cannot be Retired in one step.
    reviewer(&mut s, "review-11", SeatState::Standby, T0 + 20);
    reviewer(&mut s, "review-10", SeatState::Released, T0 + 30);

    init_with_home(&mut s, &admin(), &config(2), Some(crate::skill_home()), None, T0 + 50).unwrap();

    // The Active seat and the newest idle one fill the quota.
    assert_eq!(state_of(&mut s, "review-7").0, SeatState::Active);
    assert_eq!(state_of(&mut s, "review-10").0, SeatState::Released);
    // The idle seat past the quota that has no agent is Retired ...
    assert_eq!(state_of(&mut s, "review-9"), (SeatState::Retired, Some("temps quota lowered to 2".to_string())));
    // ... and the one whose agent is still there is sent on its way out.
    assert_eq!(state_of(&mut s, "review-11"), (SeatState::Releasing, Some("temps quota lowered to 2".to_string())));
}

#[test]
fn a_quota_that_fits_the_seats_retires_nothing() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(2), Some(crate::skill_home()), None, T0).unwrap();
    reviewer(&mut s, "review-7", SeatState::Active, T0 + 10);
    reviewer(&mut s, "review-9", SeatState::Released, T0 + 20);
    reviewer(&mut s, "review-10", SeatState::Released, T0 + 30);
    reviewer(&mut s, "review-11", SeatState::Released, T0 + 40);

    init_with_home(&mut s, &admin(), &config(4), Some(crate::skill_home()), None, T0 + 50).unwrap();

    for (name, state) in [
        ("review-7", SeatState::Active),
        ("review-9", SeatState::Released),
        ("review-10", SeatState::Released),
        ("review-11", SeatState::Released),
    ] {
        assert_eq!(state_of(&mut s, name).0, state, "{name}");
    }
}
