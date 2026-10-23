//! T-51: actual Git branch comparisons, persistent pair state and doorbells.
use std::{fs, path::{Path, PathBuf}, process::Command};
use gear_app::{home, identity::Fingerprint, Env};
use gear_engine::{parallel, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, GearCtx, MsgState, SeatRole, SeatState, TaskState};
use gear_store::{testing, AttemptRow, Store, TaskRow};

const T0: i64 = 1_800_000_000;
const SHARED: &str = "shared file.rs";

struct Repo { root: tempfile::TempDir, path: PathBuf, base: String }
impl Repo {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q", "-b", "dev"]);
        git(&path, &["config", "user.name", "test"]);
        git(&path, &["config", "user.email", "a@b"]);
        git(&path, &["config", "commit.gpgsign", "false"]);
        git(&path, &["config", "core.hooksPath", "/dev/null"]);
        for file in [SHARED, "other.rs", "third.rs"] { fs::write(path.join(file), "base\n").unwrap(); }
        git(&path, &["add", "."]);
        git(&path, &["commit", "-qm", "base"]);
        let base = git(&path, &["rev-parse", "HEAD"]);
        Self { root, path, base }
    }
    fn branch(&self, name: &str, file: &str) {
        git(&self.path, &["switch", "-qc", name, &self.base]);
        self.change(name, file, name);
    }
    fn change(&self, branch: &str, file: &str, text: &str) {
        git(&self.path, &["switch", "-q", branch]);
        fs::write(self.path.join(file), text).unwrap();
        git(&self.path, &["add", "--", file]);
        git(&self.path, &["commit", "-qm", "change"]);
    }
    fn seed(&self, mut store: Store, second_lane: &str) -> Store {
        store.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            for (lane, pane) in [("heat", "w1:p2"), ("steady", "w1:p3")] {
                testing::seed_active_seat(tx, "p", lane, SeatRole::Lane, pane, "opencode", T0)?;
                tx.set_seat_state(&format!("p:{lane}"), SeatState::Held, Some("isolated Git scan fixture"), T0)?;
            }
            for (n, lane) in [(1, "heat"), (2, second_lane)] {
                tx.insert_task(&TaskRow {
                    project: "p".into(), id: format!("T-{n}"), rank: n, state: TaskState::Running,
                    priority: "must".into(), lane: Some(lane.into()), milestone: None, required: false,
                    contract_version: 1, deps: vec![], followup_of: None, wait_on: None,
                    wait_cond: None, rework_note: None, estimate_min: Some(30), updated: T0,
                })?;
            }
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = format!(r#"
[project]
id = "p"
herdr_session = "sess"
repo = {}
[quota]
lanes = 2
temps = 0
[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"
"#, serde_json::json!(self.path.to_string_lossy()).to_string());
            tx.update_project(&project)
        }).unwrap();
        store
    }
    fn store(&self) -> Store { self.seed(Store::open_in_memory().unwrap(), "steady") }
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn scan(store: &mut Store, seconds: i64) -> parallel::Check {
    parallel::check(store, "p", T0+seconds).unwrap()
}
fn pair_items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store.read(|tx| tx.thread_messages("p", "parallel:tasks:T-1:T-2")).unwrap()
}
fn hand_in(store: &mut Store, candidate: &str) {
    store.write(|tx| tx.insert_attempt(&AttemptRow {
        project: "p".into(), task: "T-2".into(), n: 1, seat: "p:steady".into(), generation: 1,
        contract_version: 1, adopted_version: 1, pending_update: None,
        state: AttemptState::Submitted, candidate: Some(candidate.into()), report: None,
        safe_point: None, started: T0, updated: T0+1,
    })).unwrap();
}

#[test]
fn overlapping_branches_name_both_tasks_and_the_shared_file_once() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let mut store = repo.store();
    let checked = scan(&mut store, 0);
    assert!(checked.problems.is_empty());
    assert_eq!(checked.opened.len(), 1);
    let item = pair_items(&mut store).remove(0);
    assert!(item.body.contains("T-1 (heat) and T-2 (steady)"), "{}", item.body);
    assert!(item.body.contains(SHARED), "{}", item.body);
    assert_eq!(item.state, MsgState::Open);
    store.write(|tx| tx.update_message_state("p", &item.id, MsgState::Resolved,
        Some("ordered the work"), None, None, T0+1)).unwrap();
    assert!(scan(&mut store, 600).opened.is_empty());
    assert_eq!(pair_items(&mut store).len(), 1, "acknowledgment cannot repeat the same set");
    repo.change("heat/t1", "other.rs", "heat");
    repo.change("steady/t2", "other.rs", "steady");
    assert_eq!(scan(&mut store, 1200).opened.len(), 1);
    let items = pair_items(&mut store);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].state, MsgState::Resolved);
    assert_eq!(items[1].state, MsgState::Open);
    assert!(items[1].body.contains("other.rs"));
    assert!(items[1].body.contains(SHARED));
    assert!(scan(&mut store, 1800).opened.is_empty());
}

#[test]
fn disjoint_and_same_lane_branches_raise_nothing() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", "other.rs");
    assert!(scan(&mut repo.store(), 0).opened.is_empty());
    repo.branch("heat/t2", SHARED);
    let mut store = repo.seed(Store::open_in_memory().unwrap(), "heat");
    assert!(scan(&mut store, 0).opened.is_empty(), "only different lanes overlap");
}

#[test]
fn periodic_scan_and_new_hand_in_trigger_at_their_required_cadence() {
    for handoff in [false, true] {
        let repo = Repo::new();
        repo.branch("heat/t1", SHARED);
        repo.branch("steady/t2", "other.rs");
        let mut store = repo.store();
        assert!(scan(&mut store, 0).opened.is_empty());
        repo.change("steady/t2", SHARED, "new shared change");
        assert!(scan(&mut store, 599).opened.is_empty(), "not polling on every tick");
        if handoff {
            hand_in(&mut store, &git(&repo.path, &["rev-parse", "steady/t2"]));
            assert_eq!(scan(&mut store, 599).opened.len(), 1, "hand-in bypasses interval");
        } else {
            assert_eq!(scan(&mut store, 600).opened.len(), 1, "ten-minute boundary");
        }
    }
}

#[test]
fn integration_changes_since_the_branch_base_count_too() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", "other.rs");
    // Local integration can be ahead of its remote-tracking ref.
    git(&repo.path, &["update-ref", "refs/remotes/origin/dev", &repo.base]);
    repo.change("dev", SHARED, "integration changed it");
    let mut store = repo.store();
    // Accepted work awaiting merge remains an open branch.
    store.write(|tx| { let mut t = tx.get_task("p", "T-1")?.unwrap(); t.state = TaskState::Done; tx.update_task(&t) }).unwrap();
    let result = scan(&mut store, 0);
    assert_eq!(result.opened.len(), 1);
    let item = store.read(|tx| tx.get_message("p", &result.opened[0])).unwrap().unwrap();
    assert!(item.body.contains("T-1 (heat) and integration dev"), "{}", item.body);
    assert!(item.body.contains(SHARED));
    assert!(pair_items(&mut store).is_empty(), "disjoint task branches remain disjoint");
}

#[test]
fn cleared_overlaps_resolve_and_can_report_a_later_recurrence() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let steady = git(&repo.path, &["rev-parse", "steady/t2"]);
    let mut store = repo.store();
    assert_eq!(scan(&mut store, 0).opened.len(), 1);
    git(&repo.path, &["update-ref", "refs/heads/steady/t2", &repo.base]);
    assert!(scan(&mut store, 600).opened.is_empty());
    assert_eq!(pair_items(&mut store)[0].state, MsgState::Resolved);
    git(&repo.path, &["update-ref", "refs/heads/steady/t2", &steady]);
    assert_eq!(scan(&mut store, 1200).opened.len(), 1, "empty-to-shared is a set change");
    assert_eq!(pair_items(&mut store).len(), 2);
}

#[test]
fn git_read_failure_does_not_claim_that_existing_overlap_went_away() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let mut store = repo.store();
    assert_eq!(scan(&mut store, 0).opened.len(), 1);
    let offline = repo.root.path().join("offline");
    fs::rename(&repo.path, &offline).unwrap();
    let result = scan(&mut store, 600);
    assert_eq!(result.problems.len(), 1);
    assert!(result.opened.is_empty());
    assert_eq!(pair_items(&mut store)[0].state, MsgState::Open);
    assert!(scan(&mut store, 601).problems.is_empty(), "failed reads back off instead of every-tick retry");
    fs::rename(offline, &repo.path).unwrap();
    assert!(scan(&mut store, 1200).opened.is_empty());
}

#[test]
fn remote_tracking_task_refs_work_and_pair_state_survives_reopening() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let heat = git(&repo.path, &["rev-parse", "heat/t1"]);
    git(&repo.path, &["update-ref", "refs/remotes/origin/heat/t1", &heat]);
    git(&repo.path, &["branch", "-D", "heat/t1"]);
    let db = repo.root.path().join("gear.db");
    let mut store = repo.seed(Store::open(&db).unwrap(), "steady");
    assert_eq!(scan(&mut store, 0).opened.len(), 1);
    drop(store);
    let mut store = Store::open(&db).unwrap();
    assert!(scan(&mut store, 600).opened.is_empty());
    assert_eq!(pair_items(&mut store).len(), 1);
}

#[test]
fn fresh_file_sets_get_fresh_ids_and_ring_through_the_engine() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let mut store = repo.store();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "codex", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.profiles.insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    let first = tick(&mut store, &mut herdr, &cfg, T0).unwrap();
    assert_eq!(first.parallel.len(), 1);
    assert!(first.parallel_failed.is_empty());
    assert_eq!(herdr.prompts_to("w1:p1").len(), 1);
    let env = Env { ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }), pane_id: Some("w1:p1".into()), admin: false };
    let fp = Fingerprint { pane_id: "w1:p1".into(), kind: Some("codex".into()), session_ref: None };
    let page = home::build(&mut store, &env, Some(&fp), T0+1).unwrap();
    home::record_view(&mut store, &page.view).unwrap();
    tick(&mut store, &mut herdr, &cfg, T0+2).unwrap();
    assert!(tick(&mut store, &mut herdr, &cfg, T0+600).unwrap().parallel.is_empty());
    assert_eq!(herdr.prompts_to("w1:p1").len(), 1);
    repo.change("heat/t1", "third.rs", "heat third");
    repo.change("steady/t2", "third.rs", "steady third");
    assert_eq!(tick(&mut store, &mut herdr, &cfg, T0+1200).unwrap().parallel.len(), 1);
    assert_eq!(herdr.prompts_to("w1:p1").len(), 2, "a changed set rings despite a view of the previous item");
    let items = pair_items(&mut store);
    assert_eq!(items[0].state, MsgState::Resolved);
    assert_eq!(items[1].state, MsgState::Open);
}
