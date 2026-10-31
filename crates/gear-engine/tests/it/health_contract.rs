//! T-472: the fleet and machine health watch.
//!
//! The watch samples the machine and each lane's and reviewer's own processes
//! once a minute. A machine that stays short is relieved by stopping a seat
//! whose processes are over the safe line, and the captain is told; a machine
//! that is short for another reason gets one urgent captain item naming the
//! top processes, which are never touched. The readings are injected: the
//! watch's own cost is measured separately, on the real reader.

use gear_engine::health::{self, Machine, PaneProcs, Proc, Read, Reading, SeatProcs};
use gear_engine::{tick, EngineCfg};
use gear_herdr::fake::{Call, FakeHerdr};
use gear_herdr::AgentStatus;
use gear_model::{MsgKind, MsgState, SeatRole, SeatState};
use gear_store::{testing, Store};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const T0: i64 = 1_800_000_000;
const CAP: &str = "w1:p1";
const LANE: &str = "w1:p2";
const TEMP: &str = "w1:p3";

/// The readings a test scripts, one per sample, and how many were taken.
#[derive(Debug, Default)]
struct FakeRead {
    readings: Mutex<VecDeque<Reading>>,
    samples: AtomicUsize,
}

impl FakeRead {
    fn new(readings: Vec<Reading>) -> Arc<FakeRead> {
        Arc::new(FakeRead {
            readings: Mutex::new(readings.into()),
            samples: AtomicUsize::new(0),
        })
    }
}

impl health::Read for FakeRead {
    fn sample(
        &self,
        _panes: &BTreeMap<String, PaneProcs>,
        _volumes: &[String],
    ) -> gear_model::GearResult<Reading> {
        self.samples.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .readings
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default())
    }
}

struct World {
    store: Store,
    herdr: FakeHerdr,
    cfg: EngineCfg,
    read: Arc<FakeRead>,
}

fn world(readings: Vec<Reading>) -> World {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE, "codex", T0)?;
            testing::seed_active_seat(tx, "p", "rev-1", SeatRole::Temp, TEMP, "codex", T0)?;
            Ok(())
        })
        .unwrap();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent(CAP, "codex", AgentStatus::Idle);
    herdr.add_agent(LANE, "codex", AgentStatus::Idle);
    herdr.add_agent(TEMP, "codex", AgentStatus::Idle);
    let read = FakeRead::new(readings);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    cfg.health = read.clone();
    World {
        store,
        herdr,
        cfg,
        read,
    }
}

fn run(w: &mut World, seconds: i64) -> gear_engine::TickReport {
    tick(&mut w.store, &mut w.herdr, &w.cfg, T0 + seconds).unwrap()
}

/// A machine that is short from the first sample: load 30 on 10 cores.
fn short_machine() -> Machine {
    Machine {
        cores: 10,
        load5: 30.0,
        mem_total_kb: 16 * 1024 * 1024,
        mem_free_kb: 8 * 1024 * 1024,
        disk: vec![("/state".into(), 50 * 1024 * 1024)],
    }
}

fn fine_machine() -> Machine {
    Machine {
        load5: 5.0,
        ..short_machine()
    }
}

/// Six readings, one a minute: the machine short (or fine) and the lane's own
/// processes. The lane's cumulative CPU grows by `cpu_per_min` seconds each
/// minute, so its use is `cpu_per_min / 60 * 100` % of one core.
fn readings(machine: Machine, lane_cpu_s: f64, lane_rss_kb: u64, top: Vec<Proc>) -> Vec<Reading> {
    (0..6)
        .map(|n| Reading {
            machine: machine.clone(),
            seats: BTreeMap::from([(
                "p:heat".to_string(),
                SeatProcs {
                    cpu_s: lane_cpu_s * f64::from(n),
                    rss_kb: lane_rss_kb,
                    procs: 3,
                },
            )]),
            top: top.clone(),
        })
        .collect()
}

fn items(w: &mut World, thread: &str) -> Vec<gear_store::MessageRow> {
    w.store.read(|tx| tx.thread_messages("p", thread)).unwrap()
}

fn keys(w: &World, pane: &str, key: &str) -> usize {
    w.herdr
        .calls
        .iter()
        .filter(|call| matches!(call, Call::SendKeys { pane: p, keys } if p == pane && keys == &[key.to_string()]))
        .count()
}

#[test]
fn a_seat_over_the_safe_line_is_stopped_and_the_captain_told() {
    // The lane's own processes hold 3 GB (over the 2 GB safe line) for five
    // minutes while the machine is short: an internal cause.
    let mut w = world(readings(short_machine(), 60.0, 3 * 1024 * 1024, vec![]));
    for minute in 0..=5 {
        run(&mut w, minute * 60);
    }
    assert_eq!(w.read.samples.load(Ordering::Relaxed), 6);
    assert!(
        keys(&w, LANE, "esc") >= 1,
        "the turn is interrupted: {:?}",
        w.herdr.calls
    );
    assert!(
        w.herdr.prompts_to(LANE).iter().any(|text| *text == "/quit"),
        "the agent is stopped with the profile's exit: {:?}",
        w.herdr.calls
    );
    let seat = w.store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
    assert_eq!(seat.state, SeatState::NeedsCheck);
    assert!(seat
        .state_reason
        .as_deref()
        .unwrap_or_default()
        .contains("health watch"));
    let item = items(&mut w, "health:p:heat");
    assert_eq!(item.len(), 1, "one item for the seat");
    assert_eq!(item[0].recipient, "p:captain");
    assert_eq!(item[0].kind, MsgKind::System);
    assert_eq!(item[0].state, MsgState::Open);
    assert!(item[0].urgent);
    assert!(item[0].body.contains("heat"), "{}", item[0].body);
    assert!(item[0].body.contains("- retry heat"), "{}", item[0].body);
    assert!(
        items(&mut w, "health:machine:p").is_empty(),
        "an internal cause is not the machine's item"
    );
    // The reviewer, under the safe line, is untouched.
    assert_eq!(keys(&w, TEMP, "esc"), 0);
    let review = w.store.read(|tx| tx.get_seat("p:rev-1")).unwrap().unwrap();
    assert_eq!(review.state, SeatState::Active);
}

#[test]
fn a_machine_short_for_another_reason_names_the_top_processes() {
    // No seat is over the safe line: the lane uses 10 % of a core and 100 MB.
    let top = vec![
        Proc {
            pid: 4242,
            cpu_pct: 120.0,
            rss_kb: 3 * 1024 * 1024,
            command: "fseventsd".into(),
        },
        Proc {
            pid: 77,
            cpu_pct: 40.0,
            rss_kb: 200 * 1024,
            command: "some-other-build".into(),
        },
    ];
    let mut w = world(readings(short_machine(), 6.0, 100 * 1024, top));
    for minute in 0..=5 {
        run(&mut w, minute * 60);
    }
    let item = items(&mut w, "health:machine:p");
    assert_eq!(item.len(), 1, "one urgent machine item");
    assert!(item[0].urgent);
    assert_eq!(item[0].kind, MsgKind::System);
    assert!(item[0].body.contains("4242"), "{}", item[0].body);
    assert!(item[0].body.contains("fseventsd"), "{}", item[0].body);
    assert!(
        item[0].body.contains("some-other-build"),
        "{}",
        item[0].body
    );
    assert!(
        item[0].body.contains("no seat of this fleet"),
        "{}",
        item[0].body
    );
    // Never touched: no seat stopped, no key sent to the seats.
    assert!(items(&mut w, "health:p:heat").is_empty());
    assert_eq!(keys(&w, LANE, "esc"), 0);
    assert_eq!(keys(&w, TEMP, "esc"), 0);
    assert_eq!(
        w.store
            .read(|tx| tx.get_seat("p:heat"))
            .unwrap()
            .unwrap()
            .state,
        SeatState::Active
    );
}

#[test]
fn a_fine_machine_stops_nothing_even_when_a_seat_is_over_the_line() {
    let mut w = world(readings(fine_machine(), 600.0, 8 * 1024 * 1024, vec![]));
    for minute in 0..=5 {
        run(&mut w, minute * 60);
    }
    assert!(items(&mut w, "health:p:heat").is_empty());
    assert!(items(&mut w, "health:machine:p").is_empty());
    assert_eq!(keys(&w, LANE, "esc"), 0);
    assert_eq!(
        w.store
            .read(|tx| tx.get_seat("p:heat"))
            .unwrap()
            .unwrap()
            .state,
        SeatState::Active
    );
}

#[test]
fn the_watch_samples_once_a_minute() {
    let mut w = world(readings(fine_machine(), 6.0, 100 * 1024, vec![]));
    run(&mut w, 0);
    assert_eq!(w.read.samples.load(Ordering::Relaxed), 1);
    run(&mut w, 30);
    run(&mut w, 59);
    assert_eq!(
        w.read.samples.load(Ordering::Relaxed),
        1,
        "no second sample inside the minute"
    );
    run(&mut w, 60);
    assert_eq!(w.read.samples.load(Ordering::Relaxed), 2);
}

#[test]
fn the_machine_item_is_resolved_when_the_machine_is_back() {
    let mut w = world(vec![
        Reading {
            machine: short_machine(),
            ..Default::default()
        },
        Reading {
            machine: short_machine(),
            ..Default::default()
        },
        Reading {
            machine: short_machine(),
            ..Default::default()
        },
        Reading {
            machine: short_machine(),
            ..Default::default()
        },
        Reading {
            machine: short_machine(),
            ..Default::default()
        },
        Reading {
            machine: short_machine(),
            ..Default::default()
        },
        Reading {
            machine: fine_machine(),
            ..Default::default()
        },
    ]);
    for minute in 0..=5 {
        run(&mut w, minute * 60);
    }
    assert_eq!(items(&mut w, "health:machine:p").len(), 1);
    assert_eq!(items(&mut w, "health:machine:p")[0].state, MsgState::Open);
    run(&mut w, 6 * 60);
    let item = items(&mut w, "health:machine:p");
    assert_eq!(item[0].state, MsgState::Resolved);
    // A later short spell rings again, once it has lasted its five minutes.
    for _ in 0..6 {
        w.read.readings.lock().unwrap().push_back(Reading {
            machine: short_machine(),
            ..Default::default()
        });
    }
    for minute in 11..=16 {
        run(&mut w, minute * 60);
    }
    assert_eq!(
        items(&mut w, "health:machine:p").len(),
        2,
        "a new spell is a new item"
    );
}

/// The watch's own cost: one real sample per minute, measured in CPU time. At
/// 60 s between samples, 1 % of one core is 600 ms; the sample runs `ps` and
/// a few system reads, far below that.
#[test]
fn the_watch_costs_under_one_percent_of_one_core() {
    fn cpu_ms() -> f64 {
        let mut total = 0.0;
        for who in [libc::RUSAGE_SELF, libc::RUSAGE_CHILDREN] {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
            // SAFETY: getrusage fills the struct it is given.
            if unsafe { libc::getrusage(who, usage.as_mut_ptr()) } != 0 {
                continue;
            }
            let usage = unsafe { usage.assume_init() };
            total += usage.ru_utime.tv_sec as f64 * 1000.0;
            total += usage.ru_utime.tv_usec as f64 / 1000.0;
            total += usage.ru_stime.tv_sec as f64 * 1000.0;
            total += usage.ru_stime.tv_usec as f64 / 1000.0;
        }
        total
    }
    let dir = crate::temp("health-watch-cost");
    let volumes = vec![dir.to_string_lossy().into_owned()];
    let panes = BTreeMap::from([(
        "p:heat".to_string(),
        PaneProcs {
            shell: Some(std::process::id()),
            foreground: vec![],
            foreground_group: None,
        },
    )]);
    let read = health::SystemRead::new();
    let before = cpu_ms();
    let reading = read.sample(&panes, &volumes).expect("a real sample");
    let used = cpu_ms() - before;
    assert!(reading.machine.cores > 0, "{:?}", reading.machine);
    assert!(!reading.machine.disk.is_empty(), "{:?}", reading.machine);
    // 1 % of one core over a minute is 600 ms of CPU.
    assert!(
        used < 600.0,
        "the sample used {used:.1} ms of CPU; the line is 600 ms"
    );
}
