//! All integration tests of gear-engine in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.

/// Tests that onboard a `grok` lane (its trust file is written under `HOME`)
/// or set `HOME` themselves take this lock: `HOME` is process-wide, so two
/// such tests at once write into each other's home.
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static REVIEW_GIT_CALLS: std::cell::RefCell<Vec<Vec<String>>> = const { std::cell::RefCell::new(Vec::new()) };
    static REVIEW_GIT_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[derive(Debug)]
pub struct FakeReviewGit;
impl gear_engine::review::ReviewGit for FakeReviewGit {
    fn run(&self, _path: &str, args: &[&str]) -> Result<String, String> {
        REVIEW_GIT_CALLS.with(|calls| {
            calls
                .borrow_mut()
                .push(args.iter().map(|s| (*s).into()).collect())
        });
        if REVIEW_GIT_FAIL.with(|fail| fail.get()) {
            Err("injected Git failure".into())
        } else {
            Ok(String::new())
        }
    }
}

/// A fresh scratch directory under the temp directory, unique to this test
/// and this name, removed when the test is done (T-412): a passing run leaves
/// nothing behind. The same name in one test is the same directory.
pub fn temp(name: &str) -> std::path::PathBuf {
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

mod backoff_contract;
mod cancel_tick_contract;
mod config_profile_onboard;
mod contract_update;
mod deliver_confirm_contract;
mod deliver_contract;
mod delivery_landing_contract;
mod dialog_contract;
mod dispatch_contract;
mod dispatch_tick_contract;
mod doorbell_contract;
mod doorbell_isolation;
mod doorbell_while_working;
mod doorbell_working_budget_contract;
mod eng_confirm;
mod eng_execute;
mod eng_intend_recover;
mod eng_reconcile;
mod feed_contract;
mod friction_ring_contract;
mod herdr_outage_contract;
mod lane_view_contract;
mod onboard_contract;
mod onboard_run_contract;
mod park_end_contract;
mod reviewer_nudge_contract;
mod settle_review_tick;
mod slice1_messaging;
mod slice2_dispatch;
mod stall_contract;

mod agent_names_contract;
mod all_tail_contract;
mod blocked_contract;
mod blocked_delivery_contract;
mod branch_templates_contract;
mod busy_contract;
mod captain_quota_contract;
mod dead_temp_slot_contract;
mod delivery_grace_contract;
mod guardrails_contract;
mod hand_in_notifications;
mod health_contract;
mod lane_nudge_contract;
mod lane_restart_generation_contract;
mod multi_repo_contract;
mod parallel_contract;
mod pi_lane_contract;
mod project_failure_contract;
mod quota_wait_resume_contract;
mod ready_state_contract;
mod remote_contract;
mod reviewer_quota_contract;
mod rework_mid_turn_contract;
mod rounds_contract;
mod session_dispatch_contract;
mod station_contract;
mod targeting_contract;
mod undelivered_delivery_contract;
mod waits_contract;

/// A scratch home with the GEAR skill installed for every kind (T-83): `init`
/// refuses a kind whose skill is missing, and these fixtures are not about
/// that check. Built once per test process.
pub fn skill_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = HOME.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        for agent in gear_app::skill::SkillAgent::ALL {
            gear_app::skill::install(*agent, dir.path()).unwrap();
        }
        dir
    });
    // The TempDir lives in the OnceLock for the whole run.
    dir.path()
}
