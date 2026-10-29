//! Scratch directories of the unit tests, removed when the test is done (T-412):
//! the system temp directory held about 168 000 directories left by test runs
//! (2026-10-29); a passing run leaves none of its own behind. A test's
//! directories are removed when its thread ends — the harness gives every test
//! its own thread.

/// A fresh scratch directory under the temp directory, unique to this test
/// and this name, removed when the test is done (T-412): a passing run leaves
/// nothing behind. The same name in one test is the same directory.
pub(crate) fn temp(name: &str) -> std::path::PathBuf {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicU64, Ordering};
    static THREADS: AtomicU64 = AtomicU64::new(0);

    /// The directories one test made, removed when that test is done.
    struct Made {
        /// The number of this thread, given at its first scratch directory.
        thread: Cell<Option<u64>>,
        /// The directories it made, by name.
        dirs: RefCell<Vec<(String, std::path::PathBuf)>>,
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
