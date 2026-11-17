//! Observation and reconciliation (design/08).

use gear_herdr::{classify_screen, AgentInfo, AgentStatus, HerdrRead, Profile, ScreenFlag};
use gear_model::{GearResult, SeatState};
use gear_store::{BindingStatus, SeatRow, Store, Tx};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// Expected agent is in the bound pane (kind matches binding).
    Present,
    /// Pane gone or no agent, and the seat expects one.
    Missing,
    /// A different agent occupies the pane with no pending onboard/release op.
    Changed,
    /// Seat expects no agent (`SeatState::expects_agent()` is false) or is mid onboard/release.
    NotExpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatObs {
    pub seat: SeatRow,
    pub presence: Presence,
    pub status: Option<AgentStatus>,
    /// Only filled when status is idle/done (one tail read).
    pub flags: BTreeSet<ScreenFlag>,
    /// Idle/done with no Busy flag.
    pub idle: bool,
    /// The last screen lines of the agent, when the screen was read: an idle
    /// or done agent, a blocked one (the dialog it waits on) or a working
    /// agent of a kind that shows a provider back-off (T-75). None when the
    /// screen was not read.
    pub screen: Option<String>,
    /// The session the agent in the pane reports (T-97). A hand restart of the
    /// agent shows a new one: that is how a delivery that was never confirmed
    /// gets its second chance without a store edit.
    pub session: Option<String>,
}

/// One `agents()` call for the whole session. `Err` = observation failed:
/// the caller must make **no** decisions for this project this tick.
pub fn snapshot(herdr: &mut dyn HerdrRead) -> Result<Vec<AgentInfo>, gear_herdr::ObserveError> {
    herdr.agents()
}

/// Compare seats with the snapshot (reads bindings inside `tx`; reads screen
/// tails via `herdr` only for idle/done agents — callers must not hold a write
/// transaction while doing so, so this takes a read `Tx`).
/// Screen flags use `profiles[seat.profile]`; a seat whose profile is missing
/// is never `idle` (fail closed).
/// Slice 1 does not change seat states; slice 2+ acts on Missing/Changed.
pub fn reconcile(
    tx: &Tx<'_>,
    herdr: &mut dyn HerdrRead,
    project: &str,
    agents: &[AgentInfo],
    profiles: &BTreeMap<String, Profile>,
) -> GearResult<Vec<SeatObs>> {
    let mut observed = Vec::new();
    let mut cwds = None;
    for seat in tx.seats_of_project(project)? {
        observed.push(observe_seat(tx, herdr, seat, agents, profiles, &mut cwds)?);
    }
    Ok(observed)
}

/// The meta key of the repository a seat's agent works in, recorded the first
/// time the agent is seen with a session: `seat.home:<seat>:<generation>`.
pub fn home_key(seat: &str, generation: i64) -> String {
    format!("seat.home:{seat}:{generation}")
}

/// The meta key that marks a seat's bound pane as held by another agent:
/// `seat.foreign:<seat>:<generation>` = that agent's session. While it is set,
/// nothing is typed into the pane, its agent is not renamed and the pane is
/// not closed for the seat.
pub fn foreign_key(seat: &str, generation: i64) -> String {
    format!("seat.foreign:{seat}:{generation}")
}

/// Whether the seat's bound pane is marked as held by another agent.
pub fn is_foreign(tx: &Tx<'_>, seat: &str, generation: i64) -> GearResult<bool> {
    Ok(tx
        .get_meta(&foreign_key(seat, generation))?
        .is_some_and(|value| !value.is_empty()))
}

/// The repository a directory belongs to: git's common directory, which every
/// worktree of one repository shares, or the directory itself outside git.
pub fn repo_identity(cwd: &str) -> String {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            cwd,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if dir.is_empty() {
                cwd.trim_end_matches('/').to_string()
            } else {
                dir.trim_end_matches('/').to_string()
            }
        }
        _ => cwd.trim_end_matches('/').to_string(),
    }
}

/// The directory Herdr reports for `pane`, read once per reconcile and only
/// when asked. A read that fails knows no directory.
fn pane_cwd(
    herdr: &mut dyn HerdrRead,
    cwds: &mut Option<BTreeMap<String, String>>,
    pane: &str,
) -> Option<String> {
    if cwds.is_none() {
        *cwds = Some(herdr.pane_cwds().unwrap_or_default());
    }
    cwds.as_ref().and_then(|cwds| cwds.get(pane).cloned())
}

/// Whether the agent in a seat's pane is another agent (pharos, 2026-11-16).
///
/// A pane outlives its agent: the user may start another project's agent of
/// the same CLI in it. Its kind matches the binding, so the kind alone called
/// it the seat's agent and the seat's prompts went to it. A new session alone
/// does not say so either (`/new` and a hand restart are new sessions of the
/// seat's own agent, T-97). Both together do: a session that is not the
/// binding's, in a repository that is not the one the seat's agent was first
/// seen in. Anything unknown (no session, no recorded repository, no directory)
/// keeps the seat's agent present, as before.
fn is_another_agent(
    tx: &Tx<'_>,
    herdr: &mut dyn HerdrRead,
    seat: &SeatRow,
    binding: &gear_store::BindingRow,
    session: Option<&str>,
    cwds: &mut Option<BTreeMap<String, String>>,
) -> GearResult<bool> {
    let (Some(now), Some(recorded)) = (
        session.filter(|session| !session.is_empty()),
        binding
            .session_ref
            .as_deref()
            .filter(|session| !session.trim().is_empty()),
    ) else {
        return Ok(false);
    };
    if now == recorded {
        return Ok(false);
    }
    let Some(home) = tx
        .get_meta(&home_key(&seat.id, seat.generation))?
        .filter(|home| !home.is_empty())
    else {
        return Ok(false);
    };
    let Some(cwd) = pane_cwd(herdr, cwds, &binding.pane_id) else {
        return Ok(false);
    };
    Ok(repo_identity(&cwd) != home)
}

/// One seat against the snapshot: is the expected agent in its pane, and is it
/// idle?
fn observe_seat(
    tx: &Tx<'_>,
    herdr: &mut dyn HerdrRead,
    seat: SeatRow,
    agents: &[AgentInfo],
    profiles: &BTreeMap<String, Profile>,
    cwds: &mut Option<BTreeMap<String, String>>,
) -> GearResult<SeatObs> {
    if !seat.state.expects_agent() {
        // The seat expects no agent (queued/released, mid onboard/release, or
        // already missing/retired/needs-check): nothing is judged about it.
        return Ok(SeatObs {
            seat,
            presence: Presence::NotExpected,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
            screen: None,
            session: None,
        });
    }

    let binding = tx
        .get_binding(&seat.id, seat.generation)?
        .filter(|b| b.status == BindingStatus::Active);
    let Some(binding) = binding else {
        // The seat expects an agent but has no active binding to look at.
        return Ok(SeatObs {
            seat,
            presence: Presence::Missing,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
            screen: None,
            session: None,
        });
    };

    let agent = agents.iter().find(|a| a.pane_id == binding.pane_id);
    let Some(agent) = agent else {
        // The pane is gone or hosts no agent.
        return Ok(SeatObs {
            seat,
            presence: Presence::Missing,
            status: None,
            flags: BTreeSet::new(),
            idle: false,
            screen: None,
            session: None,
        });
    };

    if agent.agent.as_deref() != Some(binding.kind.as_str()) {
        // Another agent occupies the pane: the seat's agent is not there.
        return Ok(SeatObs {
            seat,
            presence: Presence::Changed,
            status: Some(agent.agent_status),
            flags: BTreeSet::new(),
            idle: false,
            screen: None,
            session: None,
        });
    }

    let session = agent
        .agent_session
        .as_ref()
        .map(|session| session.value.clone());
    if is_another_agent(tx, herdr, &seat, &binding, session.as_deref(), cwds)? {
        // Another agent of the same CLI sits in the seat's pane: the seat's
        // agent is not there. `session` names the occupant, so the record step
        // marks the pane and nothing reaches it.
        return Ok(SeatObs {
            seat,
            presence: Presence::Changed,
            status: Some(agent.agent_status),
            flags: BTreeSet::new(),
            idle: false,
            screen: None,
            session,
        });
    }

    let (flags, idle, screen) =
        screen_of(herdr, &seat, &binding.pane_id, agent.agent_status, profiles);
    Ok(SeatObs {
        seat,
        presence: Presence::Present,
        status: Some(agent.agent_status),
        flags,
        idle,
        screen,
        session,
    })
}

/// The screen of the seat's agent, the flags it shows and whether the agent
/// is idle.
///
/// The screen is read for an idle or done agent (its flags say what the seat
/// may do next) and for a blocked one (the captain's alert quotes the dialog
/// it waits on). A **working** agent's screen is read only for a kind that
/// shows a provider back-off (T-75): such an agent waits for the provider
/// for minutes without any output, and that is not a stall. Every other
/// working agent costs no read.
///
/// A screen that cannot be read, and a seat whose profile is unknown, fail
/// closed: no flags and no idle.
fn screen_of(
    herdr: &mut dyn HerdrRead,
    seat: &SeatRow,
    pane_id: &str,
    status: AgentStatus,
    profiles: &BTreeMap<String, Profile>,
) -> (BTreeSet<ScreenFlag>, bool, Option<String>) {
    let profile = profiles.get(&seat.profile);
    let idle_or_done = matches!(status, AgentStatus::Idle | AgentStatus::Done);
    let blocked = status == AgentStatus::Blocked;
    // A working agent is read only for a kind that shows a back-off.
    let shows_backoff = profile
        .map(|profile| !profile.backoff_markers.is_empty())
        .unwrap_or(false);
    if !idle_or_done && !blocked && !shows_backoff {
        return (BTreeSet::new(), false, None);
    }
    let lines = profile.map(|profile| profile.tail_lines).unwrap_or(16);
    let Ok(tail) = herdr.read_tail(pane_id, lines) else {
        // The screen could not be read: fail closed.
        return (BTreeSet::new(), false, None);
    };
    if idle_or_done {
        let Some(profile) = profile else {
            // Unknown profile: the screen cannot be read the way it must be.
            return (BTreeSet::new(), false, None);
        };
        let flags = classify_screen(profile, &tail);
        // M4 §10.2: whether an agent is busy is Herdr's state, never the text
        // on its screen. An idle or done agent is idle.
        let idle = idle_or_done;
        return (flags, idle, Some(tail));
    }
    if blocked {
        // A blocked agent is judged by its status, not by its flags.
        return (BTreeSet::new(), false, Some(tail));
    }
    // A working agent: only a provider back-off is of interest (T-75).
    let mut flags = BTreeSet::new();
    if let Some(profile) = profile {
        if profile.backoff(&tail) {
            flags.insert(ScreenFlag::Backoff);
        }
        // A guardrails refusal (T-113) is read on every screen that is read:
        // the agent goes on after it, so the screen is the only place it is
        // ever visible.
        if profile.blocked(&tail) {
            flags.insert(ScreenFlag::Blocked);
        }
    }
    (flags, false, Some(tail))
}

/// Keep each seat's binding session equal to the one its agent reports (T-97).
///
/// An agent restarted by hand in its pane is a new session with the same
/// generation: the store must say so, because the record of a delivery that was
/// never confirmed is compared with the session it was sent into. Only a
/// present agent with a session value updates the binding; a kind that reports
/// none leaves it as it is.
///
/// The same step keeps the two records [`is_another_agent`] reads: the
/// repository of a present agent, recorded once per generation, and the mark
/// of a pane that another agent holds (set while it is there, cleared once the
/// seat's agent is seen again).
pub fn record_sessions(
    store: &mut Store,
    herdr: &mut dyn HerdrRead,
    observed: &[SeatObs],
) -> GearResult<()> {
    let mut cwds = None;
    let mut homes: Vec<(String, i64, String)> = Vec::new();
    for obs in observed {
        if obs.presence != Presence::Present
            || obs
                .session
                .as_deref()
                .is_none_or(|session| session.is_empty())
        {
            continue;
        }
        let key = home_key(&obs.seat.id, obs.seat.generation);
        if store
            .read(|tx| tx.get_meta(&key))?
            .is_some_and(|home| !home.is_empty())
        {
            continue;
        }
        let Some(binding) = store.read(|tx| tx.get_binding(&obs.seat.id, obs.seat.generation))?
        else {
            continue;
        };
        if let Some(cwd) = pane_cwd(herdr, &mut cwds, &binding.pane_id) {
            homes.push((
                obs.seat.id.clone(),
                obs.seat.generation,
                repo_identity(&cwd),
            ));
        }
    }
    // What changes, read first: most ticks change nothing and take no write.
    enum Change {
        Clear(String),
        Mark(String, String),
        Session(String, i64, String),
    }
    let changes: Vec<Change> = store.read(|tx| {
        let mut changes = Vec::new();
        for obs in observed {
            let foreign = foreign_key(&obs.seat.id, obs.seat.generation);
            let session = obs.session.as_ref().filter(|s| !s.is_empty());
            match obs.presence {
                Presence::Present => {
                    if is_foreign(tx, &obs.seat.id, obs.seat.generation)? {
                        changes.push(Change::Clear(foreign));
                    }
                    let Some(session) = session else { continue };
                    let current = tx
                        .get_binding(&obs.seat.id, obs.seat.generation)?
                        .and_then(|binding| binding.session_ref);
                    if current.as_deref() != Some(session.as_str()) {
                        changes.push(Change::Session(
                            obs.seat.id.clone(),
                            obs.seat.generation,
                            session.clone(),
                        ));
                    }
                }
                Presence::Changed => {
                    if let Some(session) = session {
                        if tx.get_meta(&foreign)?.as_deref() != Some(session.as_str()) {
                            changes.push(Change::Mark(foreign, session.clone()));
                        }
                    }
                }
                Presence::Missing | Presence::NotExpected => {}
            }
        }
        Ok(changes)
    })?;
    if homes.is_empty() && changes.is_empty() {
        return Ok(());
    }
    store.write(|tx| {
        for (seat, generation, home) in &homes {
            tx.set_meta(&home_key(seat, *generation), home)?;
        }
        for change in &changes {
            match change {
                Change::Clear(key) => tx.set_meta(key, "")?,
                Change::Mark(key, session) => tx.set_meta(key, session)?,
                Change::Session(seat, generation, session) => {
                    tx.set_binding_session_ref(seat, *generation, session)?
                }
            }
        }
        Ok(())
    })
}

/// States considered "may receive a doorbell/delivery".
pub fn reachable(state: SeatState) -> bool {
    matches!(
        state,
        SeatState::Active | SeatState::Standby | SeatState::Retiring
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_model::SeatRole;
    use gear_store::testing;

    const NOW: Ts = 1_800_000_000;
    const PANE: &str = "w1:p3";
    const SEAT: &str = "pharos:captain";

    use gear_model::Ts;

    fn repo(dir: &std::path::Path, name: &str) -> String {
        let path = dir.join(name);
        std::fs::create_dir_all(&path).unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&path)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git init {name}");
        path.to_string_lossy().into_owned()
    }

    /// The pharos captain, a codex agent with session `s1`, seen once in
    /// `home` so its repository is recorded.
    fn seated(home: &str) -> (Store, FakeHerdr) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "pharos", "sess", NOW)?;
                testing::seed_active_seat(
                    tx,
                    "pharos",
                    "captain",
                    SeatRole::Captain,
                    PANE,
                    "codex",
                    NOW,
                )?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::default();
        herdr.add_agent(PANE, "codex", AgentStatus::Idle);
        herdr.set_agent_session(PANE, "s1");
        herdr.set_pane_cwd(PANE, home);
        let observed = observe(&mut store, &mut herdr);
        assert_eq!(observed.presence, Presence::Present);
        (store, herdr)
    }

    fn observe(store: &mut Store, herdr: &mut FakeHerdr) -> SeatObs {
        let agents = herdr.agents().unwrap();
        let observed = store
            .read(|tx| reconcile(tx, herdr, "pharos", &agents, &BTreeMap::new()))
            .unwrap();
        record_sessions(store, herdr, &observed).unwrap();
        observed.into_iter().next().unwrap()
    }

    fn session_ref(store: &mut Store) -> Option<String> {
        store
            .read(|tx| tx.get_binding(SEAT, 1))
            .unwrap()
            .unwrap()
            .session_ref
    }

    fn foreign(store: &mut Store) -> bool {
        store.read(|tx| is_foreign(tx, SEAT, 1)).unwrap()
    }

    /// pharos, 2026-11-16: another project's codex agent in the captain's old
    /// pane is not the captain. Nothing is recorded as the seat's session, and
    /// the pane is marked so nothing is typed into it.
    #[test]
    fn another_agent_of_the_same_cli_in_the_pane_is_not_the_seat_agent() {
        let dir = tempfile::tempdir().unwrap();
        let home = repo(dir.path(), "pharos");
        let other = repo(dir.path(), "platform");
        let (mut store, mut herdr) = seated(&home);
        assert_eq!(session_ref(&mut store).as_deref(), Some("s1"));

        herdr.set_agent_session(PANE, "s9");
        herdr.set_pane_cwd(PANE, &other);
        let observed = observe(&mut store, &mut herdr);
        assert_eq!(observed.presence, Presence::Changed);
        assert_eq!(session_ref(&mut store).as_deref(), Some("s1"));
        assert!(foreign(&mut store), "the pane is marked");

        // The next tick says the same and keeps the mark.
        let observed = observe(&mut store, &mut herdr);
        assert_eq!(observed.presence, Presence::Changed);
        assert!(foreign(&mut store));
    }

    /// T-97 stays: `/new` or a hand restart of the seat's own agent is a new
    /// session in the same repository (a worktree of it too), and the seat's
    /// agent stays present with its new session recorded.
    #[test]
    fn a_new_session_of_the_seat_agent_in_its_repository_stays_present() {
        let dir = tempfile::tempdir().unwrap();
        let home = repo(dir.path(), "pharos");
        let (mut store, mut herdr) = seated(&home);
        let sub = std::path::Path::new(&home).join("crates");
        std::fs::create_dir_all(&sub).unwrap();

        herdr.set_agent_session(PANE, "s2");
        herdr.set_pane_cwd(PANE, &sub.to_string_lossy());
        let observed = observe(&mut store, &mut herdr);
        assert_eq!(observed.presence, Presence::Present);
        assert_eq!(session_ref(&mut store).as_deref(), Some("s2"));
        assert!(!foreign(&mut store));
    }

    /// The seat's agent back in its pane clears the mark.
    #[test]
    fn the_seat_agent_back_in_its_pane_clears_the_mark() {
        let dir = tempfile::tempdir().unwrap();
        let home = repo(dir.path(), "pharos");
        let other = repo(dir.path(), "platform");
        let (mut store, mut herdr) = seated(&home);

        herdr.set_agent_session(PANE, "s9");
        herdr.set_pane_cwd(PANE, &other);
        observe(&mut store, &mut herdr);
        assert!(foreign(&mut store));

        herdr.set_agent_session(PANE, "s1");
        herdr.set_pane_cwd(PANE, &home);
        let observed = observe(&mut store, &mut herdr);
        assert_eq!(observed.presence, Presence::Present);
        assert!(!foreign(&mut store));
    }

    /// Nothing known about the directory: the seat's agent stays present, as
    /// before (a kind of check that cannot answer never takes a seat away).
    #[test]
    fn a_pane_with_no_directory_keeps_the_seat_agent() {
        let dir = tempfile::tempdir().unwrap();
        let home = repo(dir.path(), "pharos");
        let (mut store, _) = seated(&home);
        let mut herdr = FakeHerdr::default();
        herdr.add_agent(PANE, "codex", AgentStatus::Idle);
        herdr.set_agent_session(PANE, "s5");
        let observed = observe(&mut store, &mut herdr);
        assert_eq!(observed.presence, Presence::Present);
        assert!(!foreign(&mut store));
    }
}
