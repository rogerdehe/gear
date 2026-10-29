//! A `gear` child process that cannot reach the real store (T-41).
//!
//! A lane runs the tests with `GEAR_DB` set — the production store — and the
//! store `gear` falls back to lives under `$HOME`. Every `gear` a test starts
//! gets no `GEAR_DB` and a scratch `HOME`, so neither the store it was given
//! nor the one it would fall back to is the real one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A scratch HOME, unique per tag and process.
pub fn home(tag: &str) -> PathBuf {
    temp(&format!("gear-test-{tag}"))
}

/// A scratch directory, unique per tag and process, removed when the test that
/// made it is done (T-412).
/// A fresh scratch directory under the temp directory, unique to this test
/// and this name, removed when the test is done (T-412): a passing run leaves
/// nothing behind. The same name in one test is the same directory.
pub fn temp(name: &str) -> PathBuf {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicU64, Ordering};
    static THREADS: AtomicU64 = AtomicU64::new(0);

    /// The directories one test made, removed when that test is done.
    struct Made {
        /// The number of this thread, given at its first scratch directory.
        thread: Cell<Option<u64>>,
        /// The directories it made, by name.
        dirs: RefCell<Vec<(String, PathBuf)>>,
    }

    impl Drop for Made {
        fn drop(&mut self) {
            for (_, dir) in self.dirs.borrow().iter() {
                std::fs::remove_dir_all(dir).ok();
            }
        }
    }

    thread_local! {
        static MADE: Made = const { Made { thread: Cell::new(None), dirs: RefCell::new(Vec::new()) } };
    }

    MADE.with(|made| {
        if let Some((_, dir)) = made.dirs.borrow().iter().find(|(named, _)| named == name) {
            return dir.clone();
        }
        let thread = made.thread.get().unwrap_or_else(|| {
            let thread = THREADS.fetch_add(1, Ordering::Relaxed);
            made.thread.set(Some(thread));
            thread
        });
        let dir = std::env::temp_dir().join(format!("{name}-{}-{thread}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        made.dirs.borrow_mut().push((name.to_string(), dir.clone()));
        dir
    })
}

/// A `gear` child process that cannot reach the real store: no `GEAR_DB`, a
/// scratch `HOME`, and no Herdr around it (an engine only runs on the store
/// that is registered for a Herdr session).
pub fn gear(home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(args)
        .env("HOME", home)
        .env_remove("GEAR_CTX")
        .env_remove("GEAR_DB")
        .env_remove("GEAR_ADMIN")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH");
    cmd
}

/// The store `gear` uses under `home` when `GEAR_DB` is not set.
pub fn default_store(home: &Path) -> PathBuf {
    home.join(".local/state/gear/gear.db")
}
