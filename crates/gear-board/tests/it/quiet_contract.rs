//! T-384: the board does no work while nothing changes.
//!
//! The board keeps the board it built and the store's `data_version`, so a
//! refresh that finds nothing changed redraws what it has instead of asking the
//! store every view's queries again. Herdr's agent list is asked for at most
//! every ten seconds. The builds are counted through [`hook`], the two
//! intervals are shortened by it, and the frames are counted in the output the
//! board writes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use gear_board::{hook, run_with, Event, Keys, Options};
use gear_herdr::{
    fake::FakeHerdr, AgentInfo, AgentStatus, HerdrRead, ObserveError, PaneProcessInfo,
};
use gear_model::{SeatRole, TaskState};
use gear_store::{testing, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// A project with a captain, one lane and two tasks.
fn seed(store: &mut Store) {
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(
                tx,
                "p",
                "captain",
                SeatRole::Captain,
                "w1:p1",
                "claude",
                T0,
            )?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            for (id, rank, state) in [("T-1", 1, TaskState::Running), ("T-2", 2, TaskState::Ready)]
            {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: id.into(),
                    rank,
                    state,
                    priority: "must".into(),
                    lane: Some("data".into()),
                    milestone: Some("M5".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
            }
            Ok(())
        })
        .unwrap();
}

fn options(refresh: Duration) -> Options {
    Options {
        project: Some("p".into()),
        color: false,
        refresh,
        ..Options::default()
    }
}

/// A keyboard that lets `periods` refresh periods time out and then quits. Each
/// period waits the whole timeout, as a terminal does, so the wall clock moves
/// with the periods.
struct Beat {
    periods: usize,
}

impl Keys for Beat {
    fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        if self.periods == 0 {
            return Some(Event::Key('q'));
        }
        self.periods -= 1;
        std::thread::sleep(timeout);
        None
    }

    fn size(&self) -> (usize, usize) {
        (100, 30)
    }
}

/// The keys of a test, in order; `q` when they run out.
struct Events(Vec<Event>);

impl Keys for Events {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.0.is_empty() {
            return Some(Event::Key('q'));
        }
        Some(self.0.remove(0))
    }

    fn size(&self) -> (usize, usize) {
        (100, 30)
    }
}

/// A keyboard that writes to the store from a second connection in its first
/// period: another writer commits while the board holds the store it reads.
struct Writer {
    path: PathBuf,
    wrote: bool,
}

impl Keys for Writer {
    fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        if !self.wrote {
            self.wrote = true;
            let mut other = Store::open(&self.path).unwrap();
            other.write(|tx| tx.set_meta("board", "wrote")).unwrap();
            std::thread::sleep(timeout);
            return None;
        }
        Some(Event::Key('q'))
    }

    fn size(&self) -> (usize, usize) {
        (100, 30)
    }
}

/// Herdr as the board sees it, counting the agent-list asks. It hands out a
/// different list after the first ask when `moves` is set.
struct Counting {
    herdr: FakeHerdr,
    asks: usize,
    moves: bool,
}

impl Counting {
    fn new(moves: bool) -> Counting {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p2", "opencode", AgentStatus::Working);
        Counting {
            herdr,
            asks: 0,
            moves,
        }
    }
}

impl HerdrRead for Counting {
    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError> {
        self.asks += 1;
        if self.moves && self.asks == 2 {
            self.herdr.add_agent("w1:p1", "claude", AgentStatus::Idle);
        }
        self.herdr.agents()
    }

    fn agent_get(&mut self, _pane_id: &str) -> Result<Option<AgentInfo>, ObserveError> {
        unimplemented!("the board only reads the agent list")
    }

    fn read_tail(&mut self, _pane_id: &str, _lines: u32) -> Result<String, ObserveError> {
        unimplemented!("the board only reads the agent list")
    }

    fn read_screen(&mut self, _pane_id: &str) -> Result<String, ObserveError> {
        unimplemented!("the board only reads the agent list")
    }

    fn pane_process_info(&mut self, _pane_id: &str) -> Result<PaneProcessInfo, ObserveError> {
        unimplemented!("the board only reads the agent list")
    }

    fn panes(&mut self) -> Result<Vec<String>, ObserveError> {
        unimplemented!("the board only reads the agent list")
    }

    fn agent_names(&mut self) -> Result<BTreeMap<String, Option<String>>, ObserveError> {
        unimplemented!("the board only reads the agent list")
    }
}

/// The frames the board drew, in order.
///
/// A frame opens a synchronized update and homes the cursor (T-403); the
/// frames are told apart there.
fn frames(out: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(out);
    text.split("\u{1b}[?2026h\u{1b}[H")
        .skip(1)
        .map(str::to_string)
        .collect()
}

#[test]
fn ten_refresh_periods_build_the_board_once() {
    hook::reset();
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store);

    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &options(Duration::from_millis(1)),
        &mut out,
        &mut Beat { periods: 10 },
        None,
    )
    .unwrap();

    assert_eq!(
        hook::builds(),
        1,
        "nothing changed: one board, ten refreshes"
    );
    // The board is not redrawn either: the frame the ten refreshes build says
    // what the first one said, so it is not written again (T-403: rewriting
    // the same frame is what made the page blink).
    assert_eq!(
        frames(&out).len(),
        1,
        "the first frame, and no refresh writes one"
    );
}

#[test]
fn a_write_from_another_connection_builds_the_board_again() {
    hook::reset();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut store = Store::open(&path).unwrap();
    seed(&mut store);

    let mut out: Vec<u8> = Vec::new();
    let mut keys = Writer { path, wrote: false };
    run_with(
        &mut store,
        &options(Duration::from_millis(2)),
        &mut out,
        &mut keys,
        None,
    )
    .unwrap();

    assert_eq!(
        hook::builds(),
        2,
        "the first frame, and one rebuild after the other writer's commit"
    );
}

#[test]
fn a_key_press_redraws_without_building_the_board() {
    hook::reset();
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store);

    // The graph, then one row down, then quit: the tab and the selection are
    // redrawn from the kept board. The second `Down` has nowhere to go on two
    // tasks and writes no frame at all (T-403).
    let events = vec![Event::Key('2'), Event::Down, Event::Down, Event::Key('q')];
    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &options(Duration::from_millis(1)),
        &mut out,
        &mut Events(events),
        None,
    )
    .unwrap();

    assert_eq!(
        hook::builds(),
        1,
        "the keys redrew the board, they did not build it"
    );
    let drawn = frames(&out);
    assert_eq!(
        drawn.len(),
        3,
        "the first frame and one redraw per change: {drawn:?}"
    );
    assert_ne!(
        drawn[0], drawn[2],
        "the last frame shows the moved selection"
    );
}

#[test]
fn an_old_board_is_built_again_once() {
    hook::reset();
    hook::set_stale_after(Duration::from_millis(25));
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store);

    // One period of 40 ms is longer than the 25 ms a kept board is trusted.
    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &options(Duration::from_millis(40)),
        &mut out,
        &mut Beat { periods: 1 },
        None,
    )
    .unwrap();

    assert_eq!(
        hook::builds(),
        2,
        "the first frame, then one rebuild of an old board"
    );
}

#[test]
fn herdr_is_asked_for_its_agents_at_most_every_interval() {
    hook::reset();
    hook::set_agents_every(Duration::from_millis(25));
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store);

    let mut herdr = Counting::new(false);
    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &options(Duration::from_millis(40)),
        &mut out,
        &mut Beat { periods: 1 },
        Some(&mut herdr),
    )
    .unwrap();

    assert_eq!(
        herdr.asks, 2,
        "asked at the first frame and once the interval passed, not per refresh"
    );
    assert_eq!(hook::builds(), 1, "the list did not change: no rebuild");
}

#[test]
fn a_changed_agent_list_builds_the_board_again() {
    hook::reset();
    hook::set_agents_every(Duration::from_millis(25));
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store);

    let mut herdr = Counting::new(true);
    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &options(Duration::from_millis(40)),
        &mut out,
        &mut Beat { periods: 1 },
        Some(&mut herdr),
    )
    .unwrap();

    assert_eq!(herdr.asks, 2);
    assert_eq!(
        hook::builds(),
        2,
        "the agent list moved: the board was built again"
    );
}

/// A keyboard that reports another size after the first frame, waits the
/// period out and then quits.
struct Resize {
    size: (usize, usize),
    asked: bool,
}

impl Keys for Resize {
    fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        if self.asked {
            return Some(Event::Key('q'));
        }
        self.asked = true;
        self.size = (90, 20);
        std::thread::sleep(timeout);
        None
    }

    fn size(&self) -> (usize, usize) {
        self.size
    }
}

#[test]
fn a_resize_draws_once_over_a_full_clear() {
    hook::reset();
    let mut store = Store::open_in_memory().unwrap();
    seed(&mut store);

    let mut out: Vec<u8> = Vec::new();
    run_with(
        &mut store,
        &options(Duration::from_millis(1)),
        &mut out,
        &mut Resize {
            size: (100, 30),
            asked: false,
        },
        None,
    )
    .unwrap();

    // The frame under the new size is drawn all the same, and the terminal is
    // cleared once for it (T-403): the resize threw the old one away.
    let drawn = frames(&out);
    assert_eq!(
        drawn.len(),
        2,
        "the first frame and the frame of the new size"
    );
    let text = String::from_utf8_lossy(&out);
    assert_eq!(
        text.matches("\u{1b}[2J").count(),
        1,
        "the resize clears the screen once: {text:?}"
    );
}

/// The keys of a board with no project named: `p` twice — one full cycle over
/// two projects — then a third project registered from a second connection,
/// then `q`. The registration waits out the refresh period so the board sees
/// it on the next frame (T-384 rework, F1).
struct Registers {
    path: PathBuf,
    call: usize,
}

impl Keys for Registers {
    fn next_event(&mut self, timeout: Duration) -> Option<Event> {
        self.call += 1;
        match self.call {
            1 | 2 => Some(Event::Key('p')),
            3 => {
                let mut other = Store::open(&self.path).unwrap();
                other
                    .write(|tx| testing::seed_project(tx, "c", "sess", T0))
                    .unwrap();
                std::thread::sleep(timeout);
                None
            }
            _ => Some(Event::Key('q')),
        }
    }

    fn size(&self) -> (usize, usize) {
        (100, 30)
    }
}

/// T-384 (review F1): two `p` presses with two projects show the first one
/// again, and a third project registered behind the board's back leaves it
/// there — the cycle counts the projects shown, not the presses made.
#[test]
fn a_project_registered_later_does_not_move_the_shown_one() {
    hook::reset();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");
    let mut store = Store::open(&path).unwrap();
    store
        .write(|tx| {
            for (project, pane) in [("a", "w1:p1"), ("b", "w1:p2")] {
                testing::seed_project(tx, project, "sess", T0)?;
                testing::seed_active_seat(
                    tx,
                    project,
                    "data",
                    SeatRole::Lane,
                    pane,
                    "opencode",
                    T0,
                )?;
            }
            Ok(())
        })
        .unwrap();

    let mut out: Vec<u8> = Vec::new();
    let mut keys = Registers { path, call: 0 };
    let opts = Options {
        project: None,
        color: false,
        refresh: Duration::from_millis(2),
        ..Options::default()
    };
    run_with(&mut store, &opts, &mut out, &mut keys, None).unwrap();

    assert_eq!(
        hook::builds(),
        2,
        "the first frame, and one rebuild for the new project"
    );
    let drawn = frames(&out);
    // The title row of a frame, up to the clock at its end. T-353's head names
    // the shown project only: the number of the session's projects is no
    // longer on the row.
    let names = |frame: &str, project: &str| {
        let title = frame.lines().next().unwrap_or_default();
        title.contains(&format!("GEAR · {project} "))
    };
    // a, b, and a again — two presses over two projects complete the cycle.
    assert!(names(&drawn[1], "b"), "the second project: {drawn:?}");
    assert!(
        names(&drawn[2], "a"),
        "the cycle is back at the first: {drawn:?}"
    );
    // The registration rebuilds once, and the board still shows `a`.
    assert!(
        names(drawn.last().unwrap(), "a"),
        "still the first project: {drawn:?}"
    );
}
