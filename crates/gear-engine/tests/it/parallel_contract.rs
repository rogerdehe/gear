//! T-51: actual Git branch comparisons, persistent pair state and doorbells.
use gear_app::{home, identity::Fingerprint, Env};
use gear_engine::{parallel, tick, EngineCfg};
use gear_herdr::{fake::FakeHerdr, AgentStatus};
use gear_model::{AttemptState, GearCtx, MsgState, SeatRole, SeatState, TaskState};
use gear_store::{testing, AttemptRow, Store, TaskRow};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const T0: i64 = 1_800_000_000;
const SHARED: &str = "shared file.rs";
/// A source file: two tasks that change it conflict.
const SRC: &str = "src/lib.rs";
/// A test file: two tasks that change it do not conflict (T-188).
const TEST_FILE: &str = "tests/lib_test.rs";
/// A contract file: a test file by another name (T-188).
const CONTRACT_FILE: &str = "src/thing_contract.rs";

/// The base repo every test starts from: `dev` with the fixture files,
/// built once per test process and copied per test (conventions "Test time":
/// a git fixture is built once and a copy costs a copy, not ten git calls).
fn template() -> &'static Path {
    static TEMPLATE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("repo");
            fs::create_dir(&path).unwrap();
            git(&path, &["init", "-q", "-b", "dev"]);
            git(&path, &["config", "user.name", "test"]);
            git(&path, &["config", "user.email", "a@b"]);
            git(&path, &["config", "commit.gpgsign", "false"]);
            git(&path, &["config", "core.hooksPath", "/dev/null"]);
            for file in [
                SHARED,
                "other.rs",
                "third.rs",
                SRC,
                TEST_FILE,
                CONTRACT_FILE,
            ] {
                let full = path.join(file);
                if let Some(dir) = full.parent() {
                    fs::create_dir_all(dir).unwrap();
                }
                fs::write(full, "base\n").unwrap();
            }
            git(&path, &["add", "."]);
            git(&path, &["commit", "-qm", "base"]);
            root
        })
        .path()
        .join("repo")
        .leak()
}

/// Copy a tree in this process: a copied fixture costs a copy, not a shell
/// and a `cp` on the test's budget.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

struct Repo {
    root: tempfile::TempDir,
    path: PathBuf,
    base: String,
}
impl Repo {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        copy_tree(template(), &path);
        let base = git(&path, &["rev-parse", "dev"]);
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
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "codex",
                    T0,
                )?;
                for (lane, pane) in [("heat", "w1:p2"), ("steady", "w1:p3")] {
                    testing::seed_active_seat(tx, "p", lane, SeatRole::Lane, pane, "opencode", T0)?;
                    tx.set_seat_state(
                        &format!("p:{lane}"),
                        SeatState::Held,
                        Some("isolated Git scan fixture"),
                        T0,
                    )?;
                }
                for (n, lane) in [(1, "heat"), (2, second_lane)] {
                    tx.insert_task(&TaskRow {
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        project: "p".into(),
                        id: format!("T-{n}"),
                        rank: n,
                        state: TaskState::Running,
                        priority: "must".into(),
                        lane: Some(lane.into()),
                        milestone: None,
                        required: false,
                        contract_version: 1,
                        deps: vec![],
                        followup_of: None,
                        wait_on: None,
                        wait_cond: None,
                        rework_note: None,
                        estimate_min: Some(30),
                        issue: None,
                        updated: T0,
                    })?;
                }
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = format!(
                    r#"
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
"#,
                    serde_json::json!(self.path.to_string_lossy()).to_string()
                );
                tx.update_project(&project)
            })
            .unwrap();
        store
    }
    fn store(&self) -> Store {
        self.seed(Store::open_in_memory().unwrap(), "steady")
    }
}

/// A child repo below the project root (T-53): its own integration branch
/// and its own branches.
struct Child {
    path: PathBuf,
}
impl Child {
    fn new(root: &Path, integration: &str) -> Child {
        let path = root.join("web");
        fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", integration]);
        git(&path, &["config", "user.name", "test"]);
        git(&path, &["config", "user.email", "a@b"]);
        git(&path, &["config", "commit.gpgsign", "false"]);
        git(&path, &["config", "core.hooksPath", "/dev/null"]);
        for file in [SHARED, "other.rs"] {
            fs::write(path.join(file), "base\n").unwrap();
        }
        git(&path, &["add", "."]);
        git(&path, &["commit", "-qm", "base"]);
        Child { path }
    }
    fn branch(&self, name: &str, file: &str) {
        git(&self.path, &["switch", "-qc", name]);
        fs::write(self.path.join(file), name).unwrap();
        git(&self.path, &["add", "--", file]);
        git(&self.path, &["commit", "-qm", "change"]);
    }
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn scan(store: &mut Store, seconds: i64) -> parallel::Check {
    parallel::check(store, "p", T0 + seconds).unwrap()
}
/// The information line of a task that overlaps the integration branch
/// (T-188): the board and `gear queue` read it, the captain is not told.
fn integration_info(store: &mut Store, task: &str) -> Option<String> {
    integration_line(store, "p", task)
}
/// The information line of one project's task (T-281): a project of the store
/// may be scanned in the same test.
fn integration_line(store: &mut Store, project: &str, task: &str) -> Option<String> {
    store
        .read(|tx| tx.get_meta(&format!("parallel:{project}:integration:{task}")))
        .unwrap()
}
fn pair_items(store: &mut Store) -> Vec<gear_store::MessageRow> {
    store
        .read(|tx| tx.thread_messages("p", "parallel:tasks:T-1:T-2"))
        .unwrap()
}
fn hand_in(store: &mut Store, project: &str, task: &str, seat: &str, n: i64, candidate: &str) {
    store
        .write(|tx| {
            tx.insert_attempt(&AttemptRow {
                session_ref: None,
                session_cli: None,
                session_action: None,
                sent_at: None,
                project: project.into(),
                task: task.into(),
                n,
                seat: seat.into(),
                generation: 1,
                contract_version: 1,
                adopted_version: 1,
                pending_update: None,
                state: AttemptState::Submitted,
                candidate: Some(candidate.into()),
                report: None,
                safe_point: None,
                started: T0,
                updated: T0 + 1,
            })
        })
        .unwrap();
}

/// T-188: a pair of tasks rings once — for its lifetime. Another shared file
/// does not ring the captain again, and an overlap that clears and comes back
/// does not either: the captain answered "known" once.
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
    assert!(
        item.body.contains("T-1 (heat) and T-2 (steady)"),
        "{}",
        item.body
    );
    assert!(item.body.contains(SHARED), "{}", item.body);
    assert_eq!(item.state, MsgState::Open);
    store
        .write(|tx| {
            tx.update_message_state(
                "p",
                &item.id,
                MsgState::Resolved,
                Some("ordered the work"),
                None,
                None,
                T0 + 1,
            )
        })
        .unwrap();
    assert!(scan(&mut store, 600).opened.is_empty());
    assert_eq!(
        pair_items(&mut store).len(),
        1,
        "acknowledgment cannot repeat the same set"
    );

    // Another shared file: the pair does not ring again (T-188).
    repo.change("heat/t1", "other.rs", "heat");
    repo.change("steady/t2", "other.rs", "steady");
    assert!(
        scan(&mut store, 1200).opened.is_empty(),
        "a pair rings once"
    );
    let items = pair_items(&mut store);
    assert_eq!(
        items.len(),
        1,
        "one item for the pair's lifetime: {items:?}"
    );
    assert_eq!(items[0].state, MsgState::Resolved);

    // The overlap clears and comes back: still no second item.
    let steady = git(&repo.path, &["rev-parse", "steady/t2"]);
    git(
        &repo.path,
        &["update-ref", "refs/heads/steady/t2", &repo.base],
    );
    assert!(scan(&mut store, 1800).opened.is_empty());
    git(&repo.path, &["update-ref", "refs/heads/steady/t2", &steady]);
    assert!(
        scan(&mut store, 2400).opened.is_empty(),
        "the marker stays when the overlap clears"
    );
    assert_eq!(
        pair_items(&mut store).len(),
        1,
        "one item for the pair's lifetime"
    );

    // Another pair rings once of its own.
    repo.branch("steady/t3", SHARED);
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-3".into(),
                rank: 3,
                state: TaskState::Running,
                priority: "must".into(),
                lane: Some("steady".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: T0,
            })
        })
        .unwrap();
    let checked = scan(&mut store, 3000);
    assert_eq!(
        checked.opened.len(),
        1,
        "another pair rings once: {checked:?}"
    );
    let items = store
        .read(|tx| tx.thread_messages("p", "parallel:tasks:T-1:T-3"))
        .unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].state, MsgState::Open);
}

#[test]
fn legacy_root_overlap_keeps_its_item_and_resolves_it() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let mut store = repo.store();
    // Seed the pre-multi-repo namespace, including its original open item.
    let item = store
        .write(|tx| {
            let item = tx.insert_message(&gear_store::NewMessage {
                meta: None,
                project: "p".into(),
                thread: "parallel:tasks:T-1:T-2".into(),
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: "p:captain".into(),
                kind: gear_model::MsgKind::System,
                urgent: false,
                body: "legacy overlap".into(),
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: T0 - 600,
            })?;
            tx.set_meta("parallel:p:pairs", r#"["tasks:T-1:T-2"]"#)?;
            tx.set_meta(
                "parallel:p:pair:tasks:T-1:T-2:files",
                &serde_json::json!([SHARED]).to_string(),
            )?;
            tx.set_meta("parallel:p:pair:tasks:T-1:T-2:item", &item.id)?;
            Ok(item.id)
        })
        .unwrap();
    assert!(
        scan(&mut store, 0).opened.is_empty(),
        "upgrade must not duplicate an alert"
    );
    assert_eq!(pair_items(&mut store).len(), 1);
    for branch in ["heat/t1", "steady/t2"] {
        git(
            &repo.path,
            &["update-ref", &format!("refs/heads/{branch}"), &repo.base],
        );
    }
    assert!(scan(&mut store, 600).opened.is_empty());
    let items = pair_items(&mut store);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, item);
    assert_eq!(items[0].state, MsgState::Resolved);
}

#[test]
fn disjoint_and_same_lane_branches_raise_nothing() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", "other.rs");
    assert!(scan(&mut repo.store(), 0).opened.is_empty());
    repo.branch("heat/t2", SHARED);
    let mut store = repo.seed(Store::open_in_memory().unwrap(), "heat");
    assert!(
        scan(&mut store, 0).opened.is_empty(),
        "only different lanes overlap"
    );
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
        assert!(
            scan(&mut store, 599).opened.is_empty(),
            "not polling on every tick"
        );
        if handoff {
            hand_in(
                &mut store,
                "p",
                "T-2",
                "p:steady",
                1,
                &git(&repo.path, &["rev-parse", "steady/t2"]),
            );
            assert_eq!(
                scan(&mut store, 599).opened.len(),
                1,
                "hand-in bypasses interval"
            );
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
    git(
        &repo.path,
        &["update-ref", "refs/remotes/origin/dev", &repo.base],
    );
    repo.change("dev", SHARED, "integration changed it");
    let mut store = repo.store();
    // Accepted work awaiting merge remains an open branch.
    store
        .write(|tx| {
            let mut t = tx.get_task("p", "T-1")?.unwrap();
            t.state = TaskState::Done;
            tx.update_task(&t)
        })
        .unwrap();
    let result = scan(&mut store, 0);
    // An overlap with the integration branch is an information line, never a
    // captain item (T-188).
    assert!(result.opened.is_empty(), "no captain item: {result:?}");
    assert!(
        pair_items(&mut store).is_empty(),
        "disjoint task branches remain disjoint"
    );
    let info = integration_info(&mut store, "T-1").expect("the information line");
    assert!(info.contains(SHARED), "{info}");
    assert!(info.contains("\"integration\":\"dev\""), "{info}");
}

#[test]
fn cleared_overlaps_resolve_and_the_pair_does_not_ring_again() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let steady = git(&repo.path, &["rev-parse", "steady/t2"]);
    let mut store = repo.store();
    assert_eq!(scan(&mut store, 0).opened.len(), 1);
    git(
        &repo.path,
        &["update-ref", "refs/heads/steady/t2", &repo.base],
    );
    assert!(scan(&mut store, 600).opened.is_empty());
    assert_eq!(pair_items(&mut store)[0].state, MsgState::Resolved);
    // The overlap comes back: the pair has already rung (T-188).
    git(&repo.path, &["update-ref", "refs/heads/steady/t2", &steady]);
    assert!(
        scan(&mut store, 1200).opened.is_empty(),
        "a pair rings once for its lifetime"
    );
    assert_eq!(pair_items(&mut store).len(), 1);
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
    assert!(
        scan(&mut store, 601).problems.is_empty(),
        "failed reads back off instead of every-tick retry"
    );
    fs::rename(offline, &repo.path).unwrap();
    assert!(scan(&mut store, 1200).opened.is_empty());
}

#[test]
fn remote_tracking_task_refs_work_and_pair_state_survives_reopening() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let heat = git(&repo.path, &["rev-parse", "heat/t1"]);
    git(
        &repo.path,
        &["update-ref", "refs/remotes/origin/heat/t1", &heat],
    );
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
fn a_pair_rings_once_through_the_engine() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let mut store = repo.store();
    let mut herdr = FakeHerdr::new();
    herdr.add_agent("w1:p1", "codex", AgentStatus::Idle);
    let mut cfg = EngineCfg::default();
    cfg.profiles
        .insert("codex".into(), gear_herdr::builtin("codex").unwrap());
    let first = tick(&mut store, &mut herdr, &cfg, T0).unwrap();
    assert_eq!(first.parallel.len(), 1);
    assert!(first.parallel_failed.is_empty());
    assert_eq!(herdr.prompts_to("w1:p1").len(), 1);
    let env = Env {
        ctx: Some(GearCtx {
            project: "p".into(),
            seat: "captain".into(),
            generation: 1,
        }),
        pane_id: Some("w1:p1".into()),
        admin: false,
    };
    let fp = Fingerprint {
        pane_id: "w1:p1".into(),
        kind: Some("codex".into()),
        session_ref: None,
    };
    let page = home::build(&mut store, &env, Some(&fp), T0 + 1).unwrap();
    home::record_view(&mut store, &page.view).unwrap();
    tick(&mut store, &mut herdr, &cfg, T0 + 2).unwrap();
    assert!(tick(&mut store, &mut herdr, &cfg, T0 + 600)
        .unwrap()
        .parallel
        .is_empty());
    assert!(tick(&mut store, &mut herdr, &cfg, T0 + 900)
        .unwrap()
        .parallel
        .is_empty());
    assert!(tick(&mut store, &mut herdr, &cfg, T0 + 1200)
        .unwrap()
        .parallel
        .is_empty());
    assert_eq!(
        herdr.prompts_to("w1:p1").len(),
        1,
        "unchanged overlap must not get a stale-item reminder"
    );
    // Another shared file: the pair does not ring again (T-188).
    repo.change("heat/t1", "third.rs", "heat third");
    repo.change("steady/t2", "third.rs", "steady third");
    assert!(tick(&mut store, &mut herdr, &cfg, T0 + 1800)
        .unwrap()
        .parallel
        .is_empty());
    assert_eq!(
        herdr.prompts_to("w1:p1").len(),
        1,
        "a pair rings once for its lifetime"
    );
    let items = pair_items(&mut store);
    assert_eq!(items.len(), 1, "one item for the pair: {items:?}");
    assert_eq!(items[0].state, MsgState::Open);
}

/// A project with a child repo (T-53): the lane "heat" works in the root,
/// the lanes "steady" and "web2" in the child repo.
fn multi_repo_store(root: &Repo) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            for (lane, pane) in [("heat", "w1:p2"), ("steady", "w1:p3"), ("web2", "w1:p4")] {
                testing::seed_active_seat(tx, "p", lane, SeatRole::Lane, pane, "opencode", T0)?;
                tx.set_seat_state(
                    &format!("p:{lane}"),
                    SeatState::Held,
                    Some("isolated Git scan fixture"),
                    T0,
                )?;
            }
            for (n, lane) in [(1, "heat"), (2, "steady"), (3, "web2")] {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: format!("T-{n}"),
                    rank: n,
                    state: TaskState::Running,
                    priority: "must".into(),
                    lane: Some(lane.into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(30),
                    issue: None,
                    updated: T0,
                })?;
            }
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = format!(
                r#"
[project]
id = "p"
herdr_session = "sess"
repo = {root}

[[repos]]
name = "web"
path = "web"
integration = "web-dev"

[quota]
lanes = 3
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "steady"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
repo = "web"

[[lanes]]
name = "web2"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
repo = "web"
"#,
                root = serde_json::json!(root.path.to_string_lossy()).to_string()
            );
            tx.update_project(&project)
        })
        .unwrap();
    store
}

/// T-53: the scan runs per repo. The task of the root repo and the task of
/// the child repo change the same file name and are not compared: only the
/// tasks whose lanes share a repo are.
#[test]
fn tasks_of_different_repos_are_never_compared() {
    let root = Repo::new();
    let child = Child::new(&root.path, "web-dev");
    root.branch("heat/t1", SHARED);
    child.branch("steady/t2", SHARED);
    child.branch("web2/t3", SHARED);
    let mut store = multi_repo_store(&root);

    let checked = scan(&mut store, 0);

    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(
        checked.opened.len(),
        1,
        "only the two tasks of the child repo overlap"
    );
    let item = store
        .read(|tx| tx.get_message("p", &checked.opened[0]))
        .unwrap()
        .unwrap();
    assert!(
        item.body.contains("T-2 (steady) and T-3 (web2)"),
        "{}",
        item.body
    );
    assert!(
        item.body.contains("in web"),
        "the item names the repo: {}",
        item.body
    );
    assert!(item.body.contains(SHARED), "{}", item.body);
    assert!(
        !item.body.contains("T-1"),
        "the task of the root repo is another repo: {}",
        item.body
    );
}

/// T-53: the scan of a repo reads that repo's own integration branch: a
/// change on the child's branch is a change on its integration branch.
#[test]
fn the_scan_of_a_child_repo_reads_its_own_integration_branch() {
    let root = Repo::new();
    let child = Child::new(&root.path, "web-dev");
    child.branch("steady/t2", SHARED);
    // The child's integration branch moved: the shared file is now on it.
    git(&child.path, &["switch", "-q", "web-dev"]);
    fs::write(child.path.join(SHARED), "integration\n").unwrap();
    git(&child.path, &["add", "--", SHARED]);
    git(&child.path, &["commit", "-qm", "integration"]);
    let mut store = multi_repo_store(&root);

    let checked = scan(&mut store, 0);

    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    // An overlap with the integration branch is an information line, never a
    // captain item (T-188).
    assert!(checked.opened.is_empty(), "{:?}", checked.opened);
    let info = store
        .read(|tx| tx.get_meta("parallel:p:web:integration:T-2"))
        .unwrap()
        .expect("the information line of the child repo");
    assert!(info.contains(SHARED), "{info}");
    assert!(info.contains("web-dev"), "{info}");
}

/// T-64: a task whose candidate is already in the integration branch has
/// landed. The engine merges the approved candidate, not the branch (T-61), so
/// a branch may still point past it; that leftover is not an open change. An
/// open task is still paired with another open one on the same file.
#[test]
fn a_landed_task_is_not_an_open_change_and_open_tasks_still_pair() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", "other.rs");
    // T-1 handed in this commit, and the integration branch merged it.
    let candidate = git(&repo.path, &["rev-parse", "heat/t1"]);
    let mut store = repo.store();
    hand_in(&mut store, "p", "T-1", "p:heat", 1, &candidate);
    git(&repo.path, &["switch", "-q", "dev"]);
    git(
        &repo.path,
        &["merge", "-q", "--no-ff", "-m", "merge T-1", "heat/t1"],
    );
    // Another task landed on the integration branch and touched the same file.
    repo.change("dev", SHARED, "another task landed");
    // The branch moves past the merged candidate: a commit no merge will take.
    repo.change("heat/t1", SHARED, "heat after the hand-in");

    let checked = scan(&mut store, 0);
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert!(
        checked.opened.is_empty(),
        "a merged task raises no parallel item: {:?}",
        checked.opened
    );
    assert!(
        pair_items(&mut store).is_empty(),
        "nor a pair with an open task"
    );

    // A newer attempt on T-1 is not merged: T-1 is open again, and two open
    // branches on the same file still pair.
    let rework = git(&repo.path, &["rev-parse", "heat/t1"]);
    hand_in(&mut store, "p", "T-1", "p:heat", 2, &rework);
    repo.change("steady/t2", SHARED, "steady shares it");
    assert!(!scan(&mut store, 600).opened.is_empty());
    let pair = pair_items(&mut store);
    assert_eq!(pair.len(), 1, "{pair:?}");
    assert!(
        pair[0].body.contains("T-1 (heat) and T-2 (steady)"),
        "{}",
        pair[0].body
    );
    assert!(pair[0].body.contains(SHARED), "{}", pair[0].body);
}

/// T-431: a candidate git cannot resolve (a commit this clone lacks, or a
/// branch name stored before T-421) is no candidate: the task is judged by its
/// branch as before, and one such task never stops the scan for the whole
/// project. A task whose branch does not resolve either is skipped.
#[test]
fn an_unresolvable_candidate_does_not_stop_the_scan() {
    let repo = Repo::new();
    repo.branch("heat/t1", SHARED);
    repo.branch("steady/t2", SHARED);
    let mut store = repo.store();
    // A done task handed in a ref this clone does not have (T-431), and its
    // branch does not resolve either: the scan skips it, it never fails.
    store
        .write(|tx| {
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-3".into(),
                rank: 3,
                state: TaskState::Done,
                priority: "must".into(),
                lane: Some("steady".into()),
                milestone: None,
                required: false,
                contract_version: 1,
                deps: vec![],
                followup_of: None,
                wait_on: None,
                wait_cond: None,
                rework_note: None,
                estimate_min: Some(30),
                issue: None,
                updated: T0,
            })
        })
        .unwrap();
    hand_in(&mut store, "p", "T-3", "p:steady", 1, "scan/t12");

    let checked = scan(&mut store, 0);
    assert!(
        checked.problems.is_empty(),
        "one bad candidate fails nothing: {:?}",
        checked.problems
    );
    assert_eq!(
        checked.opened.len(),
        1,
        "the open pair is still reported: {checked:?}"
    );
    let pair = pair_items(&mut store);
    assert_eq!(pair.len(), 1, "{pair:?}");
    assert!(
        pair[0].body.contains("T-1 (heat) and T-2 (steady)"),
        "{}",
        pair[0].body
    );
    assert!(pair[0].body.contains(SHARED), "{}", pair[0].body);
}

/// T-188: an overlap with the integration branch is an information line (the
/// board and `gear queue` show it), never a captain item: the lane merges
/// dev in and the merge gate catches a conflict.
#[test]
fn a_task_that_overlaps_the_integration_branch_makes_no_captain_item() {
    let repo = Repo::new();
    repo.branch("heat/t1", SRC);
    let mut store = repo.store();
    // The integration branch moved on and changed the same file.
    repo.change("dev", SRC, "integration changed it");
    let result = scan(&mut store, 0);
    assert!(result.problems.is_empty());
    assert!(result.opened.is_empty(), "no captain item: {result:?}");
    // Nothing at all was written to the captain's inbox.
    let inbox = store
        .read(|tx| tx.actionable_for("p", "p:captain", T0 + 1))
        .unwrap();
    assert!(inbox.is_empty(), "{inbox:?}");
    // The information line is there for the board and `gear queue`.
    let info = integration_info(&mut store, "T-1").expect("the information line");
    assert!(info.contains(SRC), "{info}");

    // The overlap is gone: the information line goes with it.
    git(&repo.path, &["update-ref", "refs/heads/dev", &repo.base]);
    assert!(scan(&mut store, 600).opened.is_empty());
    assert!(integration_info(&mut store, "T-1")
        .unwrap_or_default()
        .is_empty());
}

/// T-188: two tasks that share only a test file do not conflict: a test that
/// is written twice is not a merge conflict.
#[test]
fn two_tasks_that_share_only_a_test_file_make_no_item() {
    let repo = Repo::new();
    repo.branch("heat/t1", TEST_FILE);
    repo.branch("steady/t2", TEST_FILE);
    let mut store = repo.store();
    let result = scan(&mut store, 0);
    assert!(result.problems.is_empty());
    assert!(
        result.opened.is_empty(),
        "no item for a shared test file: {result:?}"
    );
    assert!(pair_items(&mut store).is_empty());
}

/// T-188: a contract file is a test file by another name: it does not count
/// either.
#[test]
fn two_tasks_that_share_only_a_contract_file_make_no_item() {
    let repo = Repo::new();
    repo.branch("heat/t1", CONTRACT_FILE);
    repo.branch("steady/t2", CONTRACT_FILE);
    let mut store = repo.store();
    let result = scan(&mut store, 0);
    assert!(result.problems.is_empty());
    assert!(
        result.opened.is_empty(),
        "a contract file does not count: {result:?}"
    );
    assert!(pair_items(&mut store).is_empty());
}

/// T-188: two tasks that share a source file ring once per pair.
#[test]
fn two_tasks_that_share_a_source_file_make_one_item_once() {
    let repo = Repo::new();
    repo.branch("heat/t1", SRC);
    repo.branch("steady/t2", SRC);
    let mut store = repo.store();
    let result = scan(&mut store, 0);
    assert_eq!(result.opened.len(), 1, "{result:?}");
    let item = pair_items(&mut store).remove(0);
    assert!(
        item.body.contains("T-1 (heat) and T-2 (steady)"),
        "{}",
        item.body
    );
    assert!(item.body.contains(SRC), "{}", item.body);

    // The same pair rings once: the next scan opens nothing.
    assert!(scan(&mut store, 600).opened.is_empty());
    assert_eq!(pair_items(&mut store).len(), 1, "one item per pair");
}

/// T-281: a project of the store on a repo, with the `{project}/{lane}/t{n}`
/// branch template that T-256 requires of projects sharing a repository: the
/// branches are p/heat/t1 and q/heat/t1, not heat/t1 twice.
fn seed_on(
    tx: &gear_store::Tx<'_>,
    project: &str,
    repo: &Path,
    panes: &str,
    lanes: &[&str],
) -> gear_model::GearResult<()> {
    testing::seed_project(tx, project, "sess", T0)?;
    testing::seed_active_seat(
        tx,
        project,
        "captain",
        SeatRole::Captain,
        &format!("{panes}:p1"),
        "codex",
        T0,
    )?;
    for (index, lane) in lanes.iter().enumerate() {
        testing::seed_active_seat(
            tx,
            project,
            lane,
            SeatRole::Lane,
            &format!("{panes}:p{}", index + 2),
            "opencode",
            T0,
        )?;
        tx.set_seat_state(
            &format!("{project}:{lane}"),
            SeatState::Held,
            Some("isolated Git scan fixture"),
            T0,
        )?;
    }
    let sections: String = lanes
        .iter()
        .map(|lane| {
            format!(
                r#"
[[lanes]]
name = "{lane}"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
"#
            )
        })
        .collect();
    let mut row = tx.get_project(project)?.unwrap();
    row.config_toml = format!(
        r#"
[project]
id = "{project}"
herdr_session = "sess"
repo = {repo}

[quota]
lanes = {count}
temps = 0

[captain]
pane = "{panes}:p1"
kind = "codex"
profile = "codex"
{sections}
[conventions]
task_branch = "{{project}}/{{lane}}/t{{n}}"
"#,
        repo = serde_json::json!(repo.to_string_lossy()).to_string(),
        count = lanes.len(),
    );
    tx.update_project(&row)
}

fn add_task(
    tx: &gear_store::Tx<'_>,
    project: &str,
    id: &str,
    rank: i64,
    lane: &str,
) -> gear_model::GearResult<()> {
    tx.insert_task(&TaskRow {
        repo: None,
        base: None,
        gate: None,
        rework_estimate_min: None,
        project: project.into(),
        id: id.into(),
        rank,
        state: TaskState::Running,
        priority: "must".into(),
        lane: Some(lane.into()),
        milestone: None,
        required: false,
        contract_version: 1,
        deps: vec![],
        followup_of: None,
        wait_on: None,
        wait_cond: None,
        rework_note: None,
        estimate_min: Some(30),
        issue: None,
        updated: T0,
    })
    .map(|_| ())
}

/// One store with the given projects, each on the repo it is given (T-281):
/// two entries with one path are two fleets on one repository, two paths are
/// two repositories.
fn shared_store(projects: &[(&str, &Path, &[&str])]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            for (index, (project, repo, lanes)) in projects.iter().enumerate() {
                seed_on(tx, project, repo, &format!("w{}", index + 1), lanes)?;
            }
            Ok(())
        })
        .unwrap();
    store
}

fn projects_of(store: &mut Store, projects: &[(&str, &str, &str)]) {
    store
        .write(|tx| {
            for (project, id, lane) in projects {
                add_task(tx, project, id, 1, lane)?;
            }
            Ok(())
        })
        .unwrap();
}

/// T-281: two fleets work in one repository. Each project's own pass writes
/// its own captain the pair, so both captains hear of it before the merge
/// conflicts, and the item names the other side with its project.
#[test]
fn two_projects_on_one_repo_each_hear_of_a_shared_pair() {
    let repo = Repo::new();
    repo.branch("p/heat/t1", SRC);
    repo.branch("q/heat/t1", SRC);
    let mut store = shared_store(&[
        ("p", repo.path.as_path(), &["heat"]),
        ("q", repo.path.as_path(), &["heat"]),
    ]);
    projects_of(&mut store, &[("p", "T-1", "heat"), ("q", "T-1", "heat")]);

    let for_p = parallel::check(&mut store, "p", T0).unwrap();
    assert!(for_p.problems.is_empty(), "{:?}", for_p.problems);
    assert_eq!(for_p.opened.len(), 1, "{for_p:?}");
    let item = store
        .read(|tx| tx.get_message("p", &for_p.opened[0]))
        .unwrap()
        .unwrap();
    assert_eq!(item.recipient, "p:captain");
    assert!(
        item.body
            .starts_with("parallel changes: T-1 (heat) and T-1 (q/heat) share files:"),
        "{}",
        item.body
    );
    assert!(item.body.contains(SRC), "{}", item.body);

    let for_q = parallel::check(&mut store, "q", T0).unwrap();
    assert_eq!(for_q.opened.len(), 1, "{for_q:?}");
    let item = store
        .read(|tx| tx.get_message("q", &for_q.opened[0]))
        .unwrap()
        .unwrap();
    assert_eq!(item.recipient, "q:captain");
    assert!(
        item.body.contains("T-1 (heat) and T-1 (p/heat)"),
        "{}",
        item.body
    );
    assert!(item.body.contains(SRC), "{}", item.body);

    // The pair rings once for each captain: the next pass opens nothing.
    assert!(parallel::check(&mut store, "p", T0 + 600)
        .unwrap()
        .opened
        .is_empty());
    assert!(parallel::check(&mut store, "q", T0 + 600)
        .unwrap()
        .opened
        .is_empty());
}

/// T-281: a project on another repository shares no file, even under the same
/// name: only projects whose lane works in the same directory are compared.
#[test]
fn a_project_working_in_another_repository_is_not_paired() {
    let repo = Repo::new();
    let elsewhere = Repo::new();
    repo.branch("p/heat/t1", SRC);
    elsewhere.branch("r/heat/t1", SRC);
    let mut store = shared_store(&[
        ("p", repo.path.as_path(), &["heat"]),
        ("r", elsewhere.path.as_path(), &["heat"]),
    ]);
    projects_of(&mut store, &[("p", "T-1", "heat"), ("r", "T-1", "heat")]);

    let for_p = parallel::check(&mut store, "p", T0).unwrap();
    assert!(for_p.problems.is_empty(), "{:?}", for_p.problems);
    assert!(
        for_p.opened.is_empty(),
        "one file name in two repositories is not one file: {for_p:?}"
    );
    assert!(parallel::check(&mut store, "r", T0)
        .unwrap()
        .opened
        .is_empty());
}

/// T-281: another project's two tasks are that project's pair. This project's
/// captain is not told about work it does not share.
#[test]
fn two_tasks_of_another_project_alone_raise_no_item_here() {
    let repo = Repo::new();
    repo.branch("p/heat/t1", "other.rs");
    repo.branch("q/heat/t1", SRC);
    repo.branch("q/steady/t2", SRC);
    let mut store = shared_store(&[
        ("p", repo.path.as_path(), &["heat"]),
        ("q", repo.path.as_path(), &["heat", "steady"]),
    ]);
    projects_of(
        &mut store,
        &[
            ("p", "T-1", "heat"),
            ("q", "T-1", "heat"),
            ("q", "T-2", "steady"),
        ],
    );

    assert!(
        parallel::check(&mut store, "p", T0)
            .unwrap()
            .opened
            .is_empty(),
        "another project's own pair is not this captain's item"
    );
    // q hears of its own pair: each project's own pass writes its item.
    let for_q = parallel::check(&mut store, "q", T0).unwrap();
    assert_eq!(for_q.opened.len(), 1, "{for_q:?}");
    let item = store
        .read(|tx| tx.get_message("q", &for_q.opened[0]))
        .unwrap()
        .unwrap();
    assert!(
        item.body.contains("T-1 (heat) and T-2 (steady)"),
        "{}",
        item.body
    );
}

/// T-281: another project's hand-in restarts this project's scan inside the
/// back-off period: the work it just handed in may share a file now.
#[test]
fn another_projects_hand_in_restarts_this_projects_scan() {
    let repo = Repo::new();
    repo.branch("p/heat/t1", SRC);
    repo.branch("q/heat/t1", "other.rs");
    let mut store = shared_store(&[
        ("p", repo.path.as_path(), &["heat"]),
        ("q", repo.path.as_path(), &["heat"]),
    ]);
    projects_of(&mut store, &[("p", "T-1", "heat"), ("q", "T-1", "heat")]);
    assert!(parallel::check(&mut store, "p", T0)
        .unwrap()
        .opened
        .is_empty());
    assert!(
        parallel::check(&mut store, "p", T0 + 599)
            .unwrap()
            .opened
            .is_empty(),
        "unchanged, so the back-off holds"
    );

    // q hands in work that now changes the same file.
    repo.change("q/heat/t1", SRC, "q shares it now");
    let candidate = git(&repo.path, &["rev-parse", "q/heat/t1"]);
    hand_in(&mut store, "q", "T-1", "q:heat", 1, &candidate);
    let checked = parallel::check(&mut store, "p", T0 + 599).unwrap();
    assert_eq!(
        checked.opened.len(),
        1,
        "another project's hand-in bypasses the interval: {checked:?}"
    );
    let item = store
        .read(|tx| tx.get_message("p", &checked.opened[0]))
        .unwrap()
        .unwrap();
    assert!(
        item.body.contains("T-1 (heat) and T-1 (q/heat)"),
        "{}",
        item.body
    );
    assert!(item.body.contains(SRC), "{}", item.body);
}

/// T-281: an overlap with the integration branch is the task's own project's
/// information line. This project's scan records none for another project's
/// task, whose id may even be the same.
#[test]
fn another_projects_integration_overlap_is_its_own_information_line() {
    let repo = Repo::new();
    repo.branch("p/heat/t1", "other.rs");
    repo.branch("q/heat/t1", SRC);
    repo.change("dev", SRC, "integration changed it");
    let mut store = shared_store(&[
        ("p", repo.path.as_path(), &["heat"]),
        ("q", repo.path.as_path(), &["heat"]),
    ]);
    projects_of(&mut store, &[("p", "T-1", "heat"), ("q", "T-1", "heat")]);

    assert!(parallel::check(&mut store, "p", T0)
        .unwrap()
        .opened
        .is_empty());
    assert!(
        integration_line(&mut store, "p", "T-1").is_none(),
        "another project's overlap is not this project's line"
    );
    assert!(parallel::check(&mut store, "q", T0)
        .unwrap()
        .opened
        .is_empty());
    let line =
        integration_line(&mut store, "q", "T-1").expect("its own project's information line");
    assert!(line.contains(SRC), "{line}");
}
