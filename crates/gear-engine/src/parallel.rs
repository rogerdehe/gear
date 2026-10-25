//! T-51: read-only Git overlap checks on hand-in and every ten minutes.
//! Git runs outside store transactions against pinned commit ids. Published
//! integration and local task refs are read without fetch or worktree changes.
//!
//! T-53: the scan runs per repo. A task's repo is its lane's repo, so only
//! tasks of the same repo are compared: two lanes in two repos share no file,
//! and each repo is scanned against its own integration branch.

use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}, process::Command};
use gear_app::config::Repo;
use gear_model::{GearResult, MsgKind, MsgState, TaskState, Ts};
use gear_store::{NewMessage, Store, TaskRow, Tx};

const PERIOD: i64 = 600;

/// The key of the project root in the meta: a repo name never starts with a
/// dash.
const ROOT: &str = "-";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub opened: Vec<String>,
    /// Git failures are evidence gaps, not an empty file set or a fatal tick.
    pub problems: Vec<String>,
}

/// One scan: the repo, the tasks of that repo and the state it remembers.
struct Input {
    /// The repo's key in the meta: its name, "-" for the project root.
    key: String,
    /// The project the scan belongs to.
    project: String,
    repo: PathBuf,
    integration: String,
    tasks: Vec<(String, String)>,
    handins: String,
}

struct Branch {
    task: String,
    lane: String,
    files: BTreeSet<String>,
}

struct Conflict {
    left: String,
    right: String,
    files: Vec<String>,
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
            };
            let Some(captain) = tx.captain_of(project)? else { return Ok(Check::default()); };
            let previous = tx.get_meta(&format!("{prefix}:pairs"))?
                .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default();
            let mut opened = Vec::new();
            // The repo the overlap is in: the captain must know which one.
            let where_ = match input.name() {
                Some(name) => format!(" in {name}"),
                None => String::new(),
            };
            for (pair, conflict) in conflicts {
                let key = format!("{prefix}:pair:{pair}");
                let files = serde_json::json!(conflict.files).to_string();
                if tx.get_meta(&format!("{key}:files"))?.as_deref() == Some(&files) { continue; }
                close(tx, project, &key, "the shared file set changed", now)?;
                let message = tx.insert_message(&NewMessage {
                    project: project.into(), thread: format!("parallel:{pair}"), task: None, attempt: None,
                    sender: "engine".into(), sender_gen: 0, recipient: captain.id.clone(),
                    kind: MsgKind::System, urgent: false,
                    body: format!("parallel changes{where_}: {} and {} share files: {files}", conflict.left, conflict.right),
                    reply_to: None, needs_ack: false, state: MsgState::Open, created: now,
                })?;
                tx.set_meta(&format!("{key}:files"), &files)?;
                tx.set_meta(&format!("{key}:item"), &message.id)?;
                opened.push(message.id);
            }
            for pair in previous.iter().filter(|pair| !conflicts.contains_key(*pair)) {
                let key = format!("{prefix}:pair:{pair}");
                close(tx, project, &key, "the branches no longer share changed files", now)?;
                tx.set_meta(&format!("{key}:files"), "[]")?;
            }
            tx.set_meta(&format!("{prefix}:pairs"), &serde_json::json!(conflicts.keys().collect::<Vec<_>>()).to_string())?;
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
        if self.key == ROOT { None } else { Some(&self.key) }
    }
}

/// The prefix of the meta of a repo's scan.
fn prefix_of(project: &str, key: &str) -> String {
    // Keep the pre-multi-repo root namespace and its existing alert IDs.
    if key == ROOT { format!("parallel:{project}") } else { format!("parallel:{project}:{key}") }
}

/// The scans of one project: one per repo that has tasks (T-53). A task
/// belongs to the repo of its lane, and only tasks of the same repo are
/// compared with each other.
fn inputs(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<Input>> {
    store.read(|tx| {
        let Some(row) = tx.get_project(project)? else { return Ok(Vec::new()); };
        let Ok(config) = gear_app::config::parse(&row.config_toml) else { return Ok(Vec::new()); };
        let tasks = tx.tasks_of_project(project)?;
        // The tasks of the project, grouped by the repo of their lane.
        let mut groups: BTreeMap<String, (Repo, Vec<TaskRow>)> = BTreeMap::new();
        for task in &tasks {
            let Some(lane) = task.lane.as_deref() else { continue };
            let repo = config.repo_of_lane(lane);
            groups
                .entry(repo_key(&repo))
                .or_insert_with(|| (repo.clone(), Vec::new()))
                .1
                .push(task.clone());
        }
        let mut inputs = Vec::new();
        for (key, (repo, tasks)) in groups {
            // The hand-ins of this repo: a new one starts the scan early.
            let mut handins = Vec::new();
            for task in &tasks {
                for attempt in tx.attempts_of_task(project, &task.id)? {
                    if let Some(candidate) = attempt.candidate {
                        handins.push((task.id.clone(), attempt.n, candidate));
                    }
                }
            }
            handins.sort();
            let handins = serde_json::json!([row.config_version, handins]).to_string();
            // The back-off of this repo: nothing changed and the last scan
            // was recent, so this repo waits for the next period.
            let prefix = prefix_of(project, &key);
            let last = tx.get_meta(&format!("{prefix}:at"))?.and_then(|s| s.parse::<Ts>().ok());
            let unchanged = tx.get_meta(&format!("{prefix}:handins"))?.as_deref() == Some(&handins);
            if unchanged && last.is_some_and(|last| now >= last && now - last < PERIOD) {
                continue;
            }
            let tasks = tasks.into_iter()
                .filter(|t| !matches!(t.state, TaskState::Draft | TaskState::Cancelled | TaskState::Failed))
                .filter_map(|t| t.lane.map(|lane| (t.id, lane))).collect();
            inputs.push(Input {
                key,
                project: project.to_string(),
                repo: repo.path.into(),
                integration: repo.integration,
                tasks,
                handins,
            });
        }
        Ok(inputs)
    })
}

/// The key of a repo in the meta: its name, or "-" for the project root (a
/// repo name never starts with a dash).
fn repo_key(repo: &Repo) -> String {
    if repo.name.is_empty() { ROOT.to_string() } else { repo.name.clone() }
}

fn close(tx: &Tx<'_>, project: &str, key: &str, reason: &str, now: Ts) -> GearResult<()> {
    if let Some(id) = tx.get_meta(&format!("{key}:item"))? {
        if let Some(message) = tx.get_message(project, &id)? {
            if matches!(message.state, MsgState::Open | MsgState::Waiting) {
                tx.update_message_state(project, &id, MsgState::Resolved, Some(reason), None, None, now)?;
            }
        }
    }
    Ok(())
}

fn scan(input: &Input) -> Result<BTreeMap<String, Conflict>, String> {
    git(&input.repo, &["rev-parse", "--git-dir"])?;
    let local = revision(&input.repo, &format!("refs/heads/{}", input.integration))?;
    let remote = revision(&input.repo, &format!("refs/remotes/origin/{}", input.integration))?;
    // Use the containing tip when one is ahead; origin is authoritative when
    // the local integration ref diverged. No network operation is needed.
    let integration = match (local, remote) {
        (Some(local), Some(remote)) if ancestor(&input.repo, &remote, &local)? => local,
        (_, Some(remote)) => remote,
        (Some(local), None) => local,
        _ => return Err(format!("integration branch {} is unavailable", input.integration)),
    };
    let mut branches = Vec::new();
    let mut conflicts = BTreeMap::new();
    for (task, lane) in &input.tasks {
        let name = crate::dispatch::task_branch(lane, task);
        let local = revision(&input.repo, &format!("refs/heads/{name}"))?;
        let head = match local {
            Some(head) => Some(head),
            None => revision(&input.repo, &format!("refs/remotes/origin/{name}"))?,
        };
        let Some(head) = head else { continue; };
        let files = changed(&input.repo, &integration, &head)?;
        if files.is_empty() { continue; } // includes already integrated Done branches
        let integration_files = changed(&input.repo, &head, &integration)?;
        let shared: Vec<_> = files.intersection(&integration_files).cloned().collect();
        if !shared.is_empty() {
            conflicts.insert(format!("integration:{task}"), Conflict {
                left: format!("{task} ({lane})"), right: format!("integration {}", input.integration), files: shared,
            });
        }
        branches.push(Branch { task: task.clone(), lane: lane.clone(), files });
    }
    branches.sort_by(|a, b| a.task.cmp(&b.task));
    for (index, left) in branches.iter().enumerate() {
        for right in branches.iter().skip(index + 1).filter(|b| b.lane != left.lane) {
            let files: Vec<_> = left.files.intersection(&right.files).cloned().collect();
            if !files.is_empty() {
                conflicts.insert(format!("tasks:{}:{}", left.task, right.task), Conflict {
                    left: format!("{} ({})", left.task, left.lane),
                    right: format!("{} ({})", right.task, right.lane), files,
                });
            }
        }
    }
    Ok(conflicts)
}

fn revision(repo: &Path, reference: &str) -> Result<Option<String>, String> {
    let output = Command::new("git").arg("-C").arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{reference}^{{commit}}")])
        .output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_string()))
    } else if output.status.code() == Some(1) && output.stderr.is_empty() {
        Ok(None)
    } else {
        Err(format!("cannot read {reference}: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

fn ancestor(repo: &Path, from: &str, to: &str) -> Result<bool, String> {
    let output = Command::new("git").arg("-C").arg(repo)
        .args(["merge-base", "--is-ancestor", from, to]).output().map_err(|e| e.to_string())?;
    match output.status.code() {
        Some(0) => Ok(true), Some(1) => Ok(false),
        _ => Err(String::from_utf8_lossy(&output.stderr).into_owned()),
    }
}

fn changed(repo: &Path, from: &str, to: &str) -> Result<BTreeSet<String>, String> {
    let output = git(repo, &["diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--name-only", "-z",
        &format!("{from}...{to}"), "--"])?;
    Ok(output.split(|b| *b == 0).filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git").arg("-C").arg(repo).args(args)
        .output().map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if output.status.success() { Ok(output.stdout) } else {
        Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))
    }
}
