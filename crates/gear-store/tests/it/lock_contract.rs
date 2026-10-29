//! Contract for the write lock (T-379): a caller waits for the store instead
//! of failing with `Locked`, and a write that holds the store long is named.

use gear_store::{slow_write_line, testing, Store, BUSY_TIMEOUT_MS, SLOW_WRITE_MS};
use std::time::Duration;

const T0: i64 = 1_800_000_000;

/// T-379: a `gear` call queues behind a slow writer instead of giving up
/// after five seconds.
#[test]
fn the_busy_timeout_is_thirty_seconds() {
    assert_eq!(BUSY_TIMEOUT_MS, 30_000);
}

/// T-379: a second connection whose write starts while the first holds a
/// write for 200 ms succeeds after it, never `Locked`.
#[test]
fn a_second_writer_waits_for_the_first_and_is_not_locked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gear.db");

    // Both connections exist before the lock is taken: the test measures the
    // write, not the open.
    let mut holder = Store::open(&path).unwrap();
    let mut second = Store::open(&path).unwrap();
    holder
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            Ok(())
        })
        .unwrap();

    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        holder
            .write(|tx| {
                held_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(200));
                tx.next_id("p", 'M')
            })
            .unwrap()
    });
    held_rx.recv().unwrap();

    let second_id = second
        .write(|tx| tx.next_id("p", 'M'))
        .expect("the second writer waits for the store instead of failing with Locked");

    let held_id = holder.join().unwrap();
    assert_ne!(second_id, held_id);
}

/// T-379: the slow-write line names the caller's file and line, so the log
/// says which writer held the store.
#[test]
fn the_slow_write_line_names_the_caller() {
    let caller = std::panic::Location::caller();
    let line = slow_write_line(1_500, caller);

    assert_eq!(
        line,
        format!("gear-store: a write held the store for 1500 ms ({caller})")
    );
    assert!(
        line.contains("gear-store: a write held the store for 1500 ms ("),
        "{line}"
    );
    assert!(line.contains("lock_contract.rs"), "{line}");
    assert!(
        line.contains(&format!("lock_contract.rs:{}", caller.line())),
        "{line}"
    );
}

/// T-379: the threshold is the packet's 1 000 ms.
#[test]
fn the_slow_write_threshold_is_one_second() {
    assert_eq!(SLOW_WRITE_MS, 1_000);
}
