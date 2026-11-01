//! T-51: read-only Git overlap checks on hand-in and every ten minutes.
//! Git runs outside store transactions against pinned commit ids. Published
//! integration and local task refs are read without fetch or worktree changes.
//!
//! T-53: the scan runs per repo. A task's repo is its lane's repo, so only
//! tasks of the same repo are compared: two lanes in two repos share no file,
//! and each repo is scanned against its own integration branch.
//!
//! T-64: a task whose candidate is already in the integration branch has
//! landed. The engine merges the approved candidate, not the branch (T-61),
//! and a branch may still point past it — at a commit the merge never took.
//! Such a task is not an open change: it is skipped, so a merged task raises
//! no item and an open task is not paired with work that already landed.
//!
//! T-281: several fleets may work in one repository. A project's open tasks
//! are compared with the open tasks of every other project of the store whose
//! lane works in the same directory, so two fleets that edit one file hear of
//! it before the merge conflicts. Each project's own pass writes its own
//! captain's item; a store's other projects only, never another store's.
//!
//! T-527: a pass starts a few git processes, not one per ref. Every branch tip
//! the scan needs comes from one `for-each-ref` over `refs/heads` and
//! `refs/remotes`, and the candidates the store names from one
//! `cat-file --batch-check`; `revision` answers from those two maps. An
//! ancestry answer for a pair of full commit ids is kept for the life of the
//! process, like the diff cache below: the answer never changes.
//!
//! T-435: a diff of two commits is remembered for the life of the process.
//! The scan diffs the same pair every ten minutes and at every hand-in; on a
//! large repository that reads cost one project's tick 41.7 s. A pair of full
//! commit ids names one result that never changes, so the second read is the
//! cache's; an error from git is not a result and is not kept.

use gear_app::config::Repo;
use gear_model::{GearResult, MsgKind, MsgState, TaskState, Ts};
use gear_store::{NewMessage, Store, TaskRow, Tx};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
};

const PERIOD: i64 = 600;

/// The diffs of commit pairs already read (T-435): keyed by the repo and the
/// two full commit ids of the pair, which never change for a key.
static DIFFS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<(PathBuf, String, String), BTreeSet<String>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// The entries the cache holds before it is cleared (T-435): an engine runs
/// for weeks, so the map is bounded rather than grown forever.
const DIFFS_MAX: usize = 4096;

/// The diffs the cache missed (T-435): the test counts git reads through it,
/// so the test build carries it and no other build does.
#[cfg(test)]
static DIFF_MISSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The ancestry answers already read (T-527): keyed by the repo and the two
/// full commit ids of the pair, which never change for a key. A scan asks
/// `merge-base --is-ancestor` for the same pairs every ten minutes and at every
/// hand-in; a process start per pair was most of the scan's on-CPU time.
static ANCESTORS: std::sync::LazyLock<std::sync::Mutex<HashMap<(PathBuf, String, String), bool>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// The answers the cache holds before it is cleared (T-527), as the diffs'
/// (T-435): an engine runs for weeks, so the map is bounded.
const ANCESTORS_MAX: usize = 4096;

/// The `for-each-ref` reads (T-527) and the `cat-file` reads: the test counts
/// git processes through them, so the test build carries them and no other
/// build does.
#[cfg(test)]
static REF_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static CAT_FILE_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The ancestry reads (T-527): the test proves the second answer runs no git.
#[cfg(test)]
static ANCESTOR_MISSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The key of the project root in the meta: a repo name never starts with a
/// dash.
const ROOT: &str = "-";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub opened: Vec<String>,
    /// Git failures are evidence gaps, not an empty file set or a fatal tick.
    pub problems: Vec<String>,
}

/// One task of the scan: its branch is the open change, unless the work it
/// handed in has already landed.
struct TaskIn {
    /// The project the task belongs to: a task of the scanned project or of
    /// another project whose lane works in the same repo (T-281).
    project: String,
    task: String,
    lane: String,
    /// The task's own project's `[conventions].task_branch` template (T-253):
    /// a task of another project is read from its own project's names.
    task_branch: String,
    /// The candidate of the task's latest attempt: the commit the engine
    /// merges when the review passes (T-61), and the one that tells a landed
    /// task from an open one (T-64).
    candidate: Option<String>,
}

/// One scan: the repo, the tasks of that repo and the state it remembers.
struct Input {
    /// The repo's key in the meta: its name, "-" for the project root.
    key: String,
    /// The project the scan belongs to.
    project: String,
    repo: PathBuf,
    integration: String,
    /// The project's `[conventions].remote` (T-262): the remote the scan reads
    /// the branches from.
    remote: String,
    tasks: Vec<TaskIn>,
    handins: String,
}

struct Branch {
    /// The project the task belongs to: two tasks are on different lanes when
    /// their `(project, lane)` differs (T-281).
    project: String,
    task: String,
    lane: String,
    files: BTreeSet<String>,
}

#[derive(Debug)]
struct Conflict {
    left: String,
    right: String,
    files: Vec<String>,
}

/// What one scan found: the pairs of tasks that share a file (the captain is
/// told about them) and the tasks that overlap the integration branch (an
/// information line, never a captain item).
#[derive(Debug, Default)]
struct Findings {
    pairs: BTreeMap<String, Conflict>,
    integration: BTreeMap<String, Conflict>,
}

/// Is this path a test file? Two tasks that touch only tests do not conflict
/// (T-188): a test file is not a source file, and a test that is written
/// twice is not a merge conflict.
fn is_test_file(path: &str) -> bool {
    let path = path.replace('\\', "/");
    path.split('/').any(|part| part == "tests")
        || path.ends_with("_test.rs")
        || path.ends_with("_contract.rs")
}

/// The shared files that count: the ones that are not test files.
fn real_files(files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|file| !is_test_file(file))
        .cloned()
        .collect()
}

pub fn check(store: &mut Store, project: &str, now: Ts) -> GearResult<Check> {
    let inputs = inputs(store, project, now)?;
    let mut report = Check::default();
    for input in inputs {
        let prefix = input.prefix();
        let scanned = scan(&input);
        let result = store.write(|tx| {
            // A failed read backs off too; preserve known pair state, because a
            // read failure is not evidence that an overlap disappeared.
            tx.set_meta(&format!("{prefix}:at"), &now.to_string())?;
            tx.set_meta(&format!("{prefix}:handins"), &input.handins)?;
            let conflicts = match &scanned {
                Ok(conflicts) => conflicts,
                // A read that failed names the repo it failed in: a project of
                // several repos has one read each.
                Err(error) => {
                    let error = match input.name() {
                        Some(name) => format!("{name}: {error}"),
                        None => error.clone(),
                    };
                    return Ok(Check { opened: vec![], problems: vec![error] });
                }
            };            let Some(captain) = tx.captain_of(project)? else { return Ok(Check::default()); };
            let previous: Vec<String> = tx.get_meta(&format!("{prefix}:pairs"))?
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default();
            // An older engine kept the overlaps with the integration branch in
            // the same list: they are separated here, so their items close.
            let (previous_integration, previous): (Vec<String>, Vec<String>) =
                previous.into_iter().partition(|pair| pair.starts_with("integration:"));
            // The information lines of the last scan: the ones that are gone
            // are cleared.
            let previous_lines: Vec<String> = tx.get_meta(&format!("{prefix}:integrations"))?
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                .unwrap_or_default();
            let mut opened = Vec::new();
            // The repo the overlap is in: the captain must know which one.
            let where_ = match input.name() {
                Some(name) => format!(" in {name}"),
                None => String::new(),
            };
            for (pair, conflict) in &conflicts.pairs {
                let key = format!("{prefix}:pair:{pair}");
                let files = serde_json::json!(conflict.files).to_string();
                // A pair rings once (T-188): the marker stays, even when the
                // overlap clears, so a pair that shares one more file later
                // does not ring the captain again. The file set is kept
                // current: it is what the captain's item and the board show.
                let notified = tx.get_meta(&format!("{key}:notified"))?.is_some_and(|value| !value.is_empty())
                    // An older engine kept the item's id instead of a marker:
                    // a pair that already has an item is notified too.
                    || tx.get_meta(&format!("{key}:item"))?.is_some_and(|value| !value.is_empty());
                tx.set_meta(&format!("{key}:files"), &files)?;
                if notified {
                    // The item is the pair's one notification; its text
                    // follows the files the pair shares now.
                    let text = format!(
                        "parallel changes{where_}: {} and {} share files: {files}",
                        conflict.left, conflict.right
                    );
                    if let Some(id) = tx.get_meta(&format!("{key}:item"))? {
                        if let Some(item) = tx.get_message(project, &id)? {
                            if item.body != text
                                && matches!(item.state, MsgState::Open | MsgState::Waiting)
                            {
                                tx.update_message_body(project, &id, &text)?;
                            }
                        }
                    }
                    continue;
                }
                let message = tx.insert_message(&NewMessage {
                    meta: None,
                    project: project.into(), thread: format!("parallel:{pair}"), task: None, attempt: None,
                    sender: "engine".into(), sender_gen: 0, recipient: captain.id.clone(),
                    kind: MsgKind::System, urgent: false,
                    body: format!("parallel changes{where_}: {} and {} share files: {files}", conflict.left, conflict.right),
                    reply_to: None, needs_ack: false, state: MsgState::Open, created: now,
                })?;
                tx.set_meta(&format!("{key}:files"), &files)?;
                tx.set_meta(&format!("{key}:item"), &message.id)?;
                tx.set_meta(&format!("{key}:notified"), "1")?;
                opened.push(message.id);
            }
            // An overlap with the integration branch is an information line
            // (the board and `gear queue` show it), never a captain item:
            // the lane merges dev in and the merge gate catches a conflict
            // (T-188).
            for (pair, conflict) in &conflicts.integration {
                let task = pair.strip_prefix("integration:").unwrap_or(pair);
                let key = format!("{prefix}:integration:{task}");
                let info = serde_json::json!({
                    "task": task,
                    "integration": input.integration,
                    "files": conflict.files,
                });
                tx.set_meta(&key, &info.to_string())?;
            }
            for pair in previous
                .iter()
                .chain(previous_integration.iter())
                .filter(|pair| !conflicts.pairs.contains_key(*pair))
            {
                let key = format!("{prefix}:pair:{pair}");
                // An overlap with the integration branch was an item before
                // (T-188): it closes with its own reason.
                let reason = if pair.starts_with("integration:") {
                    "an overlap with the integration branch is an information line now"
                } else {
                    "the branches no longer share changed files"
                };
                close(tx, project, &key, reason, now)?;
                tx.set_meta(&format!("{key}:files"), "[]")?;
            }
            // The integration lines that are gone: the board shows none.
            for task in &previous_lines {
                let key = format!("{prefix}:integration:{task}");
                if !conflicts.integration.contains_key(&format!("integration:{task}")) {
                    tx.set_meta(&key, "")?;
                }
            }
            tx.set_meta(
                &format!("{prefix}:integrations"),
                &serde_json::json!(conflicts.integration.keys()
                    .filter_map(|pair| pair.strip_prefix("integration:"))
                    .collect::<Vec<_>>())
                .to_string(),
            )?;
            tx.set_meta(&format!("{prefix}:pairs"), &serde_json::json!(conflicts.pairs.keys().collect::<Vec<_>>()).to_string())?;
            Ok(Check { opened, problems: vec![] })
        })?;
        report.opened.extend(result.opened);
        report.problems.extend(result.problems);
    }
    Ok(report)
}

impl Input {
    /// The prefix of the meta this scan keeps its state under: the project's
    /// and the repo's, so two repos never share a back-off or a pair set.
    fn prefix(&self) -> String {
        prefix_of(&self.project, &self.key)
    }

    /// The repo's name, when it is a child repo: the project root has none,
    /// and its messages read as they always did.
    fn name(&self) -> Option<&str> {
        if self.key == ROOT {
            None
        } else {
            Some(&self.key)
        }
    }
}

/// The prefix of the meta of a repo's scan.
fn prefix_of(project: &str, key: &str) -> String {
    // Keep the pre-multi-repo root namespace and its existing alert IDs.
    if key == ROOT {
        format!("parallel:{project}")
    } else {
        format!("parallel:{project}:{key}")
    }
}

/// The scans of one project: one per repo that has tasks (T-53). A task
/// belongs to the repo of its lane, and only tasks of the same repo are
/// compared with each other.
fn inputs(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<Input>> {
    store.read(|tx| {
        let Some(row) = tx.get_project(project)? else {
            return Ok(Vec::new());
        };
        let Ok(config) = gear_app::config::parse_row(&row) else {
            return Ok(Vec::new());
        };
        let tasks = tx.tasks_of_project(project)?;
        // The tasks of the project, grouped by the repo of their lane.
        let mut groups: BTreeMap<String, (Repo, Vec<TaskRow>)> = BTreeMap::new();
        for task in &tasks {
            let Some(lane) = task.lane.as_deref() else {
                continue;
            };
            let repo = config.repo_of_lane(lane);
            groups
                .entry(repo_key(&repo))
                .or_insert_with(|| (repo.clone(), Vec::new()))
                .1
                .push(task.clone());
        }
        // The other projects of the store (T-281): a fleet that works in the
        // same repo edits the same files, so its open tasks are compared with
        // this project's. A project whose config does not parse has no lane
        // repo to compare and is not seen. Another store (another Herdr
        // session) is not in this list either: that is a later view.
        let others: Vec<(String, gear_app::config::ProjectConfig)> = tx
            .project_ids()?
            .into_iter()
            .filter(|other| other != project)
            .filter_map(|other| {
                let row = tx.get_project(&other).ok().flatten()?;
                // The row keys its own entry (T-518): reading it under this
                // project's name would overwrite this project's config.
                let config = gear_app::config::parse_row(&row).ok()?;
                Some((other, config))
            })
            .collect();
        let mut inputs = Vec::new();
        for (key, (repo, tasks)) in groups {
            // The hand-ins of this repo: a new one starts the scan early. The
            // latest candidate of every task is the one a merge would take
            // (T-64), so the scan can tell a landed task from an open one. A
            // hand-in of another project in this repo is in here too (T-281):
            // its work restarts this project's scan as well.
            let mut handins = Vec::new();
            let mut candidates: BTreeMap<(String, String), String> = BTreeMap::new();
            let mut entries = Vec::new();
            for task in &tasks {
                // Oldest first: the last candidate is the latest attempt's.
                for (n, candidate) in attempted(tx, project, &task.id)? {
                    candidates.insert((project.to_string(), task.id.clone()), candidate.clone());
                    handins.push((project.to_string(), task.id.clone(), n, candidate));
                }
                let Some(lane) = task.lane.clone() else {
                    continue;
                };
                if !open(&task.state) {
                    continue;
                }
                let candidate = candidates
                    .get(&(project.to_string(), task.id.clone()))
                    .cloned();
                entries.push(TaskIn {
                    project: project.to_string(),
                    task: task.id.clone(),
                    lane,
                    task_branch: config.conventions.task_branch.clone(),
                    candidate,
                });
            }
            // The open tasks of every other project whose lane works in this
            // repo: the same directory, after canonicalizing where both paths
            // canonicalize, as written where either does not.
            for (other, other_config) in &others {
                for task in tx.tasks_of_project(other)? {
                    if !open(&task.state) {
                        continue;
                    }
                    let Some(lane) = task.lane.clone() else {
                        continue;
                    };
                    if !same_dir(&other_config.repo_of_lane(&lane).path, &repo.path) {
                        continue;
                    }
                    for (n, candidate) in attempted(tx, other, &task.id)? {
                        candidates.insert((other.clone(), task.id.clone()), candidate.clone());
                        handins.push((other.clone(), task.id.clone(), n, candidate));
                    }
                    let candidate = candidates.get(&(other.clone(), task.id.clone())).cloned();
                    entries.push(TaskIn {
                        project: other.clone(),
                        task: task.id.clone(),
                        lane,
                        task_branch: other_config.conventions.task_branch.clone(),
                        candidate,
                    });
                }
            }
            handins.sort();
            let handins = serde_json::json!([row.config_version, handins]).to_string();
            // The back-off of this repo: nothing changed and the last scan
            // was recent, so this repo waits for the next period.
            let prefix = prefix_of(project, &key);
            let last = tx
                .get_meta(&format!("{prefix}:at"))?
                .and_then(|s| s.parse::<Ts>().ok());
            let unchanged = tx.get_meta(&format!("{prefix}:handins"))?.as_deref() == Some(&handins);
            if unchanged && last.is_some_and(|last| now >= last && now - last < PERIOD) {
                continue;
            }
            inputs.push(Input {
                key,
                project: project.to_string(),
                repo: repo.path.into(),
                integration: repo.integration,
                remote: config.conventions.remote.clone(),
                tasks: entries,
                handins,
            });
        }
        Ok(inputs)
    })
}

/// Is the task an open change? A draft, cancelled or failed task is not one:
/// it is not compared with anything.
fn open(state: &TaskState) -> bool {
    !matches!(
        state,
        TaskState::Draft | TaskState::Cancelled | TaskState::Failed
    )
}

/// The candidates of a task, oldest first: the last one is the latest
/// attempt's (T-64), and every one is a hand-in that starts the scan early.
fn attempted(tx: &Tx<'_>, project: &str, task: &str) -> GearResult<Vec<(i64, String)>> {
    let mut attempted = Vec::new();
    for attempt in tx.attempts_of_task(project, task)? {
        if let Some(candidate) = attempt.candidate {
            attempted.push((attempt.n, candidate));
        }
    }
    Ok(attempted)
}

/// Do two configs name one directory (T-281)? Equal after canonicalizing, or
/// equal as written when either path does not canonicalize (it may not exist
/// yet).
fn same_dir(left: &str, right: &str) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// The key of a repo in the meta: its name, or "-" for the project root (a
/// repo name never starts with a dash).
fn repo_key(repo: &Repo) -> String {
    if repo.name.is_empty() {
        ROOT.to_string()
    } else {
        repo.name.clone()
    }
}

fn close(tx: &Tx<'_>, project: &str, key: &str, reason: &str, now: Ts) -> GearResult<()> {
    if let Some(id) = tx.get_meta(&format!("{key}:item"))? {
        if let Some(message) = tx.get_message(project, &id)? {
            if matches!(message.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(
                    project,
                    &id,
                    MsgState::Resolved,
                    Some(reason),
                    None,
                    None,
                    now,
                )?;
            }
        }
    }
    Ok(())
}

fn scan(input: &Input) -> Result<Findings, String> {
    git(&input.repo, &["rev-parse", "--git-dir"])?;
    // T-527: one pass reads every branch tip and resolves every candidate, so
    // the scan starts two git processes for them however many refs it asks for.
    let candidates: Vec<String> = input
        .tasks
        .iter()
        .filter_map(|task| task.candidate.clone())
        .collect();
    let tips = Tips::read(&input.repo, &candidates)?;
    let local = tips.revision(&format!("refs/heads/{}", input.integration));
    let remote = tips.revision(&format!(
        "refs/remotes/{}/{}",
        input.remote, input.integration
    ));
    // Use the containing tip when one is ahead; the remote is authoritative
    // when the local integration ref diverged. No network operation is needed.
    let integration = match (local, remote) {
        (Some(local), Some(remote)) if ancestor(&input.repo, &remote, &local)? => local,
        (_, Some(remote)) => remote,
        (Some(local), None) => local,
        _ => {
            return Err(format!(
                "integration branch {} is unavailable",
                input.integration
            ))
        }
    };
    let mut branches = Vec::new();
    let mut findings = Findings::default();
    for task in &input.tasks {
        // A task whose candidate is already in the integration branch has
        // landed: the engine merged that commit, and whatever its branch still
        // points at is work no merge will take (T-64). It is not an open
        // change, so it neither conflicts with integration nor pairs with an
        // open task.
        // A candidate git cannot resolve (a commit this clone lacks, or a
        // branch name stored before T-421) is no candidate: the task is judged
        // by its branch below, and one such task never fails the whole repo's
        // scan (T-431).
        if let Some(candidate) = &task.candidate {
            if let Some(candidate) = tips.revision(candidate) {
                if ancestor(&input.repo, &candidate, &integration)? {
                    continue;
                }
            }
        }
        let name =
            crate::dispatch::task_branch(&task.task_branch, &task.project, &task.lane, &task.task);
        let local = tips.revision(&format!("refs/heads/{name}"));
        let head = match local {
            Some(head) => Some(head),
            None => tips.revision(&format!("refs/remotes/{}/{name}", input.remote)),
        };
        let Some(head) = head else {
            continue;
        };
        let files = changed(&input.repo, &integration, &head)?;
        if files.is_empty() {
            continue;
        } // includes already integrated Done branches
          // An overlap with the integration branch is this project's business:
          // another project's task that changed the same file is not reported
          // by this project's scan, its own project reports it (T-281).
        if task.project == input.project {
            let integration_files = changed(&input.repo, &head, &integration)?;
            let shared: Vec<_> = files.intersection(&integration_files).cloned().collect();
            let shared = real_files(&shared);
            if !shared.is_empty() {
                findings.integration.insert(
                    format!("integration:{}", task.task),
                    Conflict {
                        left: format!("{} ({})", task.task, task.lane),
                        right: format!("integration {}", input.integration),
                        files: shared,
                    },
                );
            }
        }
        branches.push(Branch {
            project: task.project.clone(),
            task: task.task.clone(),
            lane: task.lane.clone(),
            files,
        });
    }
    // Deterministic, and the scanned project's own pairs keep the order they
    // always had: by task id.
    branches.sort_by(|a, b| (&a.task, &a.project, &a.lane).cmp(&(&b.task, &b.project, &b.lane)));
    for (index, left) in branches.iter().enumerate() {
        for right in branches.iter().skip(index + 1) {
            // Two tasks are on different lanes when `(project, lane)` differs.
            if (left.project.as_str(), left.lane.as_str())
                == (right.project.as_str(), right.lane.as_str())
            {
                continue;
            }
            // A pair with two other projects' tasks is their captains' item,
            // not this one's: at least one side is this project's (T-281).
            if left.project != input.project && right.project != input.project {
                continue;
            }
            let files: Vec<_> = left.files.intersection(&right.files).cloned().collect();
            let files = real_files(&files);
            if !files.is_empty() {
                // This project's task names the pair, the other side carries
                // its project: each project's own pass writes its own item.
                let (main, sibling) = if left.project != input.project {
                    (right, left)
                } else {
                    (left, right)
                };
                let (key, right) = match sibling.project == input.project {
                    true => (
                        format!("tasks:{}:{}", main.task, sibling.task),
                        format!("{} ({})", sibling.task, sibling.lane),
                    ),
                    false => (
                        format!("tasks:{}:{}/{}", main.task, sibling.project, sibling.task),
                        format!("{} ({}/{})", sibling.task, sibling.project, sibling.lane),
                    ),
                };
                findings.pairs.insert(
                    key,
                    Conflict {
                        left: format!("{} ({})", main.task, main.lane),
                        right,
                        files,
                    },
                );
            }
        }
    }
    Ok(findings)
}

/// The branch tips and the candidate commits one pass reads (T-527): a map of
/// the refs `refs/heads` and `refs/remotes` hold, and the candidates the store
/// named that git knows as commits. `revision` answers from these two maps
/// where it used to start a `rev-parse --verify` process per call.
struct Tips {
    refs: HashMap<String, String>,
    commits: HashMap<String, String>,
}

impl Tips {
    /// Read the repo's branch tips and resolve the candidates, one git process
    /// each.
    fn read(repo: &Path, candidates: &[String]) -> Result<Tips, String> {
        let output = git(
            repo,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/heads",
                "refs/remotes",
            ],
        )?;
        #[cfg(test)]
        REF_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut refs = HashMap::new();
        for line in String::from_utf8_lossy(&output).lines() {
            let Some((id, name)) = line.split_once(' ') else {
                continue;
            };
            if name.starts_with("refs/") && !id.is_empty() {
                refs.insert(name.to_string(), id.to_string());
            }
        }
        let mut commits = HashMap::new();
        if !candidates.is_empty() {
            let mut child = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["cat-file", "--batch-check"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            {
                use std::io::Write;
                let mut stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| "cat-file: no stdin".to_string())?;
                for candidate in candidates {
                    writeln!(stdin, "{candidate}").map_err(|e| e.to_string())?;
                }
                // The write is done: the child reads no more and exits.
            }
            let output = child.wait_with_output().map_err(|e| e.to_string())?;
            #[cfg(test)]
            CAT_FILE_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if !output.status.success() {
                return Err(format!(
                    "cannot read the candidates: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            // `<what was asked> <type> <size>`, or `<what was asked> missing`.
            // Only a commit is a commit id: a candidate that names a tree, a
            // blob or nothing git knows is no candidate (T-431).
            for (line, candidate) in String::from_utf8_lossy(&output.stdout)
                .lines()
                .zip(candidates)
            {
                let fields: Vec<&str> = line.split(' ').collect();
                if fields.len() == 3 && fields[1] == "commit" {
                    commits.insert(candidate.clone(), candidate.clone());
                }
            }
        }
        Ok(Tips { refs, commits })
    }

    /// The commit a reference names, as `rev-parse --verify` answered it: a
    /// branch the pass read, or a candidate git resolved to a commit.
    fn revision(&self, reference: &str) -> Option<String> {
        if let Some(id) = self.refs.get(reference) {
            return Some(id.clone());
        }
        self.commits.get(reference).cloned()
    }
}

/// Is `from` an ancestor of `to`? Read from git the first time a process asks
/// for a pair, and from the cache after that (T-527): a pair of full commit ids
/// names one answer that never changes. An error from git is no answer and is
/// not kept.
fn ancestor(repo: &Path, from: &str, to: &str) -> Result<bool, String> {
    let key = (repo.to_path_buf(), from.to_string(), to.to_string());
    if let Some(answer) = ANCESTORS
        .lock()
        .ok()
        .and_then(|answers| answers.get(&key).copied())
    {
        return Ok(answer);
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", from, to])
        .output()
        .map_err(|e| e.to_string())?;
    let answer = match output.status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => return Err(String::from_utf8_lossy(&output.stderr).into_owned()),
    };
    #[cfg(test)]
    ANCESTOR_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut answers) = ANCESTORS.lock() {
        if answers.len() >= ANCESTORS_MAX {
            answers.clear();
        }
        answers.insert(key, answer);
    }
    Ok(answer)
}

/// The files that changed between two commits: read from git the first time a
/// scan asks for a pair, and from the cache after that (T-435). A failed read
/// keeps nothing, so the next scan asks git again.
fn changed(repo: &Path, from: &str, to: &str) -> Result<BTreeSet<String>, String> {
    let key = (repo.to_path_buf(), from.to_string(), to.to_string());
    if let Some(files) = DIFFS.lock().ok().and_then(|diffs| diffs.get(&key).cloned()) {
        return Ok(files);
    }
    let output = git(
        repo,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "-z",
            &format!("{from}...{to}"),
            "--",
        ],
    )?;
    let files: BTreeSet<String> = output
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    #[cfg(test)]
    DIFF_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut diffs) = DIFFS.lock() {
        if diffs.len() >= DIFFS_MAX {
            diffs.clear();
        }
        diffs.insert(key, files.clone());
    }
    Ok(files)
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::{SeatRole, TaskState};
    use gear_store::{testing, Store, TaskRow};
    use std::sync::atomic::Ordering;

    const T0: i64 = 1_800_000_000;

    /// The integration file the two branches share.
    const SRC: &str = "src/lib.rs";

    /// T-518: a parallel scan of two projects leaves each project's own entry
    /// in the process-wide config cache — the other-projects loop keys each
    /// row by that row's own project, never by the caller's.
    #[test]
    fn a_scan_of_two_projects_keeps_each_projects_config_in_the_cache() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                for (id, lanes) in [("cache-a", 1), ("cache-b", 2)] {
                    let mut row = testing::seed_project(tx, id, "sess", T0)?;
                    row.config_toml = format!(
                        "\n[project]\nid = \"{id}\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n[quota]\nlanes = {lanes}\ntemps = 0\n\n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n"
                    );
                    tx.update_project(&row)?;
                }
                Ok(())
            })
            .unwrap();

        let _ = inputs(&mut store, "cache-a", T0).unwrap();

        let a = gear_app::config::cached("cache-a").expect("cache-a holds its own entry");
        let b = gear_app::config::cached("cache-b").expect("cache-b holds its own entry");
        assert_eq!(a.quota.lanes, 1, "cache-a kept its own config");
        assert_eq!(b.quota.lanes, 2, "cache-b kept its own config");
    }

    fn sh(path: &Path, args: &[&str]) -> String {
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

    fn commit(path: &Path, message: &str, file: &str, text: &str) -> String {
        let full = path.join(file);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(full, text).unwrap();
        sh(path, &["add", "--", file]);
        sh(path, &["commit", "-qm", message]);
        sh(path, &["rev-parse", "HEAD"])
    }

    /// A real repository: `dev` and two task branches, one of which changes a
    /// file `dev` also changes, the other a file of its own.
    fn repo() -> PathBuf {
        let path = crate::testtmp::temp("gear-parallel-cache").join("repo");
        std::fs::create_dir_all(&path).unwrap();
        sh(&path, &["init", "-q", "-b", "dev"]);
        sh(&path, &["config", "user.name", "test"]);
        sh(&path, &["config", "user.email", "a@b"]);
        sh(&path, &["config", "commit.gpgsign", "false"]);
        sh(&path, &["config", "core.hooksPath", "/dev/null"]);
        commit(&path, "base", SRC, "base\n");
        commit(&path, "other", "other.rs", "base\n");
        sh(&path, &["switch", "-qc", "heat/t1"]);
        commit(&path, "heat", SRC, "heat\n");
        sh(&path, &["switch", "-qc", "steady/t2", "dev"]);
        commit(&path, "steady", "other.rs", "steady\n");
        sh(&path, &["switch", "-q", "dev"]);
        path
    }

    /// A project of one repo with two lanes and one task each: the scan reads
    /// both branches, each both ways.
    fn store(repo: &Path) -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                let mut project = testing::seed_project(tx, "p", "sess", T0)?;
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
                    serde_json::json!(repo.to_string_lossy()).to_string()
                );
                tx.update_project(&project)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    SeatRole::Captain,
                    "w1:p1",
                    "codex",
                    T0,
                )?;
                for (n, lane) in [(1, "heat"), (2, "steady")] {
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
                Ok(())
            })
            .unwrap();
        store
    }

    /// T-435: one pair of commits is diffed once, however often a scan asks
    /// for it; a new commit is a pair the cache has not seen and is read once.
    #[test]
    fn a_pair_of_commits_is_diffed_once_and_a_new_commit_is_diffed_again() {
        let path = repo();
        let mut store = store(&path);
        let integration = sh(&path, &["rev-parse", "dev"]);
        let first = sh(&path, &["rev-parse", "heat/t1"]);
        let before = DIFF_MISSES.load(Ordering::Relaxed);
        // Each of the two task branches is read both ways: against the
        // integration branch and back for the overlap line.
        assert!(check(&mut store, "p", T0).unwrap().problems.is_empty());
        let read = DIFF_MISSES.load(Ordering::Relaxed) - before;
        assert_eq!(read, 4, "two task branches, each read both ways");
        // The same commits at the next period: the cache answers, git runs
        // no diff at all.
        assert!(check(&mut store, "p", T0 + PERIOD)
            .unwrap()
            .problems
            .is_empty());
        assert_eq!(
            DIFF_MISSES.load(Ordering::Relaxed) - before,
            read,
            "a repeated scan runs no git diff"
        );
        // A new commit on one task branch: its pair is read again, the other
        // branch's pairs are still the cache's.
        sh(&path, &["switch", "-q", "heat/t1"]);
        let second = commit(&path, "heat again", SRC, "heat again\n");
        assert_ne!(first, second);
        assert!(check(&mut store, "p", T0 + 2 * PERIOD)
            .unwrap()
            .problems
            .is_empty());
        assert_eq!(
            DIFF_MISSES.load(Ordering::Relaxed) - before,
            read + 2,
            "only the changed branch's pair is read again"
        );
        assert!(check(&mut store, "p", T0 + 3 * PERIOD)
            .unwrap()
            .problems
            .is_empty());
        assert_eq!(
            DIFF_MISSES.load(Ordering::Relaxed) - before,
            read + 2,
            "the new pair is the cache's too"
        );
        let diffs = DIFFS.lock().unwrap();
        assert!(diffs.contains_key(&(path.clone(), integration.clone(), first.clone())));
        assert!(diffs.contains_key(&(path.clone(), integration.clone(), second.clone())));
        assert!(diffs.contains_key(&(path.clone(), second.clone(), integration.clone())));
    }

    /// A store of the two-task repo where each task handed in its branch tip:
    /// the scan resolves two candidates, one per open task.
    fn store_with_candidates(path: &Path, store: &mut Store) {
        let candidates = [
            ("T-1", sh(path, &["rev-parse", "heat/t1"])),
            ("T-2", sh(path, &["rev-parse", "steady/t2"])),
        ];
        store
            .write(|tx| {
                for (n, (task, candidate)) in candidates.into_iter().enumerate() {
                    tx.insert_attempt(&gear_store::AttemptRow {
                        project: "p".into(),
                        task: task.into(),
                        n: 1,
                        seat: format!("p:{}", if n == 0 { "heat" } else { "steady" }),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: gear_model::AttemptState::Delivered,
                        candidate: Some(candidate),
                        report: None,
                        safe_point: None,
                        started: T0,
                        updated: T0,
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                    })?;
                }
                Ok(())
            })
            .unwrap();
    }

    /// T-527: a pass of a repo with several open tasks reads every branch tip
    /// with one `for-each-ref` and resolves every candidate with at most one
    /// `cat-file --batch-check`, and finds what the per-ref reads found.
    #[test]
    fn a_scan_reads_the_refs_and_the_candidates_in_one_process_each() {
        let path = repo();
        // The branches diverge from the integration branch and share one file:
        // a scan of this repo has both an overlap and a pair to find.
        sh(&path, &["switch", "-q", "dev"]);
        commit(&path, "dev moves", SRC, "dev moves\n");
        sh(&path, &["switch", "-q", "steady/t2"]);
        commit(&path, "steady moves", SRC, "steady src\n");
        // The same tasks without candidates: what the scan found before.
        let mut plain = store(&path);
        let wanted = {
            let inputs = inputs(&mut plain, "p", T0).unwrap();
            scan(&inputs[0]).unwrap()
        };
        // The store without candidates resolved none, so the counts below are
        // the pass with candidates alone.
        let refs_before = REF_READS.load(Ordering::Relaxed);
        let cat_before = CAT_FILE_READS.load(Ordering::Relaxed);
        let mut with = store(&path);
        store_with_candidates(&path, &mut with);
        let found = {
            let inputs = inputs(&mut with, "p", T0).unwrap();
            scan(&inputs[0]).unwrap()
        };

        assert_eq!(
            REF_READS.load(Ordering::Relaxed) - refs_before,
            1,
            "one for-each-ref per pass"
        );
        assert_eq!(
            CAT_FILE_READS.load(Ordering::Relaxed) - cat_before,
            1,
            "one cat-file per pass"
        );
        // Same findings as today: two candidates that are still open changes
        // leave the overlap and the integration lines alone.
        assert_eq!(format!("{:?}", found.pairs), format!("{:?}", wanted.pairs));
        assert_eq!(
            format!("{:?}", found.integration),
            format!("{:?}", wanted.integration)
        );
        assert_eq!(
            found.integration.keys().collect::<Vec<_>>(),
            vec![
                &"integration:T-1".to_string(),
                &"integration:T-2".to_string()
            ],
            "both branches overlap the integration branch: {found:?}"
        );
        assert_eq!(
            found.pairs.keys().collect::<Vec<_>>(),
            vec![&"tasks:T-1:T-2".to_string()],
            "the two branches share a file: {found:?}"
        );

        // The next pass reads the refs and the candidates again; the ancestry
        // answers of the first pass are the cache's.
        let ancestors_before = ANCESTOR_MISSES.load(Ordering::Relaxed);
        let inputs = inputs(&mut with, "p", T0 + PERIOD).unwrap();
        scan(&inputs[0]).unwrap();
        assert_eq!(
            REF_READS.load(Ordering::Relaxed) - refs_before,
            2,
            "one for-each-ref per pass"
        );
        assert_eq!(
            CAT_FILE_READS.load(Ordering::Relaxed) - cat_before,
            2,
            "one cat-file per pass"
        );
        assert_eq!(
            ANCESTOR_MISSES.load(Ordering::Relaxed),
            ancestors_before,
            "a pair of commit ids is asked of git once per process"
        );
    }

    /// T-527: the ancestry of a pair of full commit ids never changes, so the
    /// second answer is the cache's and runs no git.
    #[test]
    fn a_second_ancestry_answer_for_a_pair_runs_no_git() {
        let path = repo();
        let dev = sh(&path, &["rev-parse", "dev"]);
        let heat = sh(&path, &["rev-parse", "heat/t1"]);
        let before = ANCESTOR_MISSES.load(Ordering::Relaxed);
        assert!(!ancestor(&path, &heat, &dev).unwrap(), "heat is not in dev");
        assert_eq!(ANCESTOR_MISSES.load(Ordering::Relaxed), before + 1);
        assert!(
            !ancestor(&path, &heat, &dev).unwrap(),
            "the same answer twice"
        );
        assert_eq!(
            ANCESTOR_MISSES.load(Ordering::Relaxed),
            before + 1,
            "the second answer runs no git"
        );
        // A different pair is a new answer.
        assert!(ancestor(&path, &dev, &dev).unwrap(), "a commit is its own");
        assert_eq!(ANCESTOR_MISSES.load(Ordering::Relaxed), before + 2);
        let answers = ANCESTORS.lock().unwrap();
        assert_eq!(
            answers.get(&(path.clone(), heat, dev.clone())),
            Some(&false)
        );
        assert_eq!(answers.get(&(path.clone(), dev.clone(), dev)), Some(&true));
    }
}
