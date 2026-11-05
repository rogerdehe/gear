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

use gear_model::{GearResult, MsgState, Ts};
use gear_store::{Store, Tx};
use std::collections::BTreeSet;

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
/// none of their tasks open (the rule `tasks_live_of_project` already uses,
/// T-529: a voyage with a task that is not terminal is open).
fn finished_voyages(tx: &Tx<'_>, project: &str) -> GearResult<Vec<FinishedRow>> {
    let open: BTreeSet<String> = tx
        .tasks_live_of_project(project)?
        .into_iter()
        .filter_map(|task| task.milestone)
        .collect();
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
        let rows = finished_voyages(tx, project)?;
        let any_open = !tx
            .tasks_live_of_project(project)?
            .into_iter()
            .filter_map(|task| task.milestone)
            .collect::<BTreeSet<String>>()
            .is_empty();
        Ok(Sweep { rows, any_open })
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

/// The thread of the reclaim item: worktrees a landed voyage left behind.
pub const RECLAIM_THREAD: &str = "reclaim";
const RECLAIM_EVERY_S: i64 = 300;
/// How long after its landing a voyage's leftovers are named.
const RECLAIM_WITHIN_S: i64 = 24 * 3600;

/// A landed voyage keeps no worktree (the user, 2026-11-05: Prometheus M4
/// landed with fourteen worktrees and six Herdr workspaces left). Once the
/// project has no open voyage and its newest finished voyage was landed by
/// `station <id>: land`, every worktree of the project's repositories (its
/// path named `<project>-…` or its branch `<project>/…`) is named to the
/// captain in one item, until none is left. Read every `RECLAIM_EVERY_S`.
pub fn reclaim(store: &mut Store, project: &str, now: Ts) -> GearResult<Option<&'static str>> {
    let landed = store.read(|tx| {
        let sweep = Sweep::read(tx, project)?;
        if sweep.any_open {
            return Ok(false);
        }
        let Some(land) = sweep.rows.iter().rev().find_map(|row| row.land.clone()) else {
            return Ok(false);
        };
        // Only a landing of the last day is checked: an older voyage was
        // landed before this check existed, and its captain is not rung about
        // it (Lawrence M1, landed days before: the user wants it left alone).
        Ok(tx.get_message(project, &land)?.is_some_and(|message| {
            message.state == MsgState::Resolved
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
                })
        }))
    })?;
    let open = store.read(|tx| {
        Ok(tx
            .thread_messages(project, RECLAIM_THREAD)?
            .into_iter()
            .find(|message| message.state == MsgState::Open))
    })?;
    if !landed {
        return Ok(None);
    }
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
    let mut left = Vec::new();
    let mut seen = BTreeSet::new();
    for repo in config.repos() {
        if !seen.insert(repo.path.clone()) {
            continue;
        }
        left.extend(project_worktrees(&repo.path, project));
        left.extend(project_branches(
            &repo.path,
            &config.conventions.remote,
            project,
        ));
    }
    left.extend(project_workspaces(project));
    store.write(|tx| {
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
            "The voyage landed, but {} worktree(s), branch(es) or Herdr workspace(s) of {project} are left: {}. A landed voyage keeps none: merge what is not on the integration branch (the voyage records included), then remove each worktree with `git worktree remove`, delete each branch on the remote, and close each workspace with `herdr workspace close <id>`.",
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
    })
}

/// The worktrees of `repo` that belong to `project`: a path whose last part
/// starts with `<project>-`, or a branch under `<project>/`. The main
/// checkout is never one.
fn project_worktrees(repo: &str, project: &str) -> Vec<String> {
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
            found.push(path);
        }
    }
    found
}

/// The Herdr workspaces of the engine's session labelled `<project>-…`: the
/// homes GEAR opened for the project's seats (`prometheus-captain` stayed open
/// on a deleted folder after M4 landed). Named as `workspace <id> (<label>)`.
fn project_workspaces(project: &str) -> Vec<String> {
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
                    label
                        .starts_with(&prefix)
                        .then(|| format!("workspace {id} ({label})"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The project's branches on the remote (`<project>/…`), named as
/// `branch <name>`: a landed voyage keeps none, the captain's home included.
fn project_branches(repo: &str, remote: &str, project: &str) -> Vec<String> {
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
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|reference| reference.strip_prefix("refs/heads/"))
        .map(|name| format!("branch {name}"))
        .collect()
}
