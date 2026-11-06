//! T-571: a finished voyage sends nothing.
//!
//! A voyage is finished when none of its tasks is open (the user, 2026-11-05:
//! Lawrence's voyage M1 finished on 2026-11-04 — every task done or cancelled,
//! every lane retired — yet its new captain was rung again and again for two
//! branch warnings from 2026-11-03, each doorbell carrying M1's goal,
//! interrupting the work the user had given it).
//!
//! The items a voyage left open are resolved here, once, on the tick that finds
//! it finished: the branch sweep, the station of the voyage, the merge
//! failures and waits, the overrun asks, the stalled-voyage item, and the open
//! items of its tasks. The doorbell half is [`crate::doorbell::goal_line`] (no
//! goal of a finished voyage) and the captain filter in `tick` (no engine item
//! of a project with no open voyage).

use crate::merge;
use gear_app::config::{ProjectConfig, Repo};
use gear_herdr::{HerdrAct, Outcome};
use gear_model::{GearResult, MsgKind, MsgState, Ts};
use gear_store::{Store, Tx};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Why an item closes with its voyage: `voyage M1 is finished`.
pub fn reason(voyage: &str) -> String {
    format!("voyage {voyage} is finished")
}

/// The project-level threads a finished voyage leaves behind (T-571): the
/// branch sweep (named after the tick), the merge failures and waits (named
/// after the task), the overrun asks and the stalled-voyage item. They belong
/// to no single task, so they close only when no voyage of the project is open
/// any more.
const PROJECT_THREADS: [&str; 6] = [
    "branches:",
    "merge-failed:",
    "merge-lost:",
    "merge-wait:",
    "overrun",
    "stalled",
];

/// A finished voyage and the tasks of it.
struct FinishedRow {
    milestone: String,
    tasks: BTreeSet<String>,
    /// The voyage's landing order: it is answered by `station <id>: land`,
    /// never closed by the sweep (Prometheus M4: M-11240 opened and closed on
    /// the same tick, and the captain had no item to land with).
    land: Option<String>,
}

/// The voyages of this project that are finished: departed, with tasks, and
/// open by neither of [`open_voyages`]'s rules.
fn finished_voyages(
    tx: &Tx<'_>,
    project: &str,
    open: &BTreeSet<String>,
) -> GearResult<Vec<FinishedRow>> {
    let mut finished = Vec::new();
    for voyage in tx.voyages_of_project(project)? {
        if voyage.departure.is_none() || open.contains(&voyage.milestone) {
            continue;
        }
        let tasks: BTreeSet<String> = tx
            .tasks_of_voyage(project, &voyage.milestone)?
            .into_iter()
            .map(|task| task.id)
            .collect();
        if tasks.is_empty() {
            continue;
        }
        let land = tx.get_meta(&format!("station:{project}:{}:land", voyage.milestone))?;
        finished.push(FinishedRow {
            milestone: voyage.milestone,
            tasks,
            land,
        });
    }
    Ok(finished)
}

/// The voyages of the project that are open: one with a task that is not
/// terminal (T-529), and one whose Done tasks hold accepted work that is not on
/// the integration branch (T-595). The second rule is the landing's own
/// (`gear_app::station::unmerged_tasks`): GEAR M6.2 had every task Done, T-584's
/// merge failed its gate, and the sweep closed the merge-failed and merge-lost
/// items as "voyage M6.2 is finished" — nothing would ever merge it.
fn open_voyages(tx: &Tx<'_>, project: &str) -> GearResult<BTreeSet<String>> {
    let mut open: BTreeSet<String> = tx
        .tasks_live_of_project(project)?
        .into_iter()
        .filter_map(|task| task.milestone)
        .collect();
    for voyage in tx.voyages_of_project(project)? {
        if open.contains(&voyage.milestone) {
            continue;
        }
        if !gear_app::station::unmerged_tasks(tx, project, &voyage.milestone)?.is_empty() {
            open.insert(voyage.milestone);
        }
    }
    Ok(open)
}

/// The finished voyages of a project, read once (T-571): the sweep that closes
/// their items and the doorbell that names none of them use the same reading.
pub struct Sweep {
    rows: Vec<FinishedRow>,
    /// A voyage of the project still has open work: its project-level threads
    /// (a branch sweep, a merge wait) may be about that one, so they stay.
    any_open: bool,
}

impl Sweep {
    pub fn read(tx: &Tx<'_>, project: &str) -> GearResult<Sweep> {
        let open = open_voyages(tx, project)?;
        let rows = finished_voyages(tx, project, &open)?;
        Ok(Sweep {
            rows,
            any_open: !open.is_empty(),
        })
    }

    pub fn empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Why this item belongs to a finished voyage, if it does.
    pub fn why(&self, message: &gear_store::MessageRow) -> Option<String> {
        why(message, &self.rows, self.any_open)
    }
}

/// Resolve the open engine items a finished voyage left behind, and return
/// their ids. Runs every tick; a finished voyage whose items are already
/// resolved writes nothing.
pub fn close(store: &mut Store, project: &str, now: Ts) -> GearResult<Vec<String>> {
    let sweep = store.read(|tx| Sweep::read(tx, project))?;
    if sweep.empty() {
        return Ok(Vec::new());
    }
    store.write(|tx| {
        let mut closed = Vec::new();
        for message in tx.open_messages_of_project(project)? {
            let Some(reason) = sweep.why(&message) else {
                continue;
            };
            tx.update_message_state(
                project,
                &message.id,
                MsgState::Resolved,
                Some(&reason),
                None,
                None,
                now,
            )?;
            closed.push(message.id);
        }
        Ok(closed)
    })
}

/// Why this item belongs to a finished voyage, if it does.
fn why(
    message: &gear_store::MessageRow,
    finished: &[FinishedRow],
    any_open: bool,
) -> Option<String> {
    // The item of a task of a finished voyage: its attempt's threads, its merge
    // failures and waits, its overrun ask.
    if let Some(task) = &message.task {
        if let Some(row) = finished.iter().find(|row| row.tasks.contains(task)) {
            return Some(reason(&row.milestone));
        }
    }
    // The station of the voyage itself, except its landing order.
    if let Some(voyage) = message.thread.strip_prefix("station:") {
        if finished
            .iter()
            .any(|row| row.milestone == voyage && row.land.as_deref() != Some(message.id.as_str()))
        {
            return Some(reason(voyage));
        }
    }
    if any_open {
        return None;
    }
    // A project-level thread, with no voyage of the project open any more: the
    // newest finished voyage is the one it belonged to.
    if PROJECT_THREADS
        .iter()
        .any(|prefix| message.thread.starts_with(prefix))
    {
        return finished.last().map(|row| reason(&row.milestone));
    }
    None
}

/// The thread of the landing's records item (T-585): `records:<milestone>`.
fn records_thread(milestone: &str) -> String {
    format!("records:{milestone}")
}

/// The meta key that says a landing's records step is done (T-585): one
/// landing merges the captain's branch once.
fn records_key(project: &str, milestone: &str) -> String {
    format!("records_merged:{project}:{milestone}")
}

/// The thread of the reclaim item: worktrees a landed voyage left behind.
pub const RECLAIM_THREAD: &str = "reclaim";
const RECLAIM_EVERY_S: i64 = 300;
/// How long after its landing a voyage's leftovers are named.
const RECLAIM_WITHIN_S: i64 = 24 * 3600;

/// A landed voyage keeps no worktree (the user, 2026-11-05: Prometheus M4
/// landed with fourteen worktrees and six Herdr workspaces left). Once the
/// project has no open voyage and its newest finished voyage was landed by
/// `station <id>: land`: the captain's records branch is merged into the
/// integration branch (T-585), then every worktree of the project's
/// repositories (its path named `<project>-…` or its branch `<project>/…`) is
/// named to the captain in one item, until none is left. Read every
/// `RECLAIM_EVERY_S`.
pub fn reclaim(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    now: Ts,
) -> GearResult<Option<&'static str>> {
    let landed = landed_voyage(store, project, now)?;
    let open = store.read(|tx| {
        Ok(tx
            .thread_messages(project, RECLAIM_THREAD)?
            .into_iter()
            .find(|message| message.state == MsgState::Open))
    })?;
    let Some(milestone) = landed else {
        return Ok(None);
    };
    let key = format!("engine.reclaim_scan.{project}");
    let last = store
        .read(|tx| tx.get_meta(&key))?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(i64::MIN / 2);
    if now - last < RECLAIM_EVERY_S {
        return Ok(None);
    }
    store.write(|tx| tx.set_meta(&key, &now.to_string()))?;
    let Some(row) = store.read(|tx| tx.get_project(project))? else {
        return Ok(None);
    };
    let Ok(config) = gear_app::config::parse_row(&row) else {
        return Ok(None);
    };
    // T-585: the landing merges the captain's records before it names what is
    // left, so a captain only commits and pushes its branch before landing.
    let records = merge_records(store, herdr, project, &config, &milestone, now)?;
    let mut left = Vec::new();
    let mut seen = BTreeSet::new();
    for repo in config.repos() {
        if !seen.insert(repo.path.clone()) {
            continue;
        }
        // T-618: remove each worktree of the project that is clean and whose
        // branch has nothing the remote does not have; name the others with
        // the reason.
        for worktree in project_worktrees(&repo.path, project) {
            match remove_worktree(&repo.path, &worktree) {
                Ok(()) => {}
                Err(reason) => left.push(format!("worktree {} ({reason})", worktree.path)),
            }
        }
        // The release and staging branches outlive every voyage; the
        // integration branch is the voyage's and goes with it (a departure
        // cuts it again from the release branch).
        let keep = [repo.release.as_str(), config.conventions.staging.as_str()];
        for branch in project_branches(&repo.path, &config.conventions.remote, project) {
            if keep.contains(&branch.name.as_str()) {
                continue;
            }
            if branch_on_integration(&repo.path, &config, &branch.tip) {
                if let Err(reason) =
                    delete_remote_branch(&repo.path, &config.conventions.remote, &branch.name)
                {
                    left.push(format!("branch {} ({reason})", branch.name));
                }
            } else {
                left.push(format!("branch {} (extra commits)", branch.name));
            }
        }
    }
    // T-618: close each Herdr workspace of the project whose panes hold no
    // agent; name the others.
    for workspace in project_workspaces(project) {
        if workspace.holds_agent {
            left.push(format!(
                "workspace {} ({}) (holding an agent)",
                workspace.id, workspace.label
            ));
            continue;
        }
        match herdr.worktree_remove(&workspace.id) {
            Outcome::Ok(()) => {}
            Outcome::Refused { .. } | Outcome::Uncertain { .. } => left.push(format!(
                "workspace {} ({}) (could not close)",
                workspace.id, workspace.label
            )),
        }
    }
    let changed = store.write(|tx| {
        if left.is_empty() {
            if let Some(item) = &open {
                tx.update_message_state(
                    project,
                    &item.id,
                    MsgState::Resolved,
                    Some("every worktree of the landed voyage is removed"),
                    None,
                    None,
                    now,
                )?;
                return Ok(Some("closed"));
            }
            return Ok(None);
        }
        let body = format!(
            "The voyage landed. GEAR removed what it could; {} worktree(s), branch(es) or Herdr workspace(s) of {project} are left: {}. A landed voyage keeps none: merge what is not on the integration branch, then remove each worktree with `git worktree remove`, delete each branch on the remote, and close each workspace with `herdr workspace close <id>`.",
            left.len(),
            left.join(", ")
        );
        match &open {
            Some(item) if item.body == body => Ok(None),
            Some(item) => {
                tx.update_message_body(project, &item.id, &body)?;
                Ok(Some("updated"))
            }
            None => {
                let Some(captain) = tx.captain_of(project)? else {
                    return Ok(None);
                };
                tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: project.to_string(),
                    thread: RECLAIM_THREAD.to_string(),
                    task: None,
                    attempt: None,
                    sender: "engine".into(),
                    sender_gen: 0,
                    recipient: captain.id,
                    kind: gear_model::MsgKind::System,
                    urgent: true,
                    body,
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Open,
                    created: now,
                })?;
                Ok(Some("opened"))
            }
        }
    })?;
    Ok(changed.or(records))
}

/// The voyage this project last landed, if it was landed within the check's
/// window: the newest finished voyage whose landing order
/// `station <id>: land` was answered.
fn landed_voyage(store: &mut Store, project: &str, now: Ts) -> GearResult<Option<String>> {
    store.read(|tx| {
        let sweep = Sweep::read(tx, project)?;
        if sweep.any_open {
            return Ok(None);
        }
        let Some((land, milestone)) = sweep
            .rows
            .iter()
            .rev()
            .find_map(|row| row.land.clone().map(|land| (land, row.milestone.clone())))
        else {
            return Ok(None);
        };
        // Only a landing of the last day is checked: an older voyage was
        // landed before this check existed, and its captain is not rung about
        // it (Lawrence M1, landed days before: the user wants it left alone).
        let Some(message) = tx.get_message(project, &land)? else {
            return Ok(None);
        };
        let landed = message.state == MsgState::Resolved
            && message
                .resolved
                .is_some_and(|at| now - at < RECLAIM_WITHIN_S)
            && message.result.as_deref().is_some_and(|result| {
                // Only a landing: `station M-n: land …`, never a
                // `keep course` (M6.2 was taken for landed).
                result.split_once(':').is_some_and(|(_, action)| {
                    let action = action.trim();
                    action == "land" || action.starts_with("land ")
                })
            });
        Ok(landed.then_some(milestone))
    })
}

/// What became of a landing's captain branch (T-585).
#[derive(Debug)]
enum Recorded {
    /// The branch's commits are on the integration branch under
    /// `records: <project> <milestone>`, and the branch is gone.
    Merged,
    /// The branch held nothing the integration branch does not have: it is
    /// gone, nothing was merged.
    Empty,
    /// No branch of that name is on the remote: nothing to merge, nothing to
    /// delete.
    Missing,
    /// Another merge holds the integration worktree: the next scan tries
    /// again, and nothing was written or deleted.
    Busy,
    /// The merge or its push failed: the branch is left as it was.
    Failed(String),
}

/// A landing merges the captain's records (T-585). The captain's seat branch
/// is fetched; when it holds commits the integration branch does not have, it
/// is merged into it in the merge worktree — a merge commit
/// `records: <project> <milestone>`, no gate, no conflict resolved — and the
/// branch is deleted on the remote and locally. A branch with nothing new is
/// only deleted. Done once per landing (`records_merged:<project>:<milestone>`);
/// a conflict or a push failure opens one captain item on thread
/// `records:<milestone>` and deletes nothing.
fn merge_records(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    config: &ProjectConfig,
    milestone: &str,
    now: Ts,
) -> GearResult<Option<&'static str>> {
    let key = records_key(project, milestone);
    let repo = config.root_repo();
    let branch = gear_model::branch::expand_seat(
        &config.conventions.seat_branch,
        project,
        "captain",
        milestone,
    );
    // A branch git would read as an option is never asked of it: a project
    // that names its seat branches that way has no records step.
    if branch == repo.integration || branch.starts_with('-') {
        return Ok(None);
    }
    let dir = PathBuf::from(&repo.path);
    // The fetch names every branch of the remote, as a merge's fetch does.
    // A remote that cannot be read is not a failure of the records: nothing
    // is written or deleted, and the next scan tries again.
    if merge::git(
        &dir,
        &[
            "fetch",
            "--prune",
            &config.conventions.remote,
            &format!("+refs/heads/*:refs/remotes/{}/*", config.conventions.remote),
        ],
    )
    .is_err()
    {
        return Ok(None);
    }
    let theirs = format!("{}/{}", config.conventions.remote, branch);
    // T-619: the branch may appear again after the landing (a records apply
    // rewrote the plan). The meta remembers the tip it merged, so a new tip
    // is merged again and the same tip is left alone.
    let tip = merge::git(
        &dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{theirs}"),
        ],
    )
    .ok()
    .map(|tip| tip.trim().to_string());
    if let Some(tip) = &tip {
        if store.read(|tx| tx.get_meta(&key))?.as_deref() == Some(tip.as_str()) {
            return Ok(None);
        }
    }
    let step = records_step(
        store, herdr, project, config, &repo, milestone, &branch, now,
    )?;
    match step {
        Recorded::Merged => {
            let tip = tip.expect("a merged branch has a tip");
            store.write(|tx| tx.set_meta(&key, &tip))?;
            let reason = format!("the captain's records are merged into {}", repo.integration);
            let _ = resolve_records(store, project, milestone, &reason, now)?;
            Ok(Some("merged"))
        }
        Recorded::Empty => {
            let tip = tip.expect("a branch with nothing new has a tip");
            store.write(|tx| tx.set_meta(&key, &tip))?;
            let reason = format!(
                "the captain's branch {branch} held nothing the integration branch does not have"
            );
            resolve_records(store, project, milestone, &reason, now)
        }
        Recorded::Missing => {
            let reason = format!("there is no branch {branch} on the remote");
            resolve_records(store, project, milestone, &reason, now)
        }
        Recorded::Busy => Ok(None),
        Recorded::Failed(error) => tell_records_failed(
            store,
            project,
            milestone,
            &branch,
            &repo.integration,
            &error,
            now,
        ),
    }
}

/// The git half of [`merge_records`]: fetch (done by the caller), compare,
/// merge, push and delete. Nothing is deleted unless the branch is on the
/// integration branch (or already held nothing new).
#[allow(clippy::too_many_arguments)]
fn records_step(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    project: &str,
    config: &ProjectConfig,
    repo: &Repo,
    milestone: &str,
    branch: &str,
    now: Ts,
) -> GearResult<Recorded> {
    let dir = PathBuf::from(&repo.path);
    let remote = config.conventions.remote.clone();
    let theirs = format!("{remote}/{branch}");
    if merge::git(
        &dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{theirs}"),
        ],
    )
    .is_err()
    {
        return Ok(Recorded::Missing);
    }
    let ahead = match merge::git(
        &dir,
        &[
            "rev-list",
            "--count",
            &format!("{remote}/{}..{theirs}", repo.integration),
        ],
    ) {
        Ok(count) => count.trim().parse::<u64>().unwrap_or(0),
        Err(error) => return Ok(Recorded::Failed(error)),
    };
    if ahead == 0 {
        // Nothing new: only the branch goes.
        delete_records_branch(&dir, &remote, branch);
        return Ok(Recorded::Empty);
    }
    let worktree = match merge::open_integration_worktree(store, herdr, repo, project, config, now)
    {
        Ok(worktree) => worktree,
        Err(reason) => return Ok(Recorded::Failed(reason)),
    };
    // T-619: the reclaim may have removed the merge worktree (T-618). Herdr
    // still names it, so drop it and open it again the same way.
    let worktree = if Path::new(&worktree.path).exists() {
        worktree
    } else {
        let _ = herdr.worktree_remove(&worktree.workspace_id);
        let label = merge::merge_label(project, repo);
        match herdr.worktree_create(&repo.path, &worktree.branch, &repo.integration, &label) {
            Outcome::Ok(worktree) => worktree,
            other => {
                return Ok(Recorded::Failed(merge::step_failure(
                    "worktree_create",
                    &other,
                )))
            }
        }
    };
    let work = Path::new(&worktree.path);
    // The merge queue shares this worktree: its lock keeps the two apart. A
    // worktree another merge holds is not a failure, just a later attempt.
    let lock = match merge::merge_lock(work) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(Recorded::Busy),
        Err(error) => return Ok(Recorded::Failed(error)),
    };
    let git = |args: &[&str]| merge::git_with_lock(work, args, Some(&lock));
    let base = format!("{remote}/{}", repo.integration);
    if let Err(error) = git(&["reset", "--hard", &base]) {
        return Ok(Recorded::Failed(error));
    }
    let message = format!("records: {project} {milestone}");
    // No gate: the records are the captain's files, and a conflict is not
    // resolved here — the branch goes back to the captain.
    if let Err(error) = git(&["merge", "--no-ff", "-m", &message, &theirs]) {
        let _ = git(&["merge", "--abort"]);
        let _ = git(&["reset", "--hard", &base]);
        return Ok(Recorded::Failed(error));
    }
    if let Err(error) = git(&[
        "push",
        &remote,
        &format!("HEAD:refs/heads/{}", repo.integration),
    ]) {
        // Nothing landed: the worktree goes back to the remote tip, and the
        // branch is kept.
        let _ = git(&["reset", "--hard", &base]);
        return Ok(Recorded::Failed(error));
    }
    delete_records_branch(&dir, &remote, branch);
    Ok(Recorded::Merged)
}

/// Delete a branch on the remote and locally: a landed voyage keeps no captain
/// branch. A ref git refuses (the captain's own worktree still holds it) stays
/// and the reclaim item names it.
fn delete_records_branch(dir: &Path, remote: &str, branch: &str) {
    let _ = merge::git(dir, &["push", remote, "--delete", branch]);
    let _ = merge::git(dir, &["branch", "-D", branch]);
    let _ = merge::git(
        dir,
        &[
            "update-ref",
            "-d",
            &format!("refs/remotes/{remote}/{branch}"),
        ],
    );
}

/// One captain item on thread `records:<milestone>` (T-585): the landing could
/// not merge the captain's branch. The same failure again changes nothing.
fn tell_records_failed(
    store: &mut Store,
    project: &str,
    milestone: &str,
    branch: &str,
    integration: &str,
    error: &str,
    now: Ts,
) -> GearResult<Option<&'static str>> {
    let thread = records_thread(milestone);
    let open = store.read(|tx| {
        Ok(tx
            .thread_messages(project, &thread)?
            .into_iter()
            .find(|message| message.state == MsgState::Open))
    })?;
    let body = format!(
        "The voyage {milestone} landed, but the captain's records branch {branch} could not be merged into {integration}: {error}. Nothing was deleted: merge {integration} into {branch}, resolve it there and push it, and the next scan merges it."
    );
    store.write(|tx| match &open {
        Some(item) if item.body == body => Ok(None),
        Some(item) => {
            tx.update_message_body(project, &item.id, &body)?;
            Ok(Some("updated"))
        }
        None => {
            let Some(captain) = tx.captain_of(project)? else {
                return Ok(None);
            };
            tx.insert_message(&gear_store::NewMessage {
                meta: None,
                project: project.to_string(),
                thread,
                task: None,
                attempt: None,
                sender: "engine".into(),
                sender_gen: 0,
                recipient: captain.id,
                kind: MsgKind::System,
                urgent: true,
                body,
                reply_to: None,
                needs_ack: false,
                state: MsgState::Open,
                created: now,
            })?;
            Ok(Some("opened"))
        }
    })
}

/// Close the `records:<milestone>` item a landing left open, once its branch is
/// on the integration branch or gone.
fn resolve_records(
    store: &mut Store,
    project: &str,
    milestone: &str,
    reason: &str,
    now: Ts,
) -> GearResult<Option<&'static str>> {
    let thread = records_thread(milestone);
    let open = store.read(|tx| {
        Ok(tx
            .thread_messages(project, &thread)?
            .into_iter()
            .find(|message| message.state == MsgState::Open))
    })?;
    let Some(item) = open else {
        return Ok(None);
    };
    store.write(|tx| {
        tx.update_message_state(
            project,
            &item.id,
            MsgState::Resolved,
            Some(reason),
            None,
            None,
            now,
        )
    })?;
    Ok(Some("closed"))
}

/// A worktree of the project: its path and its branch (None when detached).
struct ProjectWorktree {
    path: String,
    branch: Option<String>,
}

/// A remote branch of the project: its name and its tip.
struct ProjectBranch {
    name: String,
    tip: String,
}

/// A Herdr workspace of the project: its id, its label and whether its panes
/// hold an agent.
struct ProjectWorkspace {
    id: String,
    label: String,
    holds_agent: bool,
}

/// The worktrees of `repo` that belong to `project`: a path whose last part
/// starts with `<project>-`, or a branch under `<project>/`. The main
/// checkout is never one.
fn project_worktrees(repo: &str, project: &str) -> Vec<ProjectWorktree> {
    let Ok(out) = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = Vec::new();
    for (index, block) in text.split("\n\n").enumerate() {
        if index == 0 {
            continue; // the main checkout
        }
        let mut path = None;
        let mut branch = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
                branch = Some(rest.to_string());
            }
        }
        let Some(path) = path else { continue };
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let ours = name.starts_with(&format!("{project}-"))
            || branch
                .as_deref()
                .is_some_and(|branch| branch.starts_with(&format!("{project}/")));
        if ours {
            found.push(ProjectWorktree { path, branch });
        }
    }
    found
}

/// T-618: remove a worktree of the project when it is clean (`git status
/// --porcelain` empty) and its branch has nothing the remote does not have.
/// Returns the reason it was kept.
fn remove_worktree(repo: &str, worktree: &ProjectWorktree) -> Result<(), String> {
    let dir = Path::new(&worktree.path);
    let status = merge::git(dir, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err("uncommitted changes".to_string());
    }
    let rev = worktree.branch.as_deref().unwrap_or("HEAD");
    let count = merge::git(dir, &["rev-list", "--count", rev, "--not", "--remotes"])?;
    if count.trim() != "0" {
        return Err("unpushed commits".to_string());
    }
    merge::git(Path::new(repo), &["worktree", "remove", &worktree.path])?;
    Ok(())
}

/// Whether the branch's tip is on the integration branch.
fn branch_on_integration(repo: &str, config: &ProjectConfig, tip: &str) -> bool {
    let remote = &config.conventions.remote;
    let integration = format!("{remote}/{}", config.conventions.integration);
    merge::git(
        Path::new(repo),
        &["merge-base", "--is-ancestor", tip, &integration],
    )
    .is_ok()
}

/// Delete a branch on the remote.
fn delete_remote_branch(repo: &str, remote: &str, branch: &str) -> Result<(), String> {
    merge::git(Path::new(repo), &["push", remote, "--delete", branch])?;
    Ok(())
}

/// The Herdr workspaces of the engine's session labelled `<project>-…`: the
/// homes GEAR opened for the project's seats (`prometheus-captain` stayed open
/// on a deleted folder after M4 landed). The agent status says whether the
/// workspace holds an agent.
fn project_workspaces(project: &str) -> Vec<ProjectWorkspace> {
    let Ok(out) = std::process::Command::new("herdr")
        .args(["workspace", "list"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    let prefix = format!("{project}-");
    value["result"]["workspaces"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|workspace| {
                    let label = workspace["label"].as_str()?;
                    let id = workspace["workspace_id"].as_str()?;
                    if !label.starts_with(&prefix) {
                        return None;
                    }
                    let holds_agent = workspace["agent_status"]
                        .as_str()
                        .is_some_and(|status| status != "unknown");
                    Some(ProjectWorkspace {
                        id: id.to_string(),
                        label: label.to_string(),
                        holds_agent,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The project's branches on the remote (`<project>/…`), named with their tip:
/// a landed voyage keeps none, the captain's home included.
fn project_branches(repo: &str, remote: &str, project: &str) -> Vec<ProjectBranch> {
    let Ok(out) = std::process::Command::new("git")
        .args([
            "ls-remote",
            "--heads",
            remote,
            &format!("refs/heads/{project}/*"),
        ])
        .current_dir(repo)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let tip = parts.next()?;
            let reference = parts.next()?;
            let name = reference.strip_prefix("refs/heads/")?;
            Some(ProjectBranch {
                name: name.to_string(),
                tip: tip.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::Call;
    use gear_model::{AttemptState, SeatRole, TaskState};
    use gear_store::{testing, AttemptRow, AttemptStep, NewMessage, TaskRow, VoyageRow};
    use std::process::Command;

    const NOW: Ts = 1_000;

    fn sh(dir: &Path, command: &str) -> String {
        let output = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn sh_ok(dir: &Path, command: &str) -> bool {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success()
    }

    /// A landed voyage of a real repository: a bare origin, its checkout, the
    /// merge worktree Herdr has open, and one project with one done task and
    /// its landing order answered. `cap` sets the captain's seat branch.
    struct Landed {
        root: tempfile::TempDir,
        work: PathBuf,
        origin: PathBuf,
        store: Store,
        herdr: gear_herdr::fake::FakeHerdr,
    }

    const CAP: &str = "p/captain/M1";

    fn config_text(repo: &Path) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "{}"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "hull"
duty = "everything"
owns = ["src/"]
kind = "pi"
profile = "pi"

[conventions]
integration = "dev"
gate = "true"
"#,
            repo.display()
        )
    }

    impl Landed {
        fn new(name: &str) -> Landed {
            let root = tempfile::tempdir().unwrap();
            let origin = root.path().join("origin.git");
            let work = root.path().join("work");
            let merge = root.path().join("merge");
            sh(
                root.path(),
                &format!("git init -q --bare {}", origin.display()),
            );
            sh(
                root.path(),
                &format!("git init -q -b dev {}", work.display()),
            );
            sh(&work, "git config user.email t@t && git config user.name t");
            sh(
                &work,
                "echo dev > README.md && git add README.md && git commit -q -m base",
            );
            sh(
                &work,
                &format!("git remote add origin {}", origin.display()),
            );
            sh(&work, "git push -q origin HEAD:refs/heads/dev");
            // The captain's seat branch: pushed at the landing's branch point.
            sh(
                &work,
                &format!(
                    "git checkout -q -b {CAP} && git push -q origin {CAP} && git checkout -q dev"
                ),
            );
            // The merge worktree, opened the way a merge opens it.
            sh(
                root.path(),
                &format!(
                    "git -C {} worktree add -q -b p/merge/M1 {} origin/dev",
                    work.display(),
                    merge.display()
                ),
            );
            let mut store = Store::open_in_memory().unwrap();
            store
                .write(|tx| {
                    testing::seed_project(tx, "p", "sess", NOW)?;
                    let mut project = tx.get_project("p")?.unwrap();
                    project.config_toml = config_text(&work);
                    tx.update_project(&project)?;
                    testing::seed_active_seat(
                        tx,
                        "p",
                        "captain",
                        SeatRole::Captain,
                        "w1:p1",
                        "codex",
                        NOW,
                    )?;
                    tx.upsert_voyage(&VoyageRow {
                        project: "p".into(),
                        milestone: "M1".into(),
                        title: format!("voyage {name}"),
                        done: Some("no red test on dev".into()),
                        bar: Some("FASTEST".into()),
                        eta: Some("4h".into()),
                        departure: Some(NOW),
                        departure_tasks: Some(1),
                        departure_estimate_min: Some(30),
                        updated: NOW,
                    })?;
                    tx.insert_task(&TaskRow {
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        project: "p".into(),
                        id: "T-1".into(),
                        rank: 1,
                        state: TaskState::Done,
                        priority: "must".into(),
                        lane: Some("hull".into()),
                        milestone: Some("M1".into()),
                        required: false,
                        contract_version: 1,
                        deps: vec![],
                        followup_of: None,
                        wait_on: None,
                        wait_cond: None,
                        rework_note: None,
                        estimate_min: Some(30),
                        issue: None,
                        updated: NOW,
                    })?;
                    let land = tx.insert_message(&NewMessage {
                        project: "p".into(),
                        thread: "station:M1".into(),
                        task: None,
                        attempt: None,
                        sender: "engine".into(),
                        sender_gen: 0,
                        recipient: "p:captain".into(),
                        kind: MsgKind::System,
                        urgent: true,
                        body: "Voyage M1 is ready to land".into(),
                        reply_to: None,
                        needs_ack: false,
                        state: MsgState::Open,
                        created: NOW,
                        meta: None,
                    })?;
                    tx.set_meta("station:p:M1:land", &land.id)?;
                    tx.update_message_state(
                        "p",
                        &land.id,
                        MsgState::Resolved,
                        Some(&format!("station {}: land", land.id)),
                        None,
                        None,
                        NOW + 1,
                    )?;
                    Ok(())
                })
                .unwrap();
            let mut herdr = gear_herdr::fake::FakeHerdr::new();
            herdr.add_worktree("p/merge/M1", &merge.display().to_string(), "wt-merge:p1");
            Landed {
                root,
                work,
                origin,
                store,
                herdr,
            }
        }

        /// One records commit on the captain's branch, pushed.
        fn records(&self, path: &str, content: &str) {
            sh(
                &self.work,
                &format!(
                    "git checkout -q {CAP} && mkdir -p $(dirname {path}) && echo '{content}' > {path} && \
                     git add {path} && git commit -q -m 'records of M1' && git push -q origin {CAP} && \
                     git checkout -q dev"
                ),
            );
        }

        /// The captain's branch changes `README.md`'s only line; a later commit
        /// on the integration branch changes it again, so the merge conflicts.
        fn conflicting_records(&self) {
            sh(
                &self.work,
                &format!(
                    "git checkout -q {CAP} && echo records > README.md && git commit -q -am 'records of M1' && \
                     git push -q origin {CAP} && git checkout -q dev && \
                     echo later > README.md && git commit -q -am 'later on dev' && git push -q origin dev"
                ),
            );
        }

        /// An accepted attempt of the voyage's only task: handed in and judged,
        /// and merged at `merged` when its work reached the integration branch.
        fn accepted(&mut self, merged: Option<Ts>) {
            self.store
                .write(|tx| {
                    tx.insert_attempt(&AttemptRow {
                        project: "p".into(),
                        task: "T-1".into(),
                        n: 1,
                        seat: "p:hull".into(),
                        generation: 1,
                        contract_version: 1,
                        adopted_version: 1,
                        pending_update: None,
                        state: AttemptState::Accepted,
                        candidate: Some("abc1234".into()),
                        report: Some("gear/voyages/M1/reports/T-1.md".into()),
                        safe_point: None,
                        started: NOW + 2,
                        updated: NOW + 2,
                        session_ref: None,
                        session_cli: None,
                        session_action: None,
                        sent_at: None,
                    })?;
                    tx.set_attempt_time("p", "T-1", 1, AttemptStep::HandedIn, NOW + 2)?;
                    tx.set_attempt_time("p", "T-1", 1, AttemptStep::Verdict, NOW + 3)?;
                    if let Some(merged) = merged {
                        tx.set_attempt_time("p", "T-1", 1, AttemptStep::Merged, merged)?;
                    }
                    Ok(())
                })
                .unwrap();
        }

        /// The merge-failed item of the voyage's only task, open, as the merge
        /// scan opens one. Returns its id.
        fn merge_failed(&mut self) -> String {
            self.store
                .write(|tx| {
                    Ok(tx
                        .insert_message(&NewMessage {
                            project: "p".into(),
                            thread: "merge-failed:T-1".into(),
                            task: Some("T-1".into()),
                            attempt: Some(1),
                            sender: "engine".into(),
                            sender_gen: 0,
                            recipient: "p:captain".into(),
                            kind: MsgKind::System,
                            urgent: true,
                            body: "the gate is red: T-1 did not land".into(),
                            reply_to: None,
                            needs_ack: false,
                            state: MsgState::Open,
                            created: NOW + 4,
                            meta: None,
                        })?
                        .id)
                })
                .unwrap()
        }

        fn reclaim(&mut self, now: Ts) -> Option<&'static str> {
            reclaim(&mut self.store, &mut self.herdr, "p", now).unwrap()
        }

        /// The records step alone, without the reclaim's worktree removal.
        fn merge_records(&mut self, now: Ts) -> Option<&'static str> {
            let config = self
                .store
                .read(|tx| {
                    let row = tx.get_project("p")?.unwrap();
                    Ok(gear_app::config::parse_row(&row).unwrap())
                })
                .unwrap();
            merge_records(&mut self.store, &mut self.herdr, "p", &config, "M1", now).unwrap()
        }

        /// The subjects on the integration branch, newest first.
        fn dev_log(&self) -> String {
            sh(
                &self.root.path(),
                &format!("git -C {} log --format=%s dev", self.origin.display()),
            )
        }

        /// The branches origin still has.
        fn origin_branches(&self) -> String {
            sh(
                &self.root.path(),
                &format!(
                    "git -C {} for-each-ref --format='%(refname:short)' refs/heads",
                    self.origin.display()
                ),
            )
        }

        fn local_branch(&self, branch: &str) -> bool {
            sh_ok(
                &self.work,
                &format!("git rev-parse --verify --quiet {branch}"),
            )
        }

        fn meta(&mut self, key: &str) -> Option<String> {
            self.store.read(|tx| tx.get_meta(key)).unwrap()
        }

        fn items(&mut self, thread: &str) -> Vec<gear_store::MessageRow> {
            self.store
                .read(|tx| tx.thread_messages("p", thread))
                .unwrap()
        }
    }

    /// T-585: a captain branch with one records commit is merged into the
    /// integration branch under `records: <project> <milestone>` and deleted
    /// on the remote and locally; the landing's records step is done once.
    #[test]
    fn a_landing_merges_the_captains_records_and_deletes_the_branch() {
        let mut landed = Landed::new("merged");
        landed.records("voyages/M1/report.md", "the voyage report");
        let tip = sh(
            &landed.root.path(),
            &format!("git -C {} rev-parse {CAP}", landed.origin.display()),
        );

        let changed = landed.reclaim(NOW + 600);

        assert!(changed.is_some(), "the records step wrote something");
        let log = landed.dev_log();
        assert_eq!(log.lines().next(), Some("records: p M1"), "{log}");
        assert!(log.contains("records of M1"), "{log}");
        assert_eq!(
            sh(
                &landed.root.path(),
                &format!(
                    "git -C {} show dev:voyages/M1/report.md",
                    landed.origin.display()
                )
            ),
            "the voyage report"
        );
        assert!(
            !landed.origin_branches().contains(CAP),
            "the branch is gone from the remote: {}",
            landed.origin_branches()
        );
        assert!(!landed.local_branch(CAP), "and locally");
        assert_eq!(
            landed.meta(&records_key("p", "M1")).as_deref(),
            Some(tip.trim()),
            "the landing's records step remembers the tip it merged"
        );
        assert!(
            landed.items(&records_thread("M1")).is_empty(),
            "nothing to tell the captain"
        );
    }

    /// The project's release branch is never named as a leftover of a landed
    /// voyage; a voyage's own branch still is.
    #[test]
    fn the_reclaim_never_names_the_projects_release_branch() {
        let mut landed = Landed::new("long-lived");
        landed
            .store
            .write(|tx| {
                let mut project = tx.get_project("p")?.unwrap();
                project.config_toml = project.config_toml.replace(
                    "integration = \"dev\"\n",
                    "integration = \"dev\"\nrelease = \"p/main\"\n",
                );
                tx.update_project(&project)
            })
            .unwrap();
        sh(
            &landed.root.path(),
            &format!("git -C {} branch p/main dev", landed.origin.display()),
        );
        // p/hull/t9 has an extra commit: not on the integration branch, so it
        // is named (T-618).
        sh(
            &landed.work,
            "git checkout -q -b p/hull/t9 dev && echo extra > extra.txt && git add extra.txt && \
             git commit -q -m extra && git push -q origin p/hull/t9 && git checkout -q dev",
        );

        landed.reclaim(NOW + 600);

        let item = landed
            .items(RECLAIM_THREAD)
            .into_iter()
            .next()
            .expect("the reclaim names what is left");
        assert!(item.body.contains("branch p/hull/t9"), "{}", item.body);
        assert!(!item.body.contains("branch p/main"), "{}", item.body);
    }

    /// T-585: a conflicting branch opens one item on thread `records:M1`,
    /// naming the branch and the git error, and nothing is deleted.
    #[test]
    fn a_conflicting_records_branch_opens_the_captains_item_and_stays() {
        let mut landed = Landed::new("conflict");
        landed.conflicting_records();

        landed.reclaim(NOW + 600);

        let items = landed.items(&records_thread("M1"));
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].state, MsgState::Open, "{items:?}");
        assert!(items[0].urgent, "a landing that kept its records is urgent");
        let body = &items[0].body;
        assert!(body.contains(CAP), "{body}");
        assert!(
            body.contains("CONFLICT") || body.contains("conflict"),
            "the git error is named: {body}"
        );
        assert!(
            landed.origin_branches().contains(CAP),
            "nothing is deleted: {}",
            landed.origin_branches()
        );
        assert!(landed.local_branch(CAP), "not even locally");
        assert!(
            !landed.dev_log().contains("records: p M1"),
            "nothing landed"
        );
        assert_eq!(landed.meta(&records_key("p", "M1")), None);
    }

    /// T-585: a branch the integration branch already holds is only deleted.
    #[test]
    fn a_records_branch_already_on_the_integration_branch_is_only_deleted() {
        let mut landed = Landed::new("empty");
        landed.records("voyages/M1/report.md", "the voyage report");
        let tip = sh(
            &landed.root.path(),
            &format!("git -C {} rev-parse {CAP}", landed.origin.display()),
        );
        // The captain's commit reaches dev some other way (a fast-forward).
        sh(
            &landed.work,
            &format!("git merge -q --ff-only {CAP} && git push -q origin dev"),
        );

        landed.reclaim(NOW + 600);

        assert!(
            !landed.origin_branches().contains(CAP),
            "the branch is deleted on the remote: {}",
            landed.origin_branches()
        );
        assert!(!landed.local_branch(CAP), "and locally");
        assert!(
            !landed.dev_log().contains("records: p M1"),
            "nothing was merged: {}",
            landed.dev_log()
        );
        assert_eq!(
            landed.meta(&records_key("p", "M1")).as_deref(),
            Some(tip.trim())
        );
    }

    /// T-585: a push the remote refuses opens the item and deletes nothing.
    #[test]
    fn a_records_push_the_remote_refuses_opens_the_item_and_deletes_nothing() {
        let mut landed = Landed::new("push-fails");
        landed.records("voyages/M1/report.md", "the voyage report");
        sh(
            &landed.root.path(),
            &format!(
                "printf '#!/bin/sh\\nexit 1\\n' > {}/hooks/pre-receive && chmod +x {}/hooks/pre-receive",
                landed.origin.display(),
                landed.origin.display()
            ),
        );

        landed.reclaim(NOW + 600);

        let items = landed.items(&records_thread("M1"));
        assert_eq!(items.len(), 1, "{items:?}");
        let body = &items[0].body;
        assert!(body.contains(CAP), "{body}");
        assert!(body.contains("push"), "the git error is named: {body}");
        assert!(
            landed.origin_branches().contains(CAP),
            "nothing is deleted: {}",
            landed.origin_branches()
        );
        assert!(landed.local_branch(CAP));
        assert!(
            !landed.dev_log().contains("records: p M1"),
            "nothing landed"
        );
        assert_eq!(landed.meta(&records_key("p", "M1")), None);
    }

    /// T-585: the records step is done once per landing — a second scan writes
    /// nothing.
    #[test]
    fn a_second_scan_does_not_merge_the_records_again() {
        let mut landed = Landed::new("twice");
        landed.records("voyages/M1/report.md", "the voyage report");

        assert!(landed.reclaim(NOW + 600).is_some());
        let after = landed.dev_log();
        let branches = landed.origin_branches();

        assert_eq!(landed.reclaim(NOW + 2 * RECLAIM_EVERY_S), None);
        assert_eq!(landed.dev_log(), after, "no second merge commit");
        assert_eq!(landed.origin_branches(), branches);
    }

    /// T-619: a records commit pushed after the landing is merged on a later
    /// scan and the branch deleted; no new commit, nothing happens. The merge
    /// worktree is left in place (the records step alone, not the reclaim).
    #[test]
    fn a_new_records_commit_after_the_landing_is_merged_on_a_later_scan() {
        let mut landed = Landed::new("again");
        landed.records("voyages/M1/report.md", "the voyage report");
        let first_tip = sh(
            &landed.root.path(),
            &format!("git -C {} rev-parse {CAP}", landed.origin.display()),
        );
        assert!(landed.merge_records(NOW + 600).is_some());
        assert_eq!(
            landed.meta(&records_key("p", "M1")).as_deref(),
            Some(first_tip.trim())
        );
        // A records apply after the landing rewrites the plan on the captain's
        // branch: a new commit appears on the remote.
        sh(
            &landed.work,
            &format!(
                "git checkout -q -b {CAP} dev && echo more > more.txt && git add more.txt && \
                 git commit -q -m 'more records' && git push -q origin {CAP} && git checkout -q dev"
            ),
        );
        let second_tip = sh(
            &landed.root.path(),
            &format!("git -C {} rev-parse {CAP}", landed.origin.display()),
        );
        assert!(landed.merge_records(NOW + 900).is_some());
        assert!(
            landed.dev_log().contains("more records"),
            "{}",
            landed.dev_log()
        );
        assert_eq!(
            landed.meta(&records_key("p", "M1")).as_deref(),
            Some(second_tip.trim())
        );
        assert!(!landed.origin_branches().contains(CAP));
        // No new commit: nothing happens.
        let after = landed.dev_log();
        assert_eq!(landed.merge_records(NOW + 1200), None);
        assert_eq!(landed.dev_log(), after, "no third merge commit");
    }

    /// T-619: a merge worktree the reclaim removed (T-618) is dropped from
    /// Herdr and opened again the same way before the records merge.
    #[test]
    fn a_removed_merge_worktree_is_reopened_before_the_records_merge() {
        let mut landed = Landed::new("reopen");
        landed.records("voyages/M1/report.md", "the voyage report");
        // The reclaim removed the merge worktree: its directory is gone.
        std::fs::remove_dir_all(landed.root.path().join("merge")).unwrap();
        landed.merge_records(NOW + 600);
        let calls = landed.herdr.calls.clone();
        let removed = calls.iter().position(
            |call| matches!(call, Call::WorktreeRemove { workspace } if workspace == "wt-p-merge-M1"),
        );
        let created = calls.iter().position(
            |call| matches!(call, Call::WorktreeCreate { branch, .. } if branch == "p/merge/M1"),
        );
        assert!(removed.is_some(), "{calls:?}");
        assert!(created.is_some(), "{calls:?}");
        assert!(removed < created, "remove before create: {calls:?}");
    }

    /// T-595: a voyage whose every task is Done but whose accepted work has not
    /// merged is not finished: its merge-failed item stays open for the captain
    /// (GEAR M6.2: T-584's merge failed its gate, and the sweep closed both
    /// items as "voyage M6.2 is finished" — nothing would ever merge it).
    #[test]
    fn a_voyage_with_accepted_work_not_merged_is_not_finished() {
        let mut landed = Landed::new("unmerged");
        landed.accepted(None);
        let item = landed.merge_failed();

        let closed = close(&mut landed.store, "p", NOW + 600).unwrap();

        assert!(closed.is_empty(), "nothing closes: {closed:?}");
        let items = landed.items("merge-failed:T-1");
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].id, item);
        assert_eq!(items[0].state, MsgState::Open, "the merge is still work");
    }

    /// T-595: once that attempt has a merge time the voyage is finished, and the
    /// sweep closes the item as it did before.
    #[test]
    fn a_voyage_whose_accepted_work_merged_is_finished_and_its_item_closes() {
        let mut landed = Landed::new("merged-work");
        landed.accepted(Some(NOW + 5));
        let item = landed.merge_failed();

        let closed = close(&mut landed.store, "p", NOW + 600).unwrap();

        assert_eq!(closed, vec![item.clone()]);
        let items = landed.items("merge-failed:T-1");
        assert_eq!(items[0].state, MsgState::Resolved, "{items:?}");
        assert_eq!(
            items[0].result.as_deref(),
            Some(reason("M1").as_str()),
            "{items:?}"
        );
    }

    /// T-618: a clean worktree whose branch is on the remote is removed by
    /// GEAR and not named.
    #[test]
    fn a_clean_pushed_worktree_is_removed_and_not_named() {
        let mut landed = Landed::new("clean");
        let merge = landed.root.path().join("merge");
        assert!(merge.exists(), "the merge worktree is there");
        landed.reclaim(NOW + 600);
        assert!(!merge.exists(), "the clean pushed worktree is removed");
        let items = landed.items(RECLAIM_THREAD);
        assert!(
            items.iter().all(|item| !item.body.contains("worktree")),
            "the removed worktree is not named: {items:?}"
        );
    }

    /// T-618: a dirty worktree is named with "uncommitted changes" and kept.
    #[test]
    fn a_dirty_worktree_is_named_with_uncommitted_changes_and_kept() {
        let mut landed = Landed::new("dirty");
        let merge = landed.root.path().join("merge");
        std::fs::write(merge.join("dirty.txt"), "x").unwrap();
        landed.reclaim(NOW + 600);
        assert!(merge.exists(), "the dirty worktree is kept");
        let item = landed
            .items(RECLAIM_THREAD)
            .into_iter()
            .next()
            .expect("the reclaim names the dirty worktree");
        assert!(item.body.contains("worktree"), "{}", item.body);
        assert!(item.body.contains("uncommitted changes"), "{}", item.body);
    }

    /// T-618: a branch whose tip is on the integration branch is deleted on
    /// the remote; one with extra commits is named.
    #[test]
    fn a_branch_on_the_integration_branch_is_deleted_and_one_with_extra_commits_is_named() {
        let mut landed = Landed::new("branches");
        sh(
            &landed.root.path(),
            &format!("git -C {} branch p/hull/t9 dev", landed.origin.display()),
        );
        sh(
            &landed.work,
            "git checkout -q -b p/hull/t10 dev && echo extra > extra.txt && git add extra.txt && \
             git commit -q -m extra && git push -q origin p/hull/t10 && git checkout -q dev",
        );
        landed.reclaim(NOW + 600);
        let branches = landed.origin_branches();
        assert!(
            !branches.contains("p/hull/t9"),
            "the branch on dev is deleted: {branches}"
        );
        assert!(
            branches.contains("p/hull/t10"),
            "the branch with extra commits is kept: {branches}"
        );
        let item = landed
            .items(RECLAIM_THREAD)
            .into_iter()
            .next()
            .expect("the reclaim names the extra branch");
        assert!(item.body.contains("branch p/hull/t10"), "{}", item.body);
        assert!(item.body.contains("extra commits"), "{}", item.body);
        assert!(!item.body.contains("branch p/hull/t9"), "{}", item.body);
    }
}
