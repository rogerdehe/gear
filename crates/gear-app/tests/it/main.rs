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
mod decision_contract;
mod draft_contract;
mod github_contract;
mod home_task_contract;
mod integrated_by_contract;
mod kickoff_contract;
mod packet_contract;
mod plan_apply_contract;
mod plan_fields_contract;
mod plan_size_contract;
mod plan_staff_contract;
mod plan_verdicts_contract;
mod profiles_contract;
mod result_contract;
mod remerge_contract;
mod review_contract;
mod reviewer_mail_contract;
mod seats_contract;
mod settle_review_contract;
mod staffing_lock_contract;
mod tasks_contract;
mod temps_quota_contract;
mod time_contract;
mod time_report_contract;
mod verdict_item_contract;
mod queue_contract;
mod retired_lane_contract;
mod voyage_contract;
mod voyage_report_contract;
mod standard_contract;
mod station_contract;
mod tail_contract;

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
        .map(|line| if line.trim_start().starts_with("url = ") { url.clone() } else { line.to_string() })
        .collect();
    std::fs::write(&config, rewritten.join("\n") + "\n").unwrap();
}
