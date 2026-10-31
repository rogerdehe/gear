//! All integration tests of gear-app in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
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

mod admin_init_contract;
mod adopt_contract;
mod app_home;
mod app_identity;
mod app_reply;
mod app_select_batch;
mod app_send;
mod backoff_home_contract;
mod bar_contract;
mod base_gate;
mod cancel_contract;
mod captain_handover_contract;
mod captain_take_contract;
mod config_change_contract;
mod config_contract;
mod coverage_contract;
mod decision_contract;
mod draft_contract;
mod github_contract;
mod home_task_contract;
mod integrated_by_contract;
mod kickoff_contract;
mod multi_captain_contract;
mod packet_contract;
mod plan_apply_contract;
mod plan_fields_contract;
mod plan_size_contract;
mod plan_staff_contract;
mod plan_tools_contract;
mod plan_verdicts_contract;
mod profiles_contract;
mod queue_contract;
mod remerge_contract;
mod result_contract;
mod retired_lane_contract;
mod review_contract;
mod review_model_contract;
mod reviewer_mail_contract;
mod seats_contract;
mod settle_review_contract;
mod staffing_lock_contract;
mod standard_contract;
mod station_contract;
mod tail_contract;
mod tasks_contract;
mod temps_quota_contract;
mod time_contract;
mod time_report_contract;
mod verdict_item_contract;
mod voyage_contract;
mod voyage_report_contract;

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

/// Copy a repository tree. A `git clone` of one is a process and a few hundred
/// milliseconds; a copy of the same files is a walk of the directory — so a
/// git fixture is built once per test file and copied per test (T-128).
pub fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Point a copied clone at this test's own remote. A clone's `url` is its only
/// absolute path, and the only thing a copy cannot share (T-128).
pub fn point_at(repo: &std::path::Path, origin: &std::path::Path) {
    let config = repo.join(".git/config");
    let text = std::fs::read_to_string(&config).unwrap();
    let url = format!("\turl = {}", origin.display());
    let rewritten: Vec<String> = text
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("url = ") {
                url.clone()
            } else {
                line.to_string()
            }
        })
        .collect();
    std::fs::write(&config, rewritten.join("\n") + "\n").unwrap();
}
