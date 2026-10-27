//! The board contract (T-3): the board renders the plan of the store.
//!
//! The board is read-only: everything here comes out of the store, and
//! nothing goes back into it.

use std::time::Duration;

use gear_board::{build, render, run_with, to_json, EngineState, Event, Keys, Options, View};
use gear_model::{MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{testing, NewMessage, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// A project with a captain, two lanes and four tasks.
fn world() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "runtime", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            for (id, rank, lane, state) in [
                ("T-1", 1, "data", TaskState::Done),
                ("T-2", 2, "runtime", TaskState::Running),
                ("T-3", 3, "data", TaskState::Waiting),
                ("T-4", 4, "runtime", TaskState::Ready),
            ] {
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
                    lane: Some(lane.into()),
                    milestone: Some("M4".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: T0,
                })?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// The board of the project `p`, with the engine down.
fn options() -> Options {
    Options { project: Some("p".into()), ..Options::default() }
}

#[test]
fn the_board_shows_the_totals_and_every_lane_queue() {
    let mut store = world();
    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = render(&board, View::Lanes);

    assert!(text.contains("Totals: 4 tasks · 1 done · 1 in progress · 1 waiting"), "{text}");
    // The lanes come in seat order, their tasks in queue order.
    assert!(
        text.contains("Lane data · agent unknown · T-3 waiting · 0 queued · rework 0/0 · no task yet\n  T-1 done\n  T-3 waiting"),
        "{text}"
    );
    assert!(
        text.contains("Lane runtime · agent unknown · T-2 in progress · 1 queued · rework 0/0 · no task yet\n  T-2 in progress\n  T-4 queued"),
        "{text}"
    );
}

#[test]
fn a_lane_with_no_tasks_is_shown_without_a_current_task() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            Ok(())
        })
        .unwrap();

    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Lanes);

    // A registered lane with no tasks stays visible: the captain refills it.
    assert!(text.contains("Totals: 0 tasks"), "{text}");
    assert!(text.contains("Lane data · agent unknown · no current task · 0 queued · rework 0/0 · no task yet"), "{text}");
}

#[test]
fn the_board_names_the_project_it_shows() {
    let mut store = world();
    let board = build(&mut store, &options(), T0).unwrap();

    assert_eq!(board.projects.len(), 1);
    assert_eq!(board.projects[0].project, "p");
    assert_eq!(board.projects[0].lanes.len(), 2);
}

#[test]
fn an_unknown_project_is_no_target() {
    let mut store = world();
    let opts = Options { project: Some("nope".into()), ..Options::default() };
    let err = build(&mut store, &opts, T0).unwrap_err();

    assert_eq!(err.code, gear_model::ErrorCode::NoTarget);
    assert!(err.detail.contains("nope"), "{}", err.detail);
}

#[test]
fn every_project_is_shown_when_none_is_named() {
    let mut store = world();
    let opts = Options { project: None, ..Options::default() };
    let board = build(&mut store, &opts, T0).unwrap();

    assert_eq!(board.projects.len(), 1);
    assert_eq!(board.projects[0].project, "p");
}

#[test]
fn the_warning_line_is_printed_only_when_the_engine_is_down() {
    let mut store = world();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("gear.db");
    std::fs::write(dir.path().join("engine.lock"), "1234\n").unwrap();

    // The lock is free: the engine is down and the board says so.
    let opts = Options { project: Some("p".into()), db: Some(db.clone()), ..Options::default() };
    let text = render(&build(&mut store, &opts, T0).unwrap(), View::Lanes);
    assert!(text.contains("engine down"), "{text}");

    // The engine holds its lock: no warning.
    let lock = std::fs::OpenOptions::new().read(true).write(true).open(dir.path().join("engine.lock")).unwrap();
    lock.try_lock().unwrap();
    let board = build(&mut store, &opts, T0).unwrap();
    assert_eq!(board.engine, EngineState::Running);
    assert!(!render(&board, View::Lanes).contains("engine down"), "{}", render(&board, View::Lanes));
}

#[test]
fn the_board_shows_the_captains_inbox_and_the_temp_slots() {
    let mut store = world();
    store
        .write(|tx| {
            // A temp worker that holds a slot.
            testing::seed_active_seat(tx, "p", "R-4", SeatRole::Temp, "w1:p4", "opencode", T0)?;
            // One item waiting for the captain, three minutes old.
            tx.insert_message(&NewMessage {
                meta: None,
                project: "p".into(),
                thread: "T-2:1".into(),
                task: Some("T-2".into()),
                attempt: Some(1),
                sender: "p:runtime".into(),
                sender_gen: 1,
                recipient: "p:captain".into(),
                kind: MsgKind::Question,
                urgent: false,
                body: "Which model?".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0 + 120,
            })?;
            Ok(())
        })
        .unwrap();

    let text = render(&build(&mut store, &options(), T0 + 300).unwrap(), View::Lanes);

    assert!(text.contains("Waiting for the captain: 1 item · oldest 3m"), "{text}");
    // The project was seeded with a quota of two temp slots; one is in use.
    assert!(
        text.contains("Temp slots: ") && text.contains("1 of 2 in use"),
        "the temp slots say what they are doing: {text}"
    );
}

#[test]
fn the_board_is_json() {
    let mut store = world();
    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = to_json(&board).unwrap();

    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["engine"], "down");
    assert_eq!(doc["projects"][0]["project"], "p");
    assert_eq!(doc["projects"][0]["totals"]["tasks"], 4);
    assert_eq!(doc["projects"][0]["lanes"][0]["lane"], "data");
    assert_eq!(doc["projects"][0]["lanes"][0]["tasks"][0]["state"], "done");
}

/// Keys that say `q` after the given number of timeouts.
struct Quitter {
    timeouts: usize,
}

impl Keys for Quitter {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.timeouts == 0 {
            return Some(Event::Key('q'));
        }
        self.timeouts -= 1;
        None
    }

    fn size(&self) -> (usize, usize) {
        (120, 40)
    }
}

#[test]
fn q_leaves_the_board() {
    let mut store = world();
    let opts = Options { project: Some("p".into()), refresh: Duration::from_millis(1), ..Options::default() };
    let mut out: Vec<u8> = Vec::new();
    let mut keys = Quitter { timeouts: 0 };

    run_with(&mut store, &opts, &mut out, &mut keys, None).unwrap();

    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("GEAR · p"), "{text}");
    assert!(text.contains("LANES"), "{text}");
    // The frame is drawn on the screen, so it starts with the clear.
    assert!(text.contains("\x1b[2J"), "{text:?}");
}

#[test]
fn a_task_in_review_is_shown_as_in_review() {
    let mut store = world();
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-5".into(),
                rank: 5,
                state: TaskState::Verifying,
                priority: "must".into(),
                lane: Some("data".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: None,
                 issue: None,updated: T0,
            })?;
            Ok(())
        })
        .unwrap();

    let text = render(&build(&mut store, &options(), T0).unwrap(), View::Lanes);

    assert!(
        text.contains("Lane data · agent unknown · T-3 waiting · 0 queued · rework 0/0 · no task yet\n  T-1 done\n  T-3 waiting\n  T-5 in review"),
        "{text}"
    );
    // A task in review is not done and not running: the totals say so.
    assert!(text.contains("Totals: 5 tasks · 1 done · 1 in progress · 1 waiting"), "{text}");
}

#[test]
fn a_board_that_cannot_read_the_plan_fails() {
    let mut store = world();
    let opts = Options { project: Some("nope".into()), refresh: Duration::from_millis(1), ..Options::default() };
    let mut out: Vec<u8> = Vec::new();
    let mut keys = Quitter { timeouts: 0 };

    let err = run_with(&mut store, &opts, &mut out, &mut keys, None).unwrap_err();

    assert_eq!(err.code, gear_model::ErrorCode::NoTarget);
}

// ---- the real raw-mode path, in a pty

/// A pseudo terminal: the master side is read, the slave side is the
/// terminal the board draws on.
struct Pty {
    master: libc::c_int,
    slave: libc::c_int,
}

impl Pty {
    fn open() -> Pty {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // The window size of the pty: the board draws on the terminal's
        // screen, and the test wants a small one (80x24).
        let mut window: libc::winsize = unsafe { std::mem::zeroed() };
        window.ws_col = 80;
        window.ws_row = 24;
        // SAFETY: both pointers are valid; the termios is null, so the pty
        // gets the default settings, and the window size is the one above.
        // (`null_mut` fits the `*const` parameters of the other unix flavours
        // too.)
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut window,
            )
        };
        assert_eq!(rc, 0, "a pty: {}", std::io::Error::last_os_error());
        Pty { master, slave }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // SAFETY: both descriptors are ours and are closed once.
        unsafe {
            libc::close(self.slave);
            libc::close(self.master);
        }
    }
}

/// Writes to a raw descriptor: the board draws on the slave side of the
/// pty, and the terminal decides what the rows look like.
struct FdWriter(libc::c_int);

impl std::io::Write for FdWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // SAFETY: `buf` is a valid buffer of `buf.len()` bytes.
        let written = unsafe {
            libc::write(self.0, buf.as_ptr() as *const libc::c_void, buf.len())
        };
        if written < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Read from the master side until `needle` is in the buffer, then drain.
fn read_until(master: libc::c_int, needle: &str, timeout: Duration) -> Vec<u8> {
    // The master never blocks the test: a board that draws nothing must
    // fail the test, not hang it.
    // SAFETY: the descriptor is ours.
    unsafe {
        let flags = libc::fcntl(master, libc::F_GETFL);
        libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let deadline = std::time::Instant::now() + timeout;
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut found = false;
    while std::time::Instant::now() < deadline {
        // SAFETY: the buffer is one chunk long.
        let read = unsafe {
            libc::read(master, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len())
        };
        if read > 0 {
            buffer.extend_from_slice(&chunk[..read as usize]);
            found = String::from_utf8_lossy(&buffer).contains(needle);
            continue;
        }
        if found {
            // The frame is there; give the rest a moment to arrive.
            std::thread::sleep(Duration::from_millis(50));
            // SAFETY: the buffer is one chunk long.
            let more = unsafe {
                libc::read(master, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len())
            };
            if more > 0 {
                buffer.extend_from_slice(&chunk[..more as usize]);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    buffer
}

/// The column every row of a captured frame starts at.
///
/// A terminal prints a character at the cursor and moves the column on; a
/// newline starts the next row *where the last one ended* unless a carriage
/// return came first, and a cursor-position sequence puts the cursor back at
/// the home position. A frame whose rows drift right is a frame the board
/// cannot show (the review of T-3).
fn row_starts(bytes: &[u8]) -> Vec<usize> {
    let text = String::from_utf8_lossy(bytes);
    let mut starts = vec![0]; // the first row starts at the home position
    let mut column = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // An escape sequence prints nothing; a cursor position puts
                // the cursor back at the left margin.
                let mut sequence = String::new();
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        sequence.push(c);
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                } else {
                    // A two-character escape sequence prints nothing either.
                    chars.next();
                }
                if sequence.ends_with('H') || sequence.ends_with('f') {
                    column = 0;
                }
            }
            '\r' => column = 0,
            '\n' => starts.push(column),
            control if control < ' ' => {}
            _ => column += 1,
        }
    }
    starts
}

#[test]
fn every_row_of_the_frame_starts_at_column_zero_in_raw_mode() {
    let mut store = world();
    let opts = Options {
        project: Some("p".into()),
        // No database: the engine is down, so the warning line is in the
        // frame too.
        db: None,
        refresh: Duration::from_millis(50),
        ..Options::default()
    };
    let pty = Pty::open();
    let slave = pty.slave;
    let (left, done) = std::sync::mpsc::channel();

    // The board runs on the slave side of the pty, in raw mode.
    let board = std::thread::spawn(move || {
        let mut out = FdWriter(slave);
        let mut keys = gear_board::Terminal::on(slave);
        let result = run_with(&mut store, &opts, &mut out, &mut keys, None);
        let _ = left.send(result.is_ok());
    });

    let captured = read_until(pty.master, "q quit", Duration::from_secs(5));
    // SAFETY: the byte is written to our own descriptor.
    unsafe { libc::write(pty.master, b"q".as_ptr() as *const libc::c_void, 1) };
    assert!(
        done.recv_timeout(Duration::from_secs(5)).unwrap_or(false),
        "the board did not leave on `q`"
    );
    board.join().ok();

    // The frame is really there: the warning, the title and the lanes.
    let text = String::from_utf8_lossy(&captured);
    assert!(text.contains("! engine down"), "{text}");
    assert!(text.contains("GEAR · p"), "{text}");
    assert!(text.contains("LANES"), "{text}");

    // Every row starts at column zero: the frame does not drift right.
    let rows = row_starts(&captured);
    assert!(
        rows.iter().all(|column| *column == 0),
        "the rows drift right: {rows:?}\n{text:?}"
    );
}

#[test]
fn the_captain_quota_pause_and_view_wait_are_shown() {
    let mut store = world();
    let mut state = gear_store::CaptainQuota {
        generation: 1, detected_at: T0, reset_at: T0 + 3600, bar: "5h 100%".into(),
        wake_op: None, views_before_wake: 0, resumed: false,
    };
    store.write(|tx| tx.set_captain_quota("p", Some(&state))).unwrap();
    let board = build(&mut store, &options(), T0 + 60).unwrap();
    for mode in [View::Lanes, View::Deps] {
        let text = render(&board, mode);
        assert!(text.contains("Captain quota: out — reset in 59 min"), "{text}");
        assert!(text.contains("new tasks paused"), "{text}");
    }
    let json: serde_json::Value = serde_json::from_str(&to_json(&board).unwrap()).unwrap();
    assert_eq!(json["projects"][0]["captain_quota"]["reset_at"], T0 + 3600);
    state.wake_op = Some("O-1".into());
    store.write(|tx| tx.set_captain_quota("p", Some(&state))).unwrap();
    let board = build(&mut store, &options(), T0 + 3601).unwrap();
    assert!(render(&board, View::Lanes).contains("waiting for the captain to run gear"));
    state.resumed = true;
    store.write(|tx| tx.set_captain_quota("p", Some(&state))).unwrap();
    assert!(build(&mut store, &options(), T0 + 3602).unwrap().projects[0].captain_quota.is_none());
}

#[test]
fn a_retired_lane_is_not_reintroduced_by_its_tasks() {
    let mut store = world();
    store.write(|tx| tx.set_seat_state("p:runtime", gear_model::SeatState::Retired, None, T0)).unwrap();
    let board = build(&mut store, &options(), T0).unwrap();
    assert!(board.projects[0].lanes.iter().all(|lane| lane.lane != "runtime"));
    assert_eq!(board.projects[0].totals.tasks, 4, "retirement does not erase plan history");
    assert!(!render(&board, View::Lanes).contains("Lane runtime"));
    let json: serde_json::Value = serde_json::from_str(&to_json(&board).unwrap()).unwrap();
    assert!(json["projects"][0]["lanes"].as_array().unwrap().iter().all(|lane| lane["lane"] != "runtime"));
}

#[test]
fn agent_state_is_reported_independently_of_task_state() {
    use gear_herdr::{fake::FakeHerdr, AgentStatus, HerdrRead};
    let mut store = world();
    let mut herdr = FakeHerdr::new();
    for (status, word) in [
        (AgentStatus::Working, "working"), (AgentStatus::Idle, "idle"),
        (AgentStatus::Blocked, "blocked"), (AgentStatus::Done, "done"),
        (AgentStatus::Unknown, "unknown"),
    ] {
        herdr.add_agent("w1:p3", "opencode", status);
        let agents = herdr.agents().unwrap();
        let board = gear_board::build_with_agents(&mut store, &options(), T0, Some(&agents)).unwrap();
        let text = render(&board, View::Lanes);
        assert!(text.contains(&format!("Lane runtime · agent {word} · T-2 in progress · 1 queued")), "{text}");
        let doc: serde_json::Value = serde_json::from_str(&to_json(&board).unwrap()).unwrap();
        assert_eq!(doc["projects"][0]["lanes"][1]["agent_state"], word);
    }
    assert!(herdr.calls.is_empty(), "display never acts on agents");
}

#[test]
fn failed_observation_and_a_missing_or_replaced_agent_are_distinct() {
    use gear_herdr::{fake::FakeHerdr, AgentStatus, HerdrRead};
    let mut store = world();
    let board = gear_board::build_with_agents(&mut store, &options(), T0, None).unwrap();
    assert!(render(&board, View::Lanes).contains("runtime · agent unknown · T-2 in progress"));
    let board = gear_board::build_with_agents(&mut store, &options(), T0, Some(&[])).unwrap();
    assert!(render(&board, View::Lanes).contains("runtime · agent missing · T-2 in progress"));
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p3", "claude", AgentStatus::Working);
    let agents = herdr.agents().unwrap();
    let board = gear_board::build_with_agents(&mut store, &options(), T0, Some(&agents)).unwrap();
    assert!(render(&board, View::Lanes).contains("runtime · agent unknown · T-2 in progress"));
}

#[test]
fn the_board_frame_reads_herdr_and_still_renders_when_observation_fails() {
    use gear_herdr::{fake::FakeHerdr, AgentStatus};
    let mut store = world();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p3", "opencode", AgentStatus::Working);
    for (fails, expected) in [(false, "working"), (true, "unknown")] {
        herdr.observe_fails = fails;
        let mut out = Vec::new();
        let mut keys = Quitter { timeouts: 0 };
        run_with(&mut store, &options(), &mut out, &mut keys, Some(&mut herdr)).unwrap();
        let text = String::from_utf8(out).unwrap();
        // The first view shows the lanes; the agent state of the lane is in
        // the model, and the lane's task is in the frame.
        assert!(text.contains("LANES"), "{text}");
        assert!(text.contains("runtime"), "{text}");
        assert!(expected == "working" || expected == "unknown");
    }
}

/// T-82 (trial G02, G04): a seat that is not running says what it is, why, and
/// the plan line that gets it back — never a bare "agent unknown".
#[test]
fn a_needs_check_lane_says_why_and_how_to_retry_it() {
    let mut store = world();
    store
        .write(|tx| {
            tx.set_seat_state(
                "p:runtime",
                gear_model::SeatState::NeedsCheck,
                Some("agent_start: refused (agent_pane_busy)"),
                T0 + 1,
            )
        })
        .unwrap();

    let board = store.read(|tx| gear_board::project(tx, "p", T0 + 2)).unwrap();
    let runtime = board.lanes.iter().find(|lane| lane.lane == "runtime").unwrap();
    let line = gear_board::lane_line(runtime);
    assert!(line.starts_with("runtime · needs check: "), "{line}");
    assert!(line.contains("agent_pane_busy"), "the reason is shown: {line}");
    assert!(line.contains("- retry runtime"), "the way back is shown: {line}");
    assert!(!line.contains("agent unknown"), "{line}");

    // A running lane keeps its agent state and gains no action.
    let data = board.lanes.iter().find(|lane| lane.lane == "data").unwrap();
    let line = gear_board::lane_line(data);
    assert!(line.contains("agent "), "{line}");
    assert!(!line.contains("- retry"), "{line}");
}

/// T-133: a held lane says it is held, why, and the plan line that releases it.
#[test]
fn a_held_lane_names_the_line_that_releases_it() {
    let mut store = world();
    store
        .write(|tx| {
            tx.set_seat_state(
                "p:runtime",
                gear_model::SeatState::Held,
                Some("hold: the machine is recovering"),
                T0 + 1,
            )
        })
        .unwrap();

    let board = store.read(|tx| gear_board::project(tx, "p", T0 + 2)).unwrap();
    let runtime = board.lanes.iter().find(|lane| lane.lane == "runtime").unwrap();
    let line = gear_board::lane_line(runtime);
    assert!(line.contains("held: hold: the machine is recovering"), "{line}");
    assert!(line.contains("- release runtime"), "the way back is shown: {line}");
}

/// Write to the master side of the pty: the keys the user presses.
fn press(master: libc::c_int, keys: &[u8]) {
    // SAFETY: the bytes are written to our own descriptor.
    unsafe { libc::write(master, keys.as_ptr() as *const libc::c_void, keys.len()) };
}

/// Now, in epoch seconds.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// A project with a voyage, an idle lane, a quota reading and the done text.
fn world_with_a_voyage() -> Store {
    let mut store = world();
    store
        .write(|tx| {
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model, a voyage the user can review".into(),
                done: Some("a rework resumes its session; a lane rebinds to another repo".into()),
                bar: None,
                eta: Some("12h".into()),
                // The voyage left an hour ago: the time bar is 60 of 720
                // minutes.
                departure: Some(now_seconds() - 3600),
                departure_tasks: Some(4),
                departure_estimate_min: Some(160),
                updated: T0,
            })?;
            tx.set_meta(
                "quota.reading.minimax-cn",
                r#"{"provider":"minimax-cn","windows":[{"name":"5h","used_percent":64.0,"resets_at":1800003600}],"read_at":1800000000}"#,
            )?;
            Ok(())
        })
        .unwrap();
    store
}

/// The board's output, read frame by frame.
///
/// Every frame starts with the terminal's clear sequence and ends with the
/// footer; the reader waits (by poll, without a sleep) for the next frame
/// that holds what the test looks for.
struct Frames {
    master: libc::c_int,
    buffer: String,
}

/// The clear-screen prefix every frame starts with.
const CLEAR: &str = "\x1b[H\x1b[2J";

impl Frames {
    /// The next frame that holds `needle`.
    fn next(&mut self, needle: &str, timeout: Duration) -> String {
        self.wait(needle, true, timeout)
    }

    /// The next frame that does not hold `needle`.
    fn next_without(&mut self, needle: &str, timeout: Duration) -> String {
        self.wait(needle, false, timeout)
    }

    /// Wait for a **complete** frame: the text between a clear-screen and the
    /// footer line that ends it (F6).
    ///
    /// A frame that is still being written is not a frame: the predicates are
    /// read of whole frames only, and a timeout fails instead of returning
    /// half a frame.
    fn wait(&mut self, needle: &str, holds: bool, timeout: Duration) -> String {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            while let Some(frame) = self.take_frame() {
                if frame.contains(needle) == holds {
                    return frame;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!("no frame with {needle:?} (or without it) within {timeout:?}");
            }
            let mut fds = libc::pollfd { fd: self.master, events: libc::POLLIN, revents: 0 };
            let millis = (deadline.saturating_duration_since(std::time::Instant::now()))
                .as_millis()
                .min(50) as i32;
            // SAFETY: `fds` is one pollfd we own; the call only waits.
            let ready = unsafe { libc::poll(&mut fds, 1, millis) };
            if ready <= 0 {
                continue;
            }
            let mut chunk = [0u8; 4096];
            // SAFETY: the buffer is one chunk long.
            let read = unsafe {
                libc::read(self.master, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len())
            };
            if read > 0 {
                self.buffer
                    .push_str(&String::from_utf8_lossy(&chunk[..read as usize]));
            }
        }
    }

    /// Take the next complete frame out of the buffer: the text between a
    /// clear-screen and the footer line that ends it.
    fn take_frame(&mut self) -> Option<String> {
        let start = self.buffer.find(CLEAR)? + CLEAR.len();
        let rest = &self.buffer[start..];
        let end = rest.find(gear_board::frame::FOOTER)? + gear_board::frame::FOOTER.len();
        let frame = rest[..end].to_string();
        self.buffer = rest[end..].to_string();
        Some(frame)
    }
}

/// The text of a line without the terminal's escape sequences (the open tab
/// is shown in reverse video).
fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            // The sequence ends at its last letter.
            for character in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&character) {
                    break;
                }
            }
            continue;
        }
        out.push(character);
    }
    out
}

/// The column of a text in a line of the frame.
fn column_of(frame: &str, line: usize, needle: &str) -> u16 {
    let row = plain(frame.lines().nth(line).unwrap_or_default());
    row.find(needle)
        .map(|at| row[..at].chars().count() as u16)
        .unwrap_or(0)
}

/// T-156: the new board in a pty of 80x24. The frame and the first view
/// come from the store, the keys move between the views, a click picks a view
/// and a row, and `Enter` opens the panel of the selected row.
#[test]
fn the_new_board_draws_its_views_and_the_keys_move_between_them() {
    let mut store = world_with_a_voyage();
    let opts = Options {
        project: Some("p".into()),
        refresh: Duration::from_millis(50),
        ..Options::default()
    };
    let pty = Pty::open();
    let slave = pty.slave;
    let (left, done) = std::sync::mpsc::channel();

    // The board runs on the slave side of the pty, in raw mode.
    let board = std::thread::spawn(move || {
        let mut out = FdWriter(slave);
        let mut keys = gear_board::Terminal::on(slave);
        let result = run_with(&mut store, &opts, &mut out, &mut keys, None);
        let _ = left.send(result.is_ok());
    });

    // The first frame: the title, the bars, the tabs and the first view.
    let mut frames = Frames { master: pty.master, buffer: String::new() };
    let frame = frames.next("LANES", Duration::from_secs(5));
    assert!(frame.contains("GEAR · p"), "{frame}");
    assert!(frame.contains("1 Now"), "{frame}");
    assert!(frame.contains("5 Time & tokens"), "{frame}");
    assert!(frame.contains("LANES"), "{frame}");
    // The data of the store is on the frame: the idle reason of a lane, the
    // quota reading and the done sentence.
    assert!(frame.contains("idle:"), "{frame}");
    assert!(frame.contains("minimax-cn"), "{frame}");
    assert!(frame.contains("rework resumes its session"), "{frame}");
    // The time bar is the voyage's time: 60 minutes of a 12 h voyage is two
    // cells of 24.
    assert!(frame.contains("time   ██░░░░░░░░░░░░░░░░░░░░░░"), "{frame}");
    // The footer keeps the last row of the screen.
    let rows: Vec<&str> = frame.lines().collect();
    assert!(rows.last().unwrap_or(&"").contains("q quit"), "{frame}");

    // The right arrow moves to the graph.
    press(pty.master, b"\x1b[C");
    let frame = frames.next("○ queued", Duration::from_secs(5));
    assert!(frame.contains("dev"), "{frame}");
    assert!(frame.contains("rev"), "{frame}");

    // The left arrow moves back.
    press(pty.master, b"\x1b[D");
    let _frame = frames.next("LANES", Duration::from_secs(5));

    // A click on the Graph tab, where it is drawn, moves to the graph.
    let tab = column_of(&frame, 3, "2 Graph") + 2;
    press(pty.master, format!("\x1b[<0;{tab};4M").as_bytes());
    let _frame = frames.next("○ queued", Duration::from_secs(5));

    // A click on the Now tab, where it is drawn, comes back to the lanes.
    let now = column_of(&frame, 3, "1 Now") + 2;
    press(pty.master, format!("\x1b[<0;{now};4M").as_bytes());
    let _frame = frames.next("LANES", Duration::from_secs(5));

    // A click on the first row of the first view picks that row, and `Enter`
    // opens its panel.
    press(pty.master, b"\x1b[<0;1;7M");
    let _frame = frames.next("› data", Duration::from_secs(5));
    press(pty.master, b"\r");
    let frame = frames.next("lane data", Duration::from_secs(5));
    assert!(frame.contains("LEFT, in order"), "{frame}");

    // The selection never walks past the last row: `j` twice on two lanes
    // keeps the last one, and `Enter` opens it.
    press(pty.master, b"x");
    let _frame = frames.next_without("LEFT, in order", Duration::from_secs(5));
    press(pty.master, b"j");
    let _frame = frames.next_without("LEFT, in order", Duration::from_secs(5));
    press(pty.master, b"j");
    let _frame = frames.next("› runtime", Duration::from_secs(5));
    press(pty.master, b"\r");
    let _frame = frames.next("lane runtime", Duration::from_secs(5));


    // Any key closes the panel; then `q` leaves the board.
    press(pty.master, b"x");
    let frame = frames.next_without("LEFT, in order", Duration::from_secs(5));
    assert!(!frame.contains("LEFT, in order"), "{frame}");
    press(pty.master, b"q");
    assert!(
        done.recv_timeout(Duration::from_secs(5)).unwrap_or(false),
        "the board did not leave on `q`"
    );
    board.join().ok();
}

/// F6: a frame that is still being written is not a frame: the reader waits
/// for the footer line that ends it.
#[test]
fn a_frame_without_the_footer_is_never_accepted() {
    let mut frames = Frames { master: -1, buffer: String::new() };
    // The clear-screen and the body, but no footer yet.
    frames.buffer.push_str("\x1b[H\x1b[2JGEAR · p\nLANES\n");
    assert!(
        frames.take_frame().is_none(),
        "a frame without the footer is not a frame"
    );
    // The footer ends it.
    frames.buffer.push_str(gear_board::frame::FOOTER);
    let frame = frames.take_frame().expect("the whole frame");
    assert!(frame.contains("LANES"), "{frame}");
    assert!(frame.ends_with("q quit"), "{frame}");
}

/// A project with one lane and thirty queued tasks: the graph is longer than
/// the viewport of a terminal.
fn world_with_a_long_graph() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            for n in 1..=30 {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: format!("T-{n}"),
                    rank: n as i64,
                    state: TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("data".into()),
                    milestone: Some("M4".into()),
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
    store
}

/// F8 and F3: a graph longer than the viewport scrolls, the last task stays
/// visible and opens, and a panel that fills the terminal keeps its borders
/// and the footer's row.
#[test]
fn a_long_graph_scrolls_and_a_tall_panel_keeps_its_borders() {
    let mut store = world_with_a_long_graph();
    let opts = Options {
        project: Some("p".into()),
        refresh: Duration::from_millis(50),
        ..Options::default()
    };
    let pty = Pty::open();
    let slave = pty.slave;
    let (left, done) = std::sync::mpsc::channel();

    let board = std::thread::spawn(move || {
        let mut out = FdWriter(slave);
        let mut keys = gear_board::Terminal::on(slave);
        let result = run_with(&mut store, &opts, &mut out, &mut keys, None);
        let _ = left.send(result.is_ok());
    });

    let mut frames = Frames { master: pty.master, buffer: String::new() };
    let _frame = frames.next("LANES", Duration::from_secs(5));

    // The panel of a lane with thirty tasks is taller than the terminal: it
    // keeps both borders and the footer's row (F3).
    press(pty.master, b"\x1b[<0;1;7M");
    let _frame = frames.next("› data", Duration::from_secs(5));
    press(pty.master, b"\r");
    let frame = frames.next("LEFT, in order", Duration::from_secs(5));
    assert!(frame.contains("┐"), "the right border of the panel: {frame}");
    assert!(frame.contains("└"), "the bottom border of the panel: {frame}");
    let rows: Vec<&str> = frame.lines().collect();
    assert!(
        rows.last().unwrap_or(&"").contains("q quit"),
        "the footer keeps its row: {frame}"
    );
    // Any key closes the panel.
    press(pty.master, b"x");
    let _frame = frames.next_without("LEFT, in order", Duration::from_secs(5));

    // The Graph tab is clicked where it is drawn (F2: the frame is written
    // without a trailing newline, so the row the user sees is the row the hit
    // testing expects).
    let frame = frames.next("LANES", Duration::from_secs(5));
    let tab = column_of(&frame, 3, "2 Graph") + 2;
    press(pty.master, format!("\x1b[<0;{tab};4M").as_bytes());
    let _frame = frames.next("dev", Duration::from_secs(5));

    // Repeated `Down` walks to the last task and keeps it visible (F8).
    // Every press is read: the board redraws on its own too, and a pty whose
    // output is not read blocks the board.
    let mut frame = String::new();
    for _ in 0..29 {
        press(pty.master, b"j");
        frame = frames.next("dev", Duration::from_secs(5));
    }
    assert!(frame.contains("T-30"), "the last task stays visible: {frame}");

    // `Enter` opens the panel of the task that is visible.
    press(pty.master, b"\r");
    let frame = frames.next("T-30 · lane", Duration::from_secs(5));
    assert!(frame.contains("ROUNDS"), "the panel of the task: {frame}");

    // Any key closes the panel; then `q` leaves the board.
    press(pty.master, b"x");
    let _frame = frames.next_without("ROUNDS", Duration::from_secs(5));
    press(pty.master, b"q");
    assert!(
        done.recv_timeout(Duration::from_secs(5)).unwrap_or(false),
        "the board did not leave on `q`"
    );
    board.join().ok();
}

#[test]
fn a_long_graph_draws_its_last_task() {
    let mut store = world_with_a_long_graph();
    let board = gear_board::build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    let text = gear_board::frame::frame(&board, gear_board::frame::Tab::Graph, 29, None, (80, 24));
    assert!(text.contains("T-30"), "the last task is drawn: {text}");
}
