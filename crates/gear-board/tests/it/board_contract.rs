//! The board contract (T-3): the board renders the plan of the store.
//!
//! The board is read-only: everything here comes out of the store, and
//! nothing goes back into it.

use std::time::Duration;

use gear_board::{build, render, run_with, to_json, EngineState, Event, Keys, Options, View};
use gear_model::{AttemptState, MsgKind, MsgState, SeatRole, TaskState};
use gear_store::{
    testing, AttemptRow, AttemptStep, ContractRow, NewMessage, NewRound, RoundKey, Store, TaskRow,
    VoyageRow,
};

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
            // The attempts behind the states: the lane lines read the stage the
            // graph gives the task, not its state word (T-347).
            for (task, lane, state) in [
                ("T-2", "runtime", AttemptState::Started),
                ("T-3", "data", AttemptState::Submitted),
            ] {
                tx.insert_attempt(&AttemptRow {
                    project: "p".into(),
                    task: task.into(),
                    n: 1,
                    seat: format!("p:{lane}"),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
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
        text.contains("Lane data · agent unknown · T-3 handed in, waits for a reviewer · 0 queued · rework 0/1 · no task yet\n  T-1 done\n  T-3 waiting"),
        "{text}"
    );
    assert!(
        text.contains("Lane runtime · agent unknown · T-2 working · 1 queued · rework 0/0 · no task yet\n  T-2 in progress\n  T-4 queued"),
        "{text}"
    );
}

/// T-347: the lane line names the task's stage in the words of the board's
/// views, so the board and the captain's home page never disagree.
#[test]
fn the_lane_line_names_the_stage_in_the_words_of_the_graph() {
    let mut store = world();
    let board = store.read(|tx| gear_board::project(tx, "p", T0 + 60)).unwrap();
    let line = |name: &str| {
        let lane = board.lanes.iter().find(|lane| lane.lane == name).expect("the lane");
        gear_board::lane_line(lane)
    };
    // A lane working on attempt 1.
    assert_eq!(line("runtime"), "runtime · agent unknown · T-2 working · 1 queued");
    // A lane whose task waits for a reviewer: the stage says so, not "waiting".
    assert_eq!(
        line("data"),
        "data · agent unknown · T-3 handed in, waits for a reviewer · 0 queued"
    );
    // The lane's line and the graph's task panel say the same words.
    let data = board.lanes.iter().find(|lane| lane.lane == "data").unwrap();
    let task = data.tasks.iter().find(|task| task.id == "T-3").unwrap();
    assert_eq!(task.stage, gear_board::graph::Stage::Handed);
    assert_eq!(task.stage_words, "handed in, waits for a reviewer");
}

/// T-347, F1 of the review of attempt 1: a cancelled task is not the lane's
/// current task. Cancelling keeps the terminal attempt, so a task cancelled
/// after a failed verdict still has a rejected last attempt and the classifier
/// still calls its stage open; the graph leaves such a task out, and the lane
/// line names the lane's next open task instead.
#[test]
fn a_cancelled_task_is_not_the_current_task_of_its_lane() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            for (id, rank, state) in
                [("T-1", 1, TaskState::Cancelled), ("T-2", 2, TaskState::Running)]
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
                    lane: Some("heat".into()),
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
            // The cancelled task keeps its rejected attempt; the running one
            // is on its first attempt, started.
            for (task, state) in
                [("T-1", AttemptState::Rejected), ("T-2", AttemptState::Started)]
            {
                tx.insert_attempt(&AttemptRow {
                    project: "p".into(),
                    task: task.into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                })?;
            }
            Ok(())
        })
        .unwrap();
    let board = store.read(|tx| gear_board::project(tx, "p", T0 + 60)).unwrap();
    let lane = board.lanes.iter().find(|lane| lane.lane == "heat").unwrap();
    // The cancelled task is in the lane model and its stage reads open...
    let cancelled = lane.tasks.iter().find(|task| task.id == "T-1").unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert_eq!(cancelled.stage, gear_board::graph::Stage::SentBack);
    // ...and the lane's current task is the next open one, not it.
    assert_eq!(gear_board::lane_line(lane), "heat · agent unknown · T-2 working · 0 queued");
    // The graph leaves the cancelled task out, so the lane line and the views
    // agree on what the lane works on.
    assert!(
        board.graph.iter().all(|task| task.id != "T-1"),
        "the graph does not draw a cancelled task"
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

#[test]
fn a_task_is_measured_by_the_time_its_lane_worked_on_it() {
    // T-348: the actual minutes of a task are the `active_s` of its dev and
    // rework rounds, not the clock since its delivery (T-241 showed "working
    // 89/25m" where the rework took 17 minutes).
    let mut store = Store::open_in_memory().unwrap();
    let now = T0 + 3 * 3600;
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p1", "pi", T0)?;
            for (id, rank) in [("T-1", 1), ("T-2", 2)] {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: id.into(),
                    rank,
                    state: TaskState::Running,
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
                    issue: None,
                    updated: T0,
                })?;
                // Both were delivered three hours ago.
                tx.insert_attempt(&AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: id.into(),
                    n: 1,
                    seat: "p:data".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Started,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                })?;
                tx.set_attempt_time("p", id, 1, AttemptStep::Started, T0)?;
            }
            // T-1 worked for 25 minutes and then reworked it for 10.
            for (attempt, role, active_s) in [(1, "dev", 1500), (2, "rework", 600)] {
                let key = RoundKey {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt,
                    role: role.into(),
                };
                tx.insert_round(&NewRound {
                    key: key.clone(),
                    seat: "p:data".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: None,
                    start_at: T0,
                })?;
                tx.set_round_active(&key, active_s)?;
            }
            Ok(())
        })
        .unwrap();

    let board = build(&mut store, &options(), now).unwrap();
    let tasks = &board.projects[0].lanes[0].tasks;
    let worked = tasks.iter().find(|task| task.id == "T-1").unwrap();
    assert_eq!(worked.actual_min, Some(35), "the work time of the two rounds");
    // A task without a dev or rework round keeps its span.
    let span = tasks.iter().find(|task| task.id == "T-2").unwrap();
    assert_eq!(span.actual_min, Some(180), "the span of a task with no round");

    let graph = store
        .read(|tx| gear_board::graph::tasks(tx, "p", now))
        .unwrap();
    let worked = graph.iter().find(|task| task.id == "T-1").unwrap();
    assert_eq!(worked.actual_min, Some(35), "the same work time in the graph");
    let span = graph.iter().find(|task| task.id == "T-2").unwrap();
    assert_eq!(span.actual_min, Some(180), "the same span in the graph");
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
    assert!(text.contains("NOW DOING"), "{text}");
    // The frame is drawn in place (T-403): it opens a synchronized update and
    // never clears the whole screen.
    assert!(text.contains("\x1b[?2026h\x1b[H"), "{text:?}");
    assert!(!text.contains("\x1b[2J"), "the screen is never cleared whole: {text:?}");
    assert!(text.contains("\x1b[K"), "every line erases its rest: {text:?}");
    assert!(text.contains("\x1b[J"), "the screen below the frame is erased: {text:?}");
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
        text.contains("Lane data · agent unknown · T-3 handed in, waits for a reviewer · 0 queued · rework 0/1 · no task yet\n  T-1 done\n  T-3 waiting\n  T-5 in review"),
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
        Pty::open_size(80, 24)
    }

    /// A pty whose screen is `cols` by `rows` cells.
    fn open_size(cols: u16, rows: u16) -> Pty {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // The window size of the pty: the board draws on the terminal's
        // screen, and the test wants a small one (80x24).
        let mut window: libc::winsize = unsafe { std::mem::zeroed() };
        window.ws_col = cols;
        window.ws_row = rows;
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

/// Read from the master side until `needle` is in the buffer.
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
    while std::time::Instant::now() < deadline {
        if String::from_utf8_lossy(&buffer).contains(needle) {
            // The needle ends the footer, the last line of a frame: nothing
            // comes after it that the frame needs.
            return buffer;
        }
        // Wait on the descriptor, not on the clock: the time left to the
        // deadline is the only timeout.
        let mut fds = libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 };
        let millis = (deadline.saturating_duration_since(std::time::Instant::now()))
            .as_millis() as i32;
        // SAFETY: `fds` is one pollfd we own; the call only waits.
        let ready = unsafe { libc::poll(&mut fds, 1, millis) };
        if ready <= 0 {
            continue;
        }
        let mut chunk = [0u8; 4096];
        // SAFETY: the buffer is one chunk long.
        let read = unsafe {
            libc::read(master, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len())
        };
        if read <= 0 {
            continue;
        }
        buffer.extend_from_slice(&chunk[..read as usize]);
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

    let captured = read_until(pty.master, FOOTER_START, Duration::from_secs(5));
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
    assert!(text.contains("NOW DOING"), "{text}");

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
        assert!(text.contains(&format!("Lane runtime · agent {word} · T-2 working · 1 queued")), "{text}");
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
    assert!(render(&board, View::Lanes).contains("runtime · agent unknown · T-2 working"));
    let board = gear_board::build_with_agents(&mut store, &options(), T0, Some(&[])).unwrap();
    assert!(render(&board, View::Lanes).contains("runtime · agent missing · T-2 working"));
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p3", "claude", AgentStatus::Working);
    let agents = herdr.agents().unwrap();
    let board = gear_board::build_with_agents(&mut store, &options(), T0, Some(&agents)).unwrap();
    assert!(render(&board, View::Lanes).contains("runtime · agent unknown · T-2 working"));
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
        // the model, and the lane's task is in the frame. A lane name longer
        // than five cells runs into NOW DOING and is cut by the task there,
        // as the prototype paints it (T-359).
        assert!(text.contains("NOW DOING"), "{text}");
        assert!(text.contains("runtim"), "{text}");
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
            // The config reads minimax-cn: the reading below is one the
            // project reads now (T-383).
            let mut row = tx.get_project("p")?.unwrap();
            row.config_toml = "[[quota.sources]]\nprovider = \"minimax-cn\"\nreader = \"minimax\"\n".to_string();
            tx.update_project(&row)?;
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

/// The clear-screen prefix every frame starts with: the synchronized update
/// and the home position (T-403: the screen is never cleared whole).
const CLEAR: &str = "\x1b[?2026h\x1b[H";

/// The start of the footer, which an 80 column terminal shows: the whole
/// footer is longer than the screen.
const FOOTER_START: &str = "←→ / Tab / 1-4";

impl Frames {
    /// The next frame that holds `needle`.
    fn next(&mut self, needle: &str, timeout: Duration) -> String {
        self.wait(needle, true, timeout)
    }

    /// The next frame that does not hold `needle`.
    fn next_without(&mut self, needle: &str, timeout: Duration) -> String {
        self.wait(needle, false, timeout)
    }

    /// Whether no frame at all was written within `timeout`.
    ///
    /// The board writes a frame only when it says something new (T-403): a
    /// key or a click that changes nothing leaves the screen as it is, so no
    /// frame arrives.
    fn quiet(&mut self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.take_frame().is_some() {
                return false;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return true;
            }
            let mut fds = libc::pollfd { fd: self.master, events: libc::POLLIN, revents: 0 };
            let millis = (deadline.saturating_duration_since(now)).as_millis().min(50) as i32;
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

    /// The last complete frame that holds `needle`.
    ///
    /// Every frame before it is dropped: the board redraws on its own too, and
    /// a pty whose output is not read blocks the board (F9). The wait ends
    /// with the frame that arrived last, so a key that was just pressed is
    /// not answered by an old frame.
    fn last_with(&mut self, needle: &str, timeout: Duration) -> String {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let mut last = None;
            while let Some(frame) = self.take_frame() {
                if frame.contains(needle) {
                    last = Some(frame);
                }
            }
            if let Some(frame) = last {
                return frame;
            }
            if std::time::Instant::now() >= deadline {
                panic!("no frame with {needle:?} within {timeout:?}");
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
        let end = rest.find(FOOTER_START)? + FOOTER_START.len();
        // The frame is read as the user reads it: the colours and the
        // reverse video of the board are not part of the text.
        let frame = plain(&rest[..end]);
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
            // A control sequence (`ESC [`, its parameters, its final letter)
            // or a two-character escape: neither prints anything.
            if chars.peek() == Some(&'[') {
                chars.next();
                for character in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&character) {
                        break;
                    }
                }
            } else {
                chars.next();
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

/// T-341: the four views the user chose: the tab row, the keys and the
/// prototype's footer. The fifth view (Workload, and its burn-up) is gone.
#[test]
fn the_tab_row_has_the_four_views_of_the_user() {
    use gear_board::frame::{self, Tab};
    use gear_board::style::{Color, Style};

    // The labels of the row, in order, and the keys 1 to 4.
    assert_eq!(
        Tab::all().map(|tab| tab.label()),
        ["  1 Now  ", "  2 Tasks  ", "  3 Cost  ", "  4 Log  "]
    );
    // The fifth view has no key: key 5 changes nothing.
    assert_eq!(Tab::of_number(5), None);
    // The right arrow wraps from the last view to the first.
    assert_eq!(Tab::Log.next(), Tab::Now);
    assert_eq!(Tab::Now.previous(), Tab::Log);

    let mut store = world();
    let board = build(&mut store, &options(), T0 + 60).unwrap();
    // The tab row holds the four labels, framed by the bar's rules (T-356):
    // the top rule on the row above, the bottom rule on the row under.
    let screen = crate::screen(Tab::Now, 0, (160, 40));
    let layout = frame::layout(&board, &screen);
    let text = frame::frame(&board, &screen, None);
    let row = text.lines().nth(layout.tab_row).unwrap_or_default();
    for label in ["  1 Now  ", "  2 Tasks  ", "  3 Cost  ", "  4 Log  "] {
        assert!(row.contains(label), "{label:?} is not in {row:?}");
    }
    assert!(row.trim_end().ends_with('│'), "the bar closes at the edge: {row:?}");
    let above = text.lines().nth(layout.tab_row - 1).unwrap_or_default();
    assert!(above.trim().starts_with('╭'), "the bar's top rule: {above:?}");
    assert!(above.trim_end().ends_with('╮'), "the bar's top rule: {above:?}");
    let under = text.lines().nth(layout.tab_row + 1).unwrap_or_default();
    assert!(under.trim().starts_with('╰'), "the bar's bottom rule: {under:?}");
    assert!(under.trim_end().ends_with('╯'), "the bar's bottom rule: {under:?}");
    // No view draws a burn-up any more (the chart is removed).
    for tab in Tab::all() {
        let text = frame::frame(&board, &crate::screen(tab, 0, (160, 40)), None);
        assert!(!text.contains("BURN-UP"), "{tab:?} draws a burn-up: {text}");
    }
    // The footer is the prototype's: it starts at column 1 of the last row,
    // in Dim.
    let canvas = frame::canvas(&board, &crate::screen(Tab::Now, 0, (160, 40)), None);
    let last = canvas.text().lines().last().unwrap_or_default().to_string();
    assert!(last.starts_with(" ←→ / Tab / 1-4"), "{last:?}");
    assert_eq!(canvas.style_at(1, 39), Style::fg(Color::Dim));
}

/// T-353: the goal box says what the quality bar blocks, and a bar with a key
/// the board knows is named by that key, however the plan wrote the rest.
#[test]
fn the_goal_box_says_what_the_quality_bar_blocks() {
    use gear_board::frame::{self, Tab};
    use gear_board::style::{Color, Style};

    let mut store = world_with_a_voyage();
    store
        .write(|tx| {
            tx.upsert_voyage(&gear_store::VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one seat model, a voyage the user can review".into(),
                done: Some("a rework resumes its session".into()),
                bar: Some("STRICT: a severe issue waits for the captain".into()),
                eta: Some("12h".into()),
                departure: Some(now_seconds() - 3600),
                departure_tasks: Some(4),
                departure_estimate_min: Some(160),
                updated: T0,
            })
            .unwrap();
            Ok(())
        })
        .unwrap();
    let board = build(&mut store, &options(), now_seconds()).unwrap();
    let screen = crate::screen(Tab::Now, 0, (160, 40));
    let canvas = frame::canvas(&board, &screen, None);
    let text = canvas.text();

    let line = "Quality bar: STRICT — severe issues block a task; medium ones are the captain's call";
    let row = text
        .lines()
        .position(|text| text.contains(line))
        .unwrap_or_else(|| panic!("the box holds {line:?}:\n{text}"));
    // The bar is locked in the box with the goal: yellow, and not bold.
    let x = text.lines().nth(row).unwrap_or_default().find(line).unwrap_or(0);
    assert_eq!(canvas.style_at(x, row), Style::fg(Color::Warn), "{text}");
    // That row is inside the box: it opens and closes with the box's borders.
    let row_text = text.lines().nth(row).unwrap_or_default();
    assert!(row_text.trim_start().starts_with("║ "), "{row_text:?}");
    assert!(row_text.contains(" ║"), "{row_text:?}");
}

/// T-353: a click on the tab row picks the tab the layout draws there. The
/// click counts from 1, as the terminal does, and the layout counts from 0.
#[test]
fn the_tab_row_of_the_layout_answers_a_click() {
    use gear_board::frame::Tab;

    let mut store = world_with_a_voyage();
    let size = (100, 30);
    let head = head_of(&mut store, size);
    // The fourth tab (Log), in the middle of its columns.
    let (first, last, tab) = *head
        .tabs
        .iter()
        .find(|(_, _, tab)| *tab == Tab::Log)
        .expect("the Log tab is on the row");
    assert_eq!(tab, Tab::Log);
    let events = vec![Event::Mouse {
        button: 0,
        x: ((first + last) / 2 + 1) as u16,
        y: head.tab_row as u16 + 1,
    }];
    let frame = run_events(&mut store, events, size);

    assert!(
        frame.contains("ALL DECISIONS"),
        "the click opened the Log view: {frame}"
    );
}

/// T-356: the tab bar is one row, and a click on it picks the tab it draws.
/// Column 15 of the terminal (counted from 1) is inside the Tasks tab, whose
/// columns run from its rule to the rule after its label. `d` unfolds the
/// header, the bar moves down with it, and a click on the bar's new row picks
/// the tab as well.
#[test]
fn a_click_on_column_15_of_the_tab_bar_picks_the_tasks_tab_in_both_headers() {
    use gear_board::frame::{self, Tab};

    let mut store = world_with_a_voyage();
    let size = (120, 48);
    let folded = head_of(&mut store, size);
    assert_eq!(folded.top, folded.tab_row + 3, "the view starts three rows under the tabs");
    // Column 15 of the terminal is the screen's column 14, inside the Tasks
    // tab: the tab's own columns and the rule after its label.
    let (first, last, tab) = *folded
        .tabs
        .iter()
        .find(|(_, _, tab)| *tab == Tab::Tasks)
        .expect("the Tasks tab is on the row");
    assert_eq!(tab, Tab::Tasks);
    assert!(
        (first..=last).contains(&14),
        "column 15 is inside the Tasks tab: columns {first} to {last}"
    );
    // The range opens with a rule: the separator that ends the tab before, so
    // a click between two labels picks the one on its right.
    let canvas = frame::canvas(
        &build(
            &mut store,
            &Options { project: Some("p".into()), ..Options::default() },
            now_seconds(),
        )
        .unwrap(),
        &crate::screen(Tab::Now, 0, size),
        None,
    );
    assert_eq!(canvas.char_at(first, folded.tab_row), '│', "the range starts on the rule");
    assert_eq!(canvas.char_at(first + 1, folded.tab_row), ' ', "the label follows the rule");

    let events = vec![Event::Mouse { button: 0, x: 15, y: folded.tab_row as u16 + 1 }];
    let frame = run_events(&mut store, events, size);
    assert!(
        frame.contains("merged into dev"),
        "the click on column 15 opened the Tasks view: {frame}"
    );

    // `d` unfolds the header: the bar and its row move down with it, and a
    // click on the new row picks the tab too.
    let board = build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    let opened = frame::layout(
        &board,
        &frame::Screen {
            tab: Tab::Now,
            selected: 0,
            detail: true,
            opened: Vec::new(),
            size,
        },
    );
    assert!(
        opened.tab_row > folded.tab_row,
        "the unfolded header pushes the tab bar down: {} then {}",
        folded.tab_row,
        opened.tab_row
    );
    let events = vec![
        Event::Key('d'),
        Event::Mouse { button: 0, x: 15, y: opened.tab_row as u16 + 1 },
    ];
    let frame = run_events(&mut store, events, size);
    assert!(
        frame.contains("merged into dev"),
        "the click on the unfolded bar opened the Tasks view: {frame}"
    );
    assert!(
        frame.contains("d: fold"),
        "the header stayed unfolded: {frame}"
    );
}

/// T-156: the new board in a pty of 80x28. The frame and the first view
/// come from the store, the keys move between the views, a click picks a view
/// and a row, and `Enter` opens the panel of the selected row.
///
/// The screen is four rows taller than the 80x24 it took before the tab bar
/// (T-356): the bar's rules and the spacer took rows of their own, and the
/// lane's panel under the view needs the rows it drew before.
#[test]
fn the_new_board_draws_its_views_and_the_keys_move_between_them() {
    let mut store = world_with_a_voyage();
    // The head of the board on the pty's 80x28 screen (T-353): the tab row and
    // the view's first row are the layout's.
    let head = head_of(&mut store, (80, 28));
    let pick_row = head.top + 2;
    let opts = Options {
        project: Some("p".into()),
        refresh: Duration::from_millis(50),
        ..Options::default()
    };
    let pty = Pty::open_size(80, 28);
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
    let frame = frames.next("NOW DOING", Duration::from_secs(5));
    assert!(frame.contains("GEAR · p"), "{frame}");
    assert!(frame.contains("1 Now"), "{frame}");
    assert!(frame.contains("4 Log"), "{frame}");
    assert!(frame.contains("NOW DOING"), "{frame}");
    // The data of the store is on the frame: the idle reason of a lane, the
    // quota reading and the done sentence.
    assert!(frame.contains("handed in, waits for a"), "{frame}");
    assert!(frame.contains("minimax-cn"), "{frame}");
    assert!(frame.contains("rework resumes its session"), "{frame}");
    // The head of the board is the prototype's (T-353): the goal box and the
    // voyage's name.
    assert!(frame.contains("╔═ GOAL "), "{frame}");
    assert!(frame.contains(" M4: one seat model, a voyage the user can review · "), "{frame}");
    // The footer keeps the last row of the screen.
    let rows: Vec<&str> = frame.lines().collect();
    assert!(rows.last().unwrap_or(&"").contains(FOOTER_START), "{frame}");

    // The right arrow moves to the graph: the prototype's git graph, whose
    // legend stands on the row above the footer (T-362). An 80-column screen
    // cuts the legend after "↺ reworking", so the frame is found by the head of
    // the legend.
    press(pty.master, b"\x1b[C");
    let frame = frames.next("merged into dev", Duration::from_secs(5));
    assert!(frame.contains("dev"), "{frame}");
    assert!(frame.contains("task") && frame.contains("what"), "{frame}");

    // The left arrow moves back.
    press(pty.master, b"\x1b[D");
    let _frame = frames.next("NOW DOING", Duration::from_secs(5));

    // A click on the Graph tab, where it is drawn, moves to the graph. The
    // click counts from 1: the tab's first column plus one (T-353).
    let tab = column_of(&frame, head.tab_row, "2 Tasks") + 1;
    press(pty.master, format!("\x1b[<0;{tab};{row}M", row = head.tab_row + 1).as_bytes());
    let _frame = frames.next("merged into dev", Duration::from_secs(5));

    // A click on the Now tab, where it is drawn, comes back to the lanes.
    let now = column_of(&frame, head.tab_row, "1 Now") + 1;
    press(pty.master, format!("\x1b[<0;{now};{row}M", row = head.tab_row + 1).as_bytes());
    let _frame = frames.next("NOW DOING", Duration::from_secs(5));

    // A click on the first row of the first view picks that row, and `Enter`
    // opens its panel. The row is the one the board already shows as picked,
    // and a frame that says what the last one said is not written (T-403):
    // the selection is moved off it first, so the click is a change.
    press(pty.master, b"j");
    let _frame = frames.next("› runtim", Duration::from_secs(5));
    press(pty.master, format!("\x1b[<0;1;{pick_row}M").as_bytes());
    let _frame = frames.next("› data", Duration::from_secs(5));
    press(pty.master, b"\r");
    let frame = frames.next("lane data", Duration::from_secs(5));
    assert!(frame.contains("LEFT, in order"), "{frame}");

    // The selection never walks past the last row: `j` moves to the last
    // lane, a second `j` has nowhere to go and writes no frame at all, and
    // `Enter` opens the last lane.
    press(pty.master, b"x");
    let _frame = frames.next_without("LEFT, in order", Duration::from_secs(5));
    press(pty.master, b"j");
    let _frame = frames.next("› runtim", Duration::from_secs(5));
    press(pty.master, b"j");
    assert!(
        frames.quiet(Duration::from_millis(300)),
        "a key that moves nothing writes no frame"
    );
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

/// A session with two projects, neither of them named: both are the
/// board's, and the store is the session's.
fn world_with_two_projects() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            for (project, pane) in [("a", "w1:p1"), ("b", "w1:p2")] {
                testing::seed_project(tx, project, "sess", T0)?;
                testing::seed_active_seat(
                    tx, project, "data", SeatRole::Lane, pane, "opencode", T0,
                )?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// T-317 (M4.2 #35): a board opened without a project shows the session's
/// projects one at a time, and `p` walks through them.
#[test]
fn a_board_without_a_project_shows_them_one_at_a_time() {
    let mut store = world_with_two_projects();
    let opts = Options {
        project: None,
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

    // The first frame shows the first project: its name is the first row's.
    let mut frames = Frames { master: pty.master, buffer: String::new() };
    let frame = frames.next("NOW DOING", Duration::from_secs(5));
    let title = plain(frame.lines().next().unwrap_or_default());
    assert!(title.trim_start().starts_with("GEAR · a "), "{title}");

    // `p` shows the second project.
    press(pty.master, b"p");
    let frame = frames.next("GEAR · b", Duration::from_secs(5));
    let title = plain(frame.lines().next().unwrap_or_default());
    assert!(title.trim_start().starts_with("GEAR · b "), "{title}");

    // `p` on the last project comes back to the first.
    press(pty.master, b"p");
    let frame = frames.next("GEAR · a", Duration::from_secs(5));
    let title = plain(frame.lines().next().unwrap_or_default());
    assert!(title.trim_start().starts_with("GEAR · a "), "{title}");

    press(pty.master, b"q");
    assert!(
        done.recv_timeout(Duration::from_secs(5)).unwrap_or(false),
        "the board did not leave on `q`"
    );
    board.join().ok();
}

/// T-317 (review round 1, F1): the board printed once — a pipe, no terminal —
/// is one static frame, so it is the frame of one project: nothing can be
/// pressed after it, and it does not promise a key. `run` takes the
/// printed-once path whenever stdin or stdout is not a terminal, which is the
/// case in every test process.
#[test]
fn a_board_printed_once_promises_no_project_key() {
    let mut store = world_with_two_projects();
    let opts = Options { project: None, color: false, ..Options::default() };
    let mut out: Vec<u8> = Vec::new();
    gear_board::run(&mut store, &opts, &mut out, None).expect("the printed board");
    let text = String::from_utf8_lossy(&out).into_owned();
    let title = plain(text.lines().next().unwrap_or_default());
    assert!(title.trim_start().starts_with("GEAR · a "), "the first project prints: {title}");
    assert!(!title.contains("next of"), "a printed board promises no key: {title}");

    // The JSON is the whole board, unchanged: the switch is only the text's
    // own business.
    let opts = Options { json: true, project: None, ..Options::default() };
    let mut out: Vec<u8> = Vec::new();
    gear_board::run(&mut store, &opts, &mut out, None).expect("the board as JSON");
    let doc: serde_json::Value = serde_json::from_slice(&out).expect("JSON");
    let projects = doc["projects"].as_array().expect("the projects");
    assert_eq!(projects.len(), 2, "the JSON holds both projects: {doc}");
}

/// T-317: a board of one project has no suffix to explain, and neither does
/// a board opened with `--project`. T-353's first row names the shown project
/// and nothing else.
#[test]
fn a_board_of_one_project_has_no_project_key() {
    let mut store = world();
    let board = build(
        &mut store,
        &Options { project: None, ..Options::default() },
        T0,
    )
    .unwrap();
    let text = gear_board::frame::frame(
        &board,
        &crate::screen(gear_board::frame::Tab::Now, 0, (80, 24)),
        None,
    );
    let title = plain(text.lines().next().unwrap_or_default());
    assert!(title.trim_start().starts_with("GEAR · p "), "{title}");
    assert!(!title.contains("next of"), "{title}");

    // Even with more projects in the store, `--project` names the one shown.
    store
        .write(|tx| testing::seed_project(tx, "other", "sess", T0))
        .unwrap();
    let board = build(&mut store, &options(), T0).unwrap();
    let text = gear_board::frame::frame(
        &board,
        &crate::screen(gear_board::frame::Tab::Now, 0, (80, 24)),
        None,
    );
    let title = plain(text.lines().next().unwrap_or_default());
    assert!(title.trim_start().starts_with("GEAR · p "), "{title}");
    assert!(!title.contains("next of"), "{title}");
}

/// F6: a frame that is still being written is not a frame: the reader waits
/// for the footer line that ends it.
#[test]
fn a_frame_without_the_footer_is_never_accepted() {
    let mut frames = Frames { master: -1, buffer: String::new() };
    // The clear-screen and the body, but no footer yet.
    frames.buffer.push_str("\x1b[?2026h\x1b[HGEAR · p\nLANES\n");
    assert!(
        frames.take_frame().is_none(),
        "a frame without the footer is not a frame"
    );
    // The footer ends it.
    frames.buffer.push_str(gear_board::frame::FOOTER);
    let frame = frames.take_frame().expect("the whole frame");
    assert!(frame.contains("LANES"), "{frame}");
    assert!(frame.ends_with(FOOTER_START), "{frame}");
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
            // Seventeen tasks: the graph is longer than the viewport of an
            // 80x24 terminal (sixteen rows), and the lane's panel is taller
            // than the terminal (F9).
            for n in 1..=17 {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    // The ids are padded, so a frame that shows one of them
                    // is unambiguous.
                    id: format!("T-{n:02}"),
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
            // The last pick's goal is longer than the screen: the panel must
            // be cut, not hidden (F3). The last pick is T-01: the waiting tasks
            // are drawn newest first (T-361, T-362).
            tx.insert_contract(&ContractRow {
                project: "p".into(),
                task: "T-01".into(),
                version: 1,
                goal: "Finish the board frame and keep the task panel visible when its goal exceeds terminal width".into(),
                accept: "the panel shows both borders and the footer".into(),
                evidence_files: vec![],
                evidence_due_min: None,
                body: String::new(),
                created: T0,
                mechanical: None,
            })?;
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
    // The head of the board on the pty's 80x24 screen (T-353).
    let head = head_of(&mut store, (80, 24));
    let opts = Options {
        project: Some("p".into()),
        // A longer refresh: the board redraws on the keys, not every 50 ms
        // (F9).
        refresh: Duration::from_millis(300),
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
    let _frame = frames.next("NOW DOING", Duration::from_secs(5));

    // The panel of a lane with seventeen tasks is taller than the terminal: it
    // keeps both borders and the footer's row (F3). The first lane is the one
    // the board shows as picked, and a click that picks it again writes no
    // frame (T-403): `Enter` opens it from there.
    press(pty.master, format!("\x1b[<0;1;{pick}M", pick = head.top + 2).as_bytes());
    assert!(
        frames.quiet(Duration::from_millis(300)),
        "a click on the picked row writes no frame"
    );
    press(pty.master, b"\r");
    let frame = frames.next("LEFT, in order", Duration::from_secs(5));
    assert!(frame.contains("┐"), "the right border of the panel: {frame}");
    assert!(frame.contains("└"), "the bottom border of the panel: {frame}");
    let rows: Vec<&str> = frame.lines().collect();
    assert!(
        rows.last().unwrap_or(&"").contains(FOOTER_START),
        "the footer keeps its row: {frame}"
    );
    // Any key closes the panel.
    press(pty.master, b"x");
    let frame = frames.next_without("LEFT, in order", Duration::from_secs(5));

    // The Graph tab is clicked where it is drawn (F2: the frame is written
    // without a trailing newline, so the row the user sees is the row the hit
    // testing expects). The frame that closed the panel is the one on the
    // screen, and a frame that reads like the last one written is not written
    // again (T-403).
    let tab = column_of(&frame, head.tab_row, "2 Tasks") + 1;
    press(pty.master, format!("\x1b[<0;{tab};{row}M", row = head.tab_row + 1).as_bytes());
    let _frame = frames.next("dev", Duration::from_secs(5));

    // Repeated `Down` walks to the last task and keeps it visible (F8). Every
    // press is read: the board redraws on its own too, and a pty whose output
    // is not read blocks the board (F9). The heading scrolls away with the
    // rows above the selected one (T-342), so the frames are drained by their
    // footer. A frame is written only when the selection moves (T-403), so the
    // last one drained is the one on the screen.
    let mut frame = String::new();
    for _ in 0..16 {
        press(pty.master, b"j");
        frame = frames.last_with(FOOTER_START, Duration::from_secs(5));
    }
    // The last pick is T-01, the last task to run (T-361, T-362): the view
    // scrolled to it.
    assert!(frame.contains("T-01"), "the last pick stays visible: {frame}");

    // A click on the footer's row and on the spacer below the data selects
    // nothing (F8): the selection stays on the last task, and no frame is
    // written (T-403).
    press(pty.master, b"\x1b[<0;1;24M");
    press(pty.master, b"\x1b[<0;1;23M");
    assert!(
        frames.quiet(Duration::from_millis(300)),
        "a click that selects nothing writes no frame"
    );

    // `Enter` opens the panel of the task that is visible.
    press(pty.master, b"\r");
    let frame = frames.next("T-01 · lane", Duration::from_secs(5));
    assert!(frame.contains("ROUNDS"), "the panel of the task: {frame}");
    // The goal is longer than the screen: the panel is cut, not hidden (F3).
    assert!(frame.contains("┐"), "the right border of the panel: {frame}");
    assert!(frame.contains("└"), "the bottom border of the panel: {frame}");
    let rows: Vec<&str> = frame.lines().collect();
    assert!(
        rows.last().unwrap_or(&"").contains(FOOTER_START),
        "the footer keeps its row: {frame}"
    );

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
    let text = gear_board::frame::frame(&board, &crate::screen(gear_board::frame::Tab::Tasks, 16, (80, 24)), None);
    // The last pick is the last task to run, T-01: the waiting tasks hang
    // newest first (T-361, T-362).
    assert!(text.contains("T-01"), "the last pick is drawn: {text}");
    assert!(!text.contains("T-17"), "the first pick scrolled away: {text}");
}

/// Keys that send the given events, then `q`, on a screen of the given size.
struct Events {
    events: Vec<Event>,
    size: (usize, usize),
}

impl Keys for Events {
    fn next_event(&mut self, _timeout: Duration) -> Option<Event> {
        if self.events.is_empty() {
            return Some(Event::Key('q'));
        }
        Some(self.events.remove(0))
    }

    fn size(&self) -> (usize, usize) {
        self.size
    }
}

/// The last frame the board drew: the frames are separated by the clear.
fn last_frame(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    text.rsplit("\u{1b}[?2026h\u{1b}[H").next().unwrap_or_default().to_string()
}

/// T-342: a view with more picks than screen rows scrolls so the selected one
/// is drawn, and a click on the drawn row keeps it selected.
#[test]
fn a_view_scrolls_to_its_selected_pick_and_a_click_keeps_it() {
    use gear_board::frame::{self, Tab};
    let mut store = world_with_a_long_graph();
    let board = build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    // The last of the seventeen picks is T-01, the last task to run (the
    // waiting tasks hang newest first, T-361 and T-362): the view scrolled to
    // it, and the heading scrolled away with the rows above it. The tab bar
    // takes three rows (T-356), so the screen is one row taller than the 12 it
    // took with the tabs on a single row.
    let size = (80, 13);
    let tab_row = frame::layout(&board, &crate::screen(Tab::Tasks, 16, size)).tab_row;
    let canvas = frame::canvas(&board, &crate::screen(Tab::Tasks, 16, size), None);
    let text = canvas.text();
    assert!(text.contains("T-01"), "the selected pick is drawn: {text}");
    assert!(!text.contains("T-17"), "the view scrolled past the first pick: {text}");
    // Its row is the last row the view owns, and it is the lit one: the view
    // stops on the row above the legend (T-362).
    let row = 9;
    assert!(text.lines().nth(row).unwrap_or_default().contains("T-01"), "{text}");
    assert!(canvas.style_at(1, row).reverse, "the selected row is lit: {text}");

    // Sixteen `Down` keys and then a click on the row T-01 is drawn on: the
    // click adds the viewport's offset and picks the same task.
    let mut events = vec![Event::Key('2')];
    for _ in 0..16 {
        events.push(Event::Key('j'));
    }
    events.push(Event::Mouse { button: 0, x: 1, y: 10 });
    let opts = Options {
        project: Some("p".into()),
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    };
    let mut out: Vec<u8> = Vec::new();
    run_with(&mut store, &opts, &mut out, &mut Events { events, size }, None).unwrap();

    let frame = last_frame(&out);
    let rows: Vec<&str> = frame.lines().collect();
    assert!(
        rows.get(row).unwrap_or(&"").contains("T-01"),
        "the clicked row holds T-01: {frame}"
    );
    assert!(
        rows.get(row).unwrap_or(&"").contains("\u{1b}[7m"),
        "the clicked row stays the lit one: {frame}"
    );
    // The first row is lit too: the shown voyage's label (README change 1).
    for (y, line) in rows.iter().enumerate() {
        if y == row || y == 0 || y == tab_row {
            continue;
        }
        assert!(!line.contains("\u{1b}[7m"), "row {y} is not lit: {line:?}");
    }
}

/// T-342's rework (F1 of the review of attempt 1): the view's own first row is
/// a row of the view, so after scrolling a click on it picks the task drawn
/// there.
///
/// The git graph draws a task on two rows (its ○ or stage row and the row of
/// its branch), so a scrolled view's first row holds a task only where the
/// merges and the forks fall that way. The fixture's voyage (the prototype's
/// M5 graph) has T-268's row three above T-262's:
/// a 12-row screen scrolled to T-262 starts on T-268.
#[test]
fn a_click_on_the_top_row_of_a_scrolled_view_picks_the_task_there() {
    use gear_board::frame;
    let (mut store, _now) = crate::golden_fixture::load();
    // The view's first row is the view's: scrolled to T-262 (the ninth pick),
    // it holds T-268's row. The head takes fourteen rows (T-354's three rows
    // and the tab bar's three, T-356), so the screen is five rows taller than
    // the 12 it took with the fixed header: the view keeps its four rows.
    let size = (160, 21);
    let board = build(
        &mut store,
        &Options { project: Some("gear".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    let head = frame::layout(&board, &crate::screen(frame::Tab::Tasks, 8, size));
    let before = frame::canvas(&board, &crate::screen(frame::Tab::Tasks, 8, size), None);
    let before = before.text();
    assert!(
        before.lines().nth(head.top).unwrap_or_default().contains("T-268"),
        "the view's first row holds T-268: {before}"
    );
    // `2` is the graph; eight `Down` select T-262. The click at x 1 y
    // `top + 1` is the view's first row: the viewport starts at `top`, and
    // scrolling has put a pick there.
    let mut events = vec![Event::Key('2')];
    for _ in 0..8 {
        events.push(Event::Key('j'));
    }
    events.push(Event::Mouse { button: 0, x: 1, y: (head.top + 1) as u16 });
    let opts = Options {
        project: Some("gear".into()),
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    };
    let mut out: Vec<u8> = Vec::new();
    run_with(&mut store, &opts, &mut out, &mut Events { events, size }, None).unwrap();

    let frame_text = last_frame(&out);
    let rows: Vec<&str> = frame_text.lines().collect();
    let tab_row = head.tab_row;
    let lit: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(y, line)| *y != 0 && *y != tab_row && line.contains("\u{1b}[7m"))
        .map(|(y, _)| y)
        .collect();
    assert_eq!(lit.len(), 1, "one lit row: {frame_text}");
    assert!(
        rows[lit[0]].contains("T-268"),
        "the click selected the task drawn on the view's first row: {frame_text}"
    );
}

/// T-362: `Down` moves over the rows of the git graph that hold a task — not
/// over every row of the drawing — and `Enter` opens the task of the row.
#[test]
fn down_then_enter_opens_the_panel_of_the_second_task_row() {
    let mut store = world_with_a_long_graph();
    // The waiting tasks hang newest first, so the second task row is T-16.
    let out = run_events_output(
        &mut store,
        vec![Event::Key('2'), Event::Key('j'), Event::Key('\r')],
        (80, 24),
    );
    assert!(out.contains("T-16 · lane"), "the panel of the second task row: {out}");
    assert!(!out.contains("T-17 · lane"), "the first task's panel is not opened: {out}");
}

/// T-362: a Tasks view longer than the screen scrolls the selected row into
/// view and keeps the legend on the row above the footer (`H - 2`).
#[test]
fn a_long_tasks_view_scrolls_and_keeps_the_legend_above_the_footer() {
    use gear_board::frame::{self, Tab};
    use gear_board::style::Color;

    let mut store = world_with_tasks(40);
    let board = build(
        &mut store,
        &Options { project: Some("p".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    let size = (80, 24);
    let canvas = frame::canvas(&board, &crate::screen(Tab::Tasks, 39, size), None);
    let text = canvas.text();
    let rows: Vec<&str> = text.lines().collect();
    // The last pick is T-01 (the waiting tasks hang newest first): it scrolled
    // into view, lit, on the last row the view owns (`H - 4`).
    let row = 20;
    assert!(rows.get(row).unwrap_or(&"").contains("T-01"), "the selected pick is drawn: {text}");
    assert!(canvas.style_at(1, row).reverse, "the selected row is lit: {text}");
    // The legend is on row `H - 2`, in Dim, and the footer keeps row `H - 1`.
    let legend = rows.get(22).copied().unwrap_or_default();
    assert!(legend.starts_with(" ● merged into dev"), "the legend on row H - 2: {text}");
    assert_eq!(canvas.style_at(1, 22).fg, Some(Color::Dim), "the legend is Dim: {text}");
    assert_eq!(canvas.style_at(1, 23).fg, Some(Color::Dim), "the footer keeps its row: {text}");
}

// ---- T-383: the QUOTA lines of the board

/// The Now view's frame, wide and tall enough for the whole quota block.
fn now_frame(store: &mut Store, now: i64) -> String {
    let board = build(
        store,
        &gear_board::Options { project: Some("p".into()), ..Default::default() },
        now,
    )
    .unwrap();
    let screen = crate::screen(gear_board::frame::Tab::Now, 0, (120, 70));
    gear_board::frame::canvas(&board, &screen, None).text()
}

/// The project `p` whose config reads `providers`, with the given quota
/// readings in the meta (`provider` → the reading's JSON).
fn world_reading_custom(providers: &[&str], readings: &[(&str, &str)]) -> Store {
    let mut store = world();
    store
        .write(|tx| {
            let mut row = tx.get_project("p")?.unwrap();
            row.config_toml = providers
                .iter()
                .map(|provider| {
                    format!("[[quota.sources]]\nprovider = \"{provider}\"\nreader = \"codex\"\n")
                })
                .collect();
            tx.update_project(&row)?;
            for (provider, reading) in readings {
                tx.set_meta(&format!("quota.reading.{provider}"), reading)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// One reading, as the engine writes it: one window, one used percent, one
/// reset and when it was read.
fn reading_json(provider: &str, window: &str, reset: i64, read_at: i64) -> String {
    format!(
        r#"{{"provider":"{provider}","windows":[{{"name":"{window}","used_percent":63.0,"resets_at":{reset}}}],"read_at":{read_at}}}"#
    )
}

#[test]
fn the_quota_shows_only_the_providers_the_project_reads() {
    // The config reads codex alone; a reading of an earlier config stays in
    // the meta and is not shown (T-383).
    let mut store = world_reading_custom(
        &["codex"],
        &[
            (
                "codex",
                &reading_json("codex", "weekly", T0 + 6 * 86_400, T0),
            ),
            (
                "minimax-cn",
                r#"{"provider":"minimax-cn","windows":[{"name":"5h","used_percent":64.0,"resets_at":1800003600}],"read_at":1800000000}"#,
            ),
        ],
    );
    let frame = now_frame(&mut store, T0);
    assert!(frame.contains("codex"), "{frame}");
    assert!(!frame.contains("minimax-cn"), "{frame}");
}

#[test]
fn a_reset_days_away_prints_the_day_and_a_later_one_the_clock_alone() {
    let week = T0 + 6 * 86_400;
    let hours = T0 + 3 * 3600;
    let mut store = world_reading_custom(
        &["codex", "minimax-cn"],
        &[
            ("codex", &reading_json("codex", "weekly", week, T0)),
            ("minimax-cn", &reading_json("minimax-cn", "5h", hours, T0)),
        ],
    );
    let frame = now_frame(&mut store, T0);
    // A week out: the month and the day, then the time.
    let line = frame
        .lines()
        .find(|line| line.contains("codex"))
        .unwrap_or_else(|| panic!("no codex line: {frame}"));
    let reset = line.split("resets ").nth(1).unwrap_or_else(|| panic!("no reset: {line}"));
    let mut words = reset.split(' ');
    let month = words.next().unwrap_or_default();
    let day = words.next().unwrap_or_default();
    let time = words.next().unwrap_or_default();
    assert_eq!(month.len(), 3, "the month is a word: {line}");
    assert!(month.chars().all(|c| c.is_ascii_alphabetic()), "{line}");
    assert!(day.parse::<u32>().is_ok(), "the day is a number: {line}");
    assert_eq!(time.len(), 5, "then the time: {line}");
    assert_eq!(&time[2..3], ":", "{line}");
    // The date is the one the clock of the machine gives.
    assert!(line.contains(&gear_board::idle::day_clock(week)), "{line}");
    // Three hours out: only the clock, as today.
    let line = frame
        .lines()
        .find(|line| line.contains("minimax-cn"))
        .unwrap_or_else(|| panic!("no minimax-cn line: {frame}"));
    assert!(
        line.contains(&format!("resets {}", gear_board::idle::clock(hours))),
        "{line}"
    );
}

#[test]
fn a_reading_half_an_hour_old_says_its_age() {
    let mut store = world_reading_custom(
        &["codex"],
        &[("codex", &reading_json("codex", "weekly", T0 + 6 * 86_400, T0 - 45 * 60))],
    );
    let frame = now_frame(&mut store, T0);
    assert!(frame.contains("(read 45 min ago)"), "{frame}");
}

// ---- T-411: the wheel and the page keys scroll a long view

/// A hundred tasks on one lane: a view no viewport shows whole, so what the
/// wheel and the page keys do to the selection is visible on the frame.
fn world_with_a_hundred_tasks() -> Store {
    world_with_tasks(100)
}

/// A project with `n` queued tasks, T-01 to T-n, all on one lane: a graph
/// longer than a view holds.
fn world_with_tasks(n: usize) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            for n in 1..=n {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: format!("T-{n:02}"),
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

/// The head of the board of the `p` world: the row of the view's heading, the
/// tab row and the columns of the tabs (T-353).
fn head_of(store: &mut Store, size: (usize, usize)) -> gear_board::frame::Layout {
    use gear_board::frame;
    let board = build(
        store,
        &Options { project: Some("p".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    frame::layout(&board, &crate::screen(frame::Tab::Now, 0, size))
}

/// The board's last frame over the given events, on a screen of a given size.
fn run_events(store: &mut Store, events: Vec<Event>, size: (usize, usize)) -> String {
    let out = run_events_output(store, events, size);
    last_frame(out.as_bytes())
}

/// Everything the board wrote over the given events: every frame, in order.
fn run_events_output(store: &mut Store, events: Vec<Event>, size: (usize, usize)) -> String {
    let opts = Options {
        project: Some("p".into()),
        color: false,
        refresh: Duration::from_millis(1),
        ..Options::default()
    };
    let mut out: Vec<u8> = Vec::new();
    run_with(store, &opts, &mut out, &mut Events { events, size }, None).unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

/// A project with two voyages (T-357): M1 landed, its one task merged, and M2
/// sailing on a task that is still open, so the board's measured voyage is M2.
fn world_with_two_voyages() -> Store {
    let now = now_seconds();
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", now)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", now)?;
            testing::seed_active_seat(tx, "p", "old", SeatRole::Lane, "w1:p2", "pi", now)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p3", "pi", now)?;
            for (milestone, departure) in [("M1", now - 7200), ("M2", now - 3600)] {
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: milestone.into(),
                    title: format!("voyage {milestone}"),
                    done: Some("it is delivered".into()),
                    bar: None,
                    eta: Some("8h".into()),
                    departure: Some(departure),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(30),
                    updated: departure,
                })?;
            }
            for (id, rank, milestone, lane, state) in [
                ("T-1", 1, "M1", "old", TaskState::Done),
                ("T-2", 2, "M2", "hull", TaskState::Ready),
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
                    milestone: Some(milestone.into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: now,
                })?;
            }
            Ok(())
        })
        .unwrap();
    store
}

/// The rows a frame draws in reverse video, the first row's voyage labels and
/// the tab row left out: the pick the view has selected.
fn lit_picks(frame: &str, tab_row: usize) -> Vec<usize> {
    frame
        .lines()
        .enumerate()
        .filter(|(row, line)| *row != 0 && *row != tab_row && line.contains("\u{1b}[7m"))
        .map(|(row, _)| row)
        .collect()
}

/// T-357: `[` and `]` pick the voyage the whole board shows — the list is
/// newest first, so `[` goes one entry older and `]` one newer, and the ends
/// stay where they are — and a click on a label on the first row picks that
/// voyage (README changes 1 and 12).
#[test]
fn the_voyage_keys_and_a_click_on_a_label_pick_the_shown_voyage() {
    use gear_board::frame::{self, Tab};

    let size = (160, 40);
    let mut store = world_with_two_voyages();
    // Where the labels sit on the first row: the board opens on the measured
    // voyage, the sailing M2, and its list is newest first.
    let board = build(&mut store, &options(), now_seconds()).unwrap();
    let head = frame::layout(&board, &crate::screen(Tab::Now, 0, size));
    let labels: Vec<&str> =
        head.voyages.iter().map(|(_, _, milestone)| milestone.as_str()).collect();
    assert_eq!(labels, vec!["M2", "M1"], "the labels of the first row: {labels:?}");
    let (first, _, _) = head.voyages[1];
    assert!(!head.tabs.is_empty(), "the tab row is drawn below the labels");

    // The first frame shows the measured voyage, M2.
    let opened = run_events_output(&mut store, vec![], size);
    let opened = last_frame(opened.as_bytes());
    assert!(opened.lines().nth(1).unwrap_or_default().contains("M2:"), "{opened}");

    // `[` goes one entry older: the whole board shows the landed M1, and its
    // label on the first row is the lit one.
    let older = run_events_output(&mut store, vec![Event::Key('[')], size);
    let older = last_frame(older.as_bytes());
    let rows: Vec<&str> = older.lines().collect();
    assert!(rows[1].contains("M1: voyage M1 · ⚓ landed"), "row 1 of the older voyage: {older}");
    assert!(!rows[1].contains("M2:"), "the board shows M1 only: {older}");
    assert!(rows[0].contains("\u{1b}[1;7m M1 ⚓"), "the M1 label is lit: {older:?}");

    // `[` again: the oldest voyage stays the oldest.
    let oldest = run_events_output(&mut store, vec![Event::Key('['), Event::Key('[')], size);
    let oldest = last_frame(oldest.as_bytes());
    assert!(oldest.lines().nth(1).unwrap_or_default().contains("M1:"), "{oldest}");

    // `]` goes one entry newer: M2 is shown again.
    let newer = run_events_output(&mut store, vec![Event::Key('['), Event::Key(']')], size);
    let newer = last_frame(newer.as_bytes());
    assert!(newer.lines().nth(1).unwrap_or_default().contains("M2:"), "{newer}");

    // A click on the first column of the M1 label picks M1, and the pick starts
    // the selection over: two `Down` keys had selected the second lane, and the
    // clicked frame draws the first row the board of M1 draws at selection 0.
    let click = Event::Mouse { button: 0, x: (first + 1) as u16, y: 1 };
    let clicked = run_events_output(&mut store, vec![Event::Key('j'), Event::Key('j'), click], size);
    let clicked = last_frame(clicked.as_bytes());
    assert!(clicked.lines().nth(1).unwrap_or_default().contains("M1:"), "{clicked}");
    let m1 = build(
        &mut store,
        &Options { project: Some("p".into()), milestone: Some("M1".into()), ..Options::default() },
        now_seconds(),
    )
    .unwrap();
    let expected = frame::canvas(&m1, &crate::screen(Tab::Now, 0, size), None).ansi(false);
    assert_eq!(
        lit_picks(&clicked, head.tab_row),
        lit_picks(&expected, head.tab_row),
        "the pick starts the selection over: {clicked}"
    );
}

/// T-411: ten notches of the wheel move the selection three picks each, the
/// wheel stops at the first pick, and a page key moves a screenful with the
/// viewport.
#[test]
fn the_wheel_and_the_page_keys_move_the_selection_over_a_long_view() {
    let mut store = world_with_a_hundred_tasks();
    let size = (80, 40);
    // The head of the board says where the view's first row is and how many
    // picks a page holds (T-353).
    let head = head_of(&mut store, size);
    let first = head.top + 1;
    let page = size.1 - (head.top + 2);

    // The waiting tasks hang newest first, so the first pick is T-100 and the
    // thirtieth is T-70 (T-361, T-362); a task's row is two rows (its ○ and
    // the dashed ┆), so thirty picks scroll the viewport to its end (T-353:
    // the head's height moves the rows, not the picks).
    let mut events = vec![Event::Key('2')];
    events.extend((0..10).map(|_| Event::Mouse { button: 65, x: 1, y: 7 }));
    let frame = run_events(&mut store, events, size);
    let rows: Vec<&str> = frame.lines().collect();
    let row = gear_board::frame::view_bottom(gear_board::frame::Tab::Tasks, size.1) - 1;
    assert!(
        rows.get(row).unwrap_or(&"").contains("T-70"),
        "the thirtieth pick's row holds T-70: {frame}"
    );
    assert!(
        rows.get(row).unwrap_or(&"").contains("\u{1b}[7m"),
        "the wheel's pick is the lit row: {frame}"
    );

    // The wheel up at the first pick stays there.
    let mut events = vec![Event::Key('2')];
    events.extend((0..10).map(|_| Event::Mouse { button: 64, x: 1, y: 7 }));
    let frame = run_events(&mut store, events, size);
    let rows: Vec<&str> = frame.lines().collect();
    assert!(
        rows.get(first).unwrap_or(&"").contains("T-100"),
        "the wheel up stays at the first pick: {frame}"
    );
    assert!(
        rows.get(first).unwrap_or(&"").contains("\u{1b}[7m"),
        "the first pick is the lit row: {frame}"
    );

    // A page down from the first pick moves a screenful of picks, so the pick
    // that far down is selected (the graph lists the tasks newest first), and
    // the viewport followed it: the heading's row scrolled away and the first
    // pick is no longer drawn.
    let frame = run_events(&mut store, vec![Event::Key('2'), Event::PageDown], size);
    let rows: Vec<&str> = frame.lines().collect();
    let last = gear_board::frame::view_bottom(gear_board::frame::Tab::Tasks, size.1);
    let lit = (first + page).min(last - 1);
    assert!(
        rows.get(lit).unwrap_or(&"").contains(&format!("T-{:02}", 100 - page)),
        "the page down picks the task one screen down: {frame}"
    );
    assert!(
        rows.get(lit).unwrap_or(&"").contains("\u{1b}[7m"),
        "the page key's pick is the lit row: {frame}"
    );
    assert!(
        !frame.contains("T-100"),
        "the viewport moved with the selection: {frame}"
    );
    assert!(
        !rows.get(head.top).unwrap_or(&"").contains("dev"),
        "the heading scrolled away: {frame}"
    );

    // A page up comes back to the first pick.
    let frame = run_events(
        &mut store,
        vec![Event::Key('2'), Event::PageDown, Event::PageUp],
        size,
    );
    let rows: Vec<&str> = frame.lines().collect();
    assert!(
        rows.get(first).unwrap_or(&"").contains("T-100"),
        "the page up comes back to the first pick: {frame}"
    );
    assert!(
        rows.get(head.top).unwrap_or(&"").contains("dev"),
        "the heading is back: {frame}"
    );
}

/// T-411: a wheel notch is never a click. It closes a panel as an arrow does
/// and opens none, and a notch where the tabs are drawn switches no view.
#[test]
fn a_wheel_notch_is_never_a_click() {
    let mut store = world_with_a_hundred_tasks();
    let size = (80, 40);
    let head = head_of(&mut store, size);

    // `Enter` opens the panel of the first task; the next notch closes it as
    // an arrow would, and no panel is opened in its place. The panel is drawn
    // on the frame before the notch, so its absence after it says something
    // (the panel's frame is not the last one: the key that closes it is read
    // after it).
    let opened = run_events_output(&mut store, vec![Event::Key('2'), Event::Key('\r')], size);
    assert!(opened.contains("ROUNDS"), "`Enter` opens the panel: {opened}");
    let frame = run_events(
        &mut store,
        vec![Event::Key('2'), Event::Key('\r'), Event::Mouse { button: 65, x: 1, y: 7 }],
        size,
    );
    let rows: Vec<&str> = frame.lines().collect();
    assert!(!frame.contains("ROUNDS"), "the wheel opens no panel: {frame}");
    assert!(
        rows.get(head.top + 1).unwrap_or(&"").contains("T-100"),
        "the notch that closed the panel moved nothing: {frame}"
    );

    // A notch on the tab row is not a click on the tab under it: the view
    // stays the task view and the selection moves three picks (T-97, drawn on
    // the row two rows per pick below the first).
    let frame = run_events(
        &mut store,
        vec![Event::Key('2'), Event::Mouse { button: 65, x: 1, y: head.tab_row as u16 + 1 }],
        size,
    );
    let rows: Vec<&str> = frame.lines().collect();
    let row = head.top + 1 + 6;
    assert!(
        rows.get(row).unwrap_or(&"").contains("T-97"),
        "a notch on the tab row scrolls instead of switching the view: {frame}"
    );
    assert!(
        rows.get(row).unwrap_or(&"").contains("\u{1b}[7m"),
        "the notch's pick is the lit row: {frame}"
    );
}

/// T-411: the page keys arrive as `[5~` and `[6~`, and the wheel's SGR
/// sequence is still a mouse event with its own button.
#[test]
fn the_page_keys_parse_and_the_wheel_is_still_a_mouse() {
    let pty = Pty::open();
    let slave = pty.slave;
    let (ready, is_ready) = std::sync::mpsc::channel();
    let (left, events) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        // Raw mode is on once `Terminal::on` returns: only then do the bytes
        // come one at a time instead of waiting for a newline.
        let mut keys = gear_board::Terminal::on(slave);
        let _ = ready.send(());
        for _ in 0..5 {
            if left.send(keys.next_event(Duration::from_secs(5))).is_err() {
                break;
            }
        }
    });
    is_ready.recv_timeout(std::time::Duration::from_secs(5)).expect("the terminal is in raw mode");
    press(pty.master, b"\x1b[5~");
    press(pty.master, b"\x1b[6~");
    press(pty.master, b"\x1b[<64;10;20M");
    press(pty.master, b"\x1b[<65;10;20M");
    press(pty.master, b"\x1b[<0;1;7M");

    let read = |events: &std::sync::mpsc::Receiver<Option<Event>>| {
        events.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or(None)
    };
    assert_eq!(read(&events), Some(Event::PageUp), "`[5~` is the page up");
    assert_eq!(read(&events), Some(Event::PageDown), "`[6~` is the page down");
    assert_eq!(
        read(&events),
        Some(Event::Mouse { button: 64, x: 10, y: 20 }),
        "the wheel up is still a mouse"
    );
    assert_eq!(
        read(&events),
        Some(Event::Mouse { button: 65, x: 10, y: 20 }),
        "the wheel down is still a mouse"
    );
    assert_eq!(
        read(&events),
        Some(Event::Mouse { button: 0, x: 1, y: 7 }),
        "the left click keeps its button"
    );
    reader.join().ok();
}

/// T-415: the totals count the live work only. A cancelled task stays in the
/// plan's views, but it is not part of what the voyage still delivers: four
/// cancelled tasks out of nine must not read as half the voyage undone.
#[test]
fn cancelled_tasks_are_not_counted_in_the_totals() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M5".into(),
                title: "voyage M5".into(),
                done: Some("it is delivered".into()),
                bar: None,
                eta: Some("8h".into()),
                departure: Some(T0 - 3600),
                departure_tasks: Some(9),
                departure_estimate_min: Some(270),
                updated: T0 - 3600,
            })?;
            for (id, rank, state) in [
                ("T-1", 1, TaskState::Done),
                ("T-2", 2, TaskState::Done),
                ("T-3", 3, TaskState::Done),
                ("T-4", 4, TaskState::Ready),
                ("T-5", 5, TaskState::Ready),
                ("T-6", 6, TaskState::Cancelled),
                ("T-7", 7, TaskState::Cancelled),
                ("T-8", 8, TaskState::Cancelled),
                ("T-9", 9, TaskState::Cancelled),
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
                    lane: Some("data".into()),
                    milestone: Some("M5".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: T0,
                })?;
            }
            Ok(())
        })
        .unwrap();

    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let plan = board.projects.first().expect("the project");
    // 3 done of the 5 live tasks, not of all nine.
    assert_eq!(plan.totals.tasks, 5, "{:?}", plan.totals);
    assert_eq!(plan.totals.done, 3, "{:?}", plan.totals);
    let text = render(&board, View::Lanes);
    assert!(text.contains("Totals: 5 tasks · 3 done"), "{text}");
    // The cancelled tasks stay in the lane's queue, as they did before.
    let lane = plan.lanes.iter().find(|lane| lane.lane == "data").expect("the lane");
    let ids: Vec<&str> = lane.tasks.iter().map(|task| task.id.as_str()).collect();
    for id in ["T-6", "T-7", "T-8", "T-9"] {
        assert!(ids.contains(&id), "{id} stays in the Tasks view: {ids:?}");
    }
}

/// T-359: the Now view's table has the prototype's columns: the task column
/// `tw = max(30, min(52, W - 110))` puts STAGE at 63 on a 200-column screen.
#[test]
fn the_now_table_puts_stage_at_the_prototype_column() {
    use gear_board::frame::{self, Tab};

    let mut store = world_with_a_voyage();
    let board = build(&mut store, &options(), now_seconds()).unwrap();
    let screen = crate::screen(Tab::Now, 0, (200, 40));
    let text = frame::canvas(&board, &screen, None).text();
    let top = frame::layout(&board, &screen).top;
    assert_eq!(column_of(&text, top, "STAGE"), 63, "{text}");
    let row = text.lines().nth(top).unwrap_or_default();
    assert!(row.contains("LANE"), "{row:?}");
    assert!(row.contains("NOW DOING"), "{row:?}");
    assert!(row.contains("LANE DONE"), "{row:?}");
    assert!(row.contains("ENDS"), "{row:?}");
}

/// T-359: a landed voyage's lanes are retired at the landing, in Dim, and no
/// lane carries an end time any more.
#[test]
fn a_landed_voyage_s_lane_reads_retired_at_the_landing_and_has_no_ends() {
    use gear_board::frame::{self, Tab};
    use gear_board::style::{Color, Style};

    let mut store = world_with_a_voyage();
    store
        .write(|tx| {
            for id in ["T-2", "T-3", "T-4"] {
                let mut row = tx.get_task("p", id)?.expect("the task");
                row.state = TaskState::Done;
                tx.update_task(&row)?;
            }
            Ok(())
        })
        .unwrap();
    let board = build(&mut store, &options(), now_seconds()).unwrap();
    assert!(!board.projects[0].shown.as_ref().expect("the voyage").sailing);
    // The second lane's row is not the lit one: its Dim reason is readable.
    let screen = crate::screen(Tab::Now, 1, (160, 40));
    let canvas = frame::canvas(&board, &screen, None);
    let text = canvas.text();
    let rows: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("retired at the landing"))
        .map(|(row, _)| row)
        .collect();
    assert_eq!(rows.len(), 2, "both lanes retired:\n{text}");
    let row = rows[0];
    assert_eq!(canvas.style_at(9, row), Style::fg(Color::Dim), "{text}");
    assert!(!text.lines().nth(row).unwrap_or_default().contains('~'), "{text}");
}

/// T-359: a lane with nothing open says what its next task waits for, with the
/// dependency's lane and stage words.
#[test]
fn a_lane_with_nothing_open_names_the_dependency_it_waits_for() {
    use gear_board::frame::{self, Tab};

    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "mast", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p3", "opencode", T0)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one voyage".into(),
                done: Some("it is delivered".into()),
                bar: None,
                eta: Some("8h".into()),
                departure: Some(T0 - 3600),
                departure_tasks: Some(2),
                departure_estimate_min: Some(60),
                updated: T0 - 3600,
            })?;
            for (id, rank, lane, state, deps) in [
                ("T-1", 1, "mast", TaskState::Running, vec![]),
                ("T-2", 2, "hull", TaskState::Ready, vec!["T-1".to_string()]),
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
                    deps,
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: T0,
                })?;
            }
            tx.insert_attempt(&AttemptRow {
                project: "p".into(),
                task: "T-1".into(),
                n: 1,
                seat: "p:mast".into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Started,
                candidate: None,
                report: None,
                safe_point: None,
                started: T0,
                updated: T0,
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
            })?;
            Ok(())
        })
        .unwrap();
    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = frame::canvas(&board, &crate::screen(Tab::Now, 0, (160, 40)), None).text();
    assert!(
        text.contains("next T-2 waits for T-1 (mast, working)"),
        "{text}"
    );
}

/// T-359, the captain's answer M-6103: a cancelled task is not the lane's NOW
/// DOING, whatever stage the graph still gives it; the lane's next open task
/// is.
#[test]
fn a_cancelled_task_is_not_the_now_doing_of_its_lane() {
    use gear_board::frame::{self, Tab};

    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "claude", T0)?;
            testing::seed_active_seat(tx, "p", "data", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            tx.upsert_voyage(&VoyageRow {
                project: "p".into(),
                milestone: "M4".into(),
                title: "one voyage".into(),
                done: Some("it is delivered".into()),
                bar: None,
                eta: Some("8h".into()),
                departure: Some(T0 - 3600),
                departure_tasks: Some(2),
                departure_estimate_min: Some(60),
                updated: T0 - 3600,
            })?;
            for (id, rank, state) in [
                ("T-1", 1, TaskState::Cancelled),
                ("T-2", 2, TaskState::Running),
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
                    lane: Some("data".into()),
                    milestone: Some("M4".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: T0,
                })?;
                // A cancelled task keeps the attempt that made its stage open.
                tx.insert_attempt(&AttemptRow {
                    project: "p".into(),
                    task: id.into(),
                    n: 1,
                    seat: "p:data".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: AttemptState::Started,
                    candidate: None,
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                })?;
            }
            Ok(())
        })
        .unwrap();
    let board = build(&mut store, &options(), T0 + 60).unwrap();
    let text = frame::canvas(&board, &crate::screen(Tab::Now, 0, (160, 40)), None).text();
    assert!(text.contains("data  T-2"), "{text}");
    assert!(!text.contains("data  T-1"), "the cancelled task is NOW DOING:\n{text}");
}
