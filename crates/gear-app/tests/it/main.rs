//! All integration tests of gear-app in one binary: each test binary is scanned by macOS on its first run, so one binary per crate keeps the gate fast.
mod admin_init_contract;
mod adopt_contract;
mod app_home;
mod app_identity;
mod app_reply;
mod app_select_batch;
mod app_send;
mod base_gate;
mod cancel_contract;
mod backoff_home_contract;
mod captain_handover_contract;
mod captain_take_contract;
mod config_change_contract;
mod config_contract;
mod draft_contract;
mod github_contract;
mod home_task_contract;
mod kickoff_contract;
mod packet_contract;
mod plan_apply_contract;
mod plan_staff_contract;
mod plan_verdicts_contract;
mod profiles_contract;
mod result_contract;
mod review_contract;
mod seats_contract;
mod settle_review_contract;
mod tasks_contract;
mod time_contract;
mod time_report_contract;
mod verdict_item_contract;
mod voyage_contract;
mod standard_contract;
mod station_contract;

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
