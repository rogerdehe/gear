//! All integration tests of gear-engine in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.

/// Tests that onboard a `grok` lane (its trust file is written under `HOME`)
/// or set `HOME` themselves take this lock: `HOME` is process-wide, so two
/// such tests at once write into each other's home.
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

mod backoff_contract;
mod cancel_tick_contract;
mod config_profile_onboard;
mod contract_update;
mod deliver_confirm_contract;
mod deliver_contract;
mod dispatch_contract;
mod dispatch_tick_contract;
mod doorbell_contract;
mod doorbell_isolation;
mod eng_confirm;
mod eng_execute;
mod eng_intend_recover;
mod eng_reconcile;
mod feed_contract;
mod lane_view_contract;
mod onboard_contract;
mod onboard_run_contract;
mod reviewer_nudge_contract;
mod settle_review_tick;
mod slice1_messaging;
mod slice2_dispatch;
mod stall_contract;

mod captain_quota_contract;
mod hand_in_notifications;
mod station_contract;
mod busy_contract;
mod blocked_contract;
mod parallel_contract;
mod multi_repo_contract;

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
