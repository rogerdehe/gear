//! The handover of a seat whose agent is switched (T-465, handover.md part A).
//!
//! Switching an agent means a new CLI, so the conversation is lost. The
//! incoming agent must not start over: its packet starts with a handover that
//! says whose work it takes over, where the previous conversation can be read,
//! and what the worktree holds now (uncommitted changes, unpushed commits, the
//! branch, the last commit). The facts are the engine's (it fills them even
//! when the outgoing agent could not write); the outgoing agent adds its own
//! summary and notes to the document while it wraps up.
//!
//! The document lives under the voyage folder, as handover.md part A names it:
//! `<voyage_dir>/handovers/<seat>-<date>-<time>.md`. The engine writes part 1,
//! keeps parts 2 and 3 as the outgoing agent left them, and commits the file
//! on the seat's own branch (only that path, so the agent's other changes are
//! not swept in). A git that cannot commit leaves the document untracked; the
//! packet still points at it.

use gear_herdr::HerdrAct;
use gear_model::{GearResult, SeatRole, Ts};
use gear_store::Store;

use crate::EngineCfg;

/// The meta key under which the block for the incoming agent's packet is
/// stored: JSON `{"generation", "task", "attempt", "doc", "block"}`.
pub fn meta_key(seat: &str) -> String {
    format!("engine.handover.{seat}")
}

/// What the handover document and the incoming packet carry: the facts the
/// engine fills (handover.md part A, section 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub seat: String,
    pub role: String,
    pub task: String,
    pub attempt: i64,
    pub contract_version: i64,
    pub why: String,
    pub old_cli: String,
    pub old_model: String,
    pub new_cli: String,
    pub new_model: String,
    pub session: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub last_commit: Option<String>,
    pub upstream: Option<String>,
    pub unpushed: usize,
    pub unpushed_list: Vec<String>,
    pub changed: Vec<String>,
    pub untracked: Vec<String>,
    pub stash: Option<String>,
    pub handin: String,
    pub acceptance: String,
    pub doc: String,
}

/// The handover of `seat`, if it holds a task worth handing over. `None` when
/// the seat is between tasks: nothing is being taken over.
pub fn collect(
    store: &mut Store,
    cfg: &EngineCfg,
    herdr: &mut dyn HerdrAct,
    project: &str,
    seat: &gear_store::SeatRow,
    old_cli: &str,
    old_model: &str,
    new_cli: &str,
    new_model: &str,
    now: Ts,
) -> GearResult<Option<Facts>> {
    let (task, attempt) = match seat.role {
        SeatRole::Lane => {
            let open = store.read(|tx| tx.open_attempts_of_seat(&seat.id))?;
            match open.into_iter().next() {
                Some(attempt) => {
                    let task = attempt.task.clone();
                    (task, attempt)
                }
                None => return Ok(None),
            }
        }
        SeatRole::Temp => {
            let Some(task) = seat.task_id.clone() else {
                return Ok(None);
            };
            let attempts = store.read(|tx| tx.attempts_of_task(project, &task))?;
            match attempts.last() {
                Some(attempt) => (task, attempt.clone()),
                None => return Ok(None),
            }
        }
        SeatRole::Captain => return Ok(None),
    };
    let acceptance = store
        .read(|tx| tx.get_contract(project, &task, attempt.adopted_version))?
        .map(|contract| contract.accept)
        .unwrap_or_default();
    let row = store
        .read(|tx| tx.get_task(project, &task))?
        .ok_or_else(|| {
            gear_model::GearError::new(
                gear_model::ErrorCode::NoTarget,
                format!("no task {task} in project {project}"),
            )
        })?;
    let milestone = row.milestone.clone().unwrap_or_default();
    let conventions = gear_app::config::conventions_or_default(store, project)?;
    let worktree = store
        .read(|tx| tx.get_meta(&crate::onboard::worktree_path_key(&seat.id)))?
        .or(store.read(|tx| tx.get_meta(&format!("engine.review_path.{}", seat.id)))?);
    let session = match store.read(|tx| tx.get_binding(&seat.id, seat.generation))? {
        Some(binding) => binding.session_ref.filter(|session| !session.is_empty()),
        None => None,
    }
    .or_else(|| attempt.session_ref.clone())
    .or_else(|| {
        worktree.as_ref().and_then(|_| {
            let pane = store
                .read(|tx| tx.get_binding(&seat.id, seat.generation))
                .ok()
                .flatten()
                .map(|binding| binding.pane_id)?;
            herdr
                .agent_get(&pane)
                .ok()
                .flatten()
                .and_then(|info| info.agent_session)
                .map(|session| session.value)
                .filter(|session| !session.is_empty())
        })
    });
    let mut facts = Facts {
        seat: seat.id.clone(),
        role: match seat.role {
            SeatRole::Lane => "lane".to_string(),
            SeatRole::Temp => "reviewer".to_string(),
            SeatRole::Captain => "captain".to_string(),
        },
        task: task.clone(),
        attempt: attempt.n,
        contract_version: attempt.adopted_version,
        why: format!(
            "the agent is switched to {new_cli} ({new_model}); the conversation does not cross CLIs"
        ),
        old_cli: old_cli.to_string(),
        old_model: old_model.to_string(),
        new_cli: new_cli.to_string(),
        new_model: new_model.to_string(),
        session,
        worktree: worktree.clone(),
        branch: None,
        last_commit: None,
        upstream: None,
        unpushed: 0,
        unpushed_list: Vec::new(),
        changed: Vec::new(),
        untracked: Vec::new(),
        stash: None,
        handin: hand_in(&attempt),
        acceptance,
        doc: handover_path(
            worktree.as_deref(),
            &conventions.voyage_dir,
            project,
            &milestone,
            seat,
            now,
        ),
    };
    if let Some(path) = worktree.as_deref().filter(|path| !path.is_empty()) {
        facts.branch = run(cfg, path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        facts.last_commit = run(cfg, path, &["log", "-1", "--format=%H %s"]);
        facts.stash = run(cfg, path, &["stash", "list", "-1", "--format=%gd %H %gs"]);
        if let Some(upstream) = run(cfg, path, &["rev-parse", "--abbrev-ref", "@{u}"]) {
            facts.upstream = Some(upstream.clone());
            facts.unpushed = run(
                cfg,
                path,
                &["rev-list", "--count", &format!("{upstream}..HEAD")],
            )
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or(0);
            facts.unpushed_list = run(
                cfg,
                path,
                &[
                    "log",
                    "--format=%h %s",
                    "-n",
                    "5",
                    &format!("{upstream}..HEAD"),
                ],
            )
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default();
        }
        // The porcelain output is column-aligned: it is not trimmed, or the
        // first line's leading space would eat a path's first character.
        if let Ok(status) = cfg.review_git.run(path, &["status", "--porcelain"]) {
            let doc_relative = facts
                .doc
                .strip_prefix(&format!("{}/", path.trim_end_matches('/')))
                .unwrap_or(facts.doc.as_str());
            for line in status.lines() {
                let Some(entry) = porcelain_path(line) else {
                    continue;
                };
                // The handover document is ours: git may name its whole new
                // folder (`?? gear/`) instead of the file.
                if entry == doc_relative
                    || doc_relative.starts_with(&format!("{}/", entry.trim_end_matches('/')))
                {
                    continue;
                }
                if line.starts_with("??") {
                    facts.untracked.push(entry);
                } else {
                    facts.changed.push(entry);
                }
            }
        }
    }
    Ok(Some(facts))
}

/// The document the outgoing agent finds while it wraps up: part 1 filled,
/// parts 2 and 3 left for it. `existing` is kept when the outgoing agent has
/// already written its summary or notes into the file.
pub fn document(facts: &Facts, existing: Option<&str>) -> String {
    let tail = existing
        .and_then(|text| text.find("\n## 2.").map(|at| text[at..].to_string()))
        .unwrap_or_else(|| {
            "\n## 2. Summary (the outgoing agent writes it; else the incoming agent writes it from the transcript)\n\n_(empty: read the session above and write this part before you work)_\n\n## 3. Free notes (optional)\n\n_(empty)_\n".to_string()
        });
    format!(
        "# Handover — {seat} ({role}) — {task}\n\n\
         <!--\n\
         Written for the incoming agent, following part A of the voyage's handover.md \
         (T-465). Part 1 is the engine's facts. The outgoing agent writes part 2 and, \
         if it wants, part 3; when it cannot, the incoming agent reads the session \
         named below and writes part 2 itself before it works.\n\
         -->\n\n\
         ## 1. Facts\n\n{body}{tail}",
        seat = facts.seat,
        role = facts.role,
        task = facts.task,
        body = fact_lines(facts),
        tail = tail
    )
}

/// The block the incoming agent's packet starts with: it takes over another
/// agent's work, reads the previous conversation first, then the handover
/// document, then the task packet.
pub fn block(facts: &Facts) -> String {
    let session = match facts.session.as_deref() {
        Some(session) => format!(
            "Previous conversation: {} session {session} (read it first; for a CLI that reports a file this is the transcript path).",
            facts.old_cli
        ),
        None => format!(
            "Previous conversation: {} reported no session id; the worktree below is the only record.",
            facts.old_cli
        ),
    };
    format!(
        "## Handover — you take over another agent's work\n\n\
         You take over the work of another agent on {seat} ({role}): {old_cli} ({old_model}) was \
         stopped and this agent continues the same task on {new_cli} ({new_model}). Read the \
         previous conversation first, then the handover document at {doc} — part 1 is the facts \
         below, parts 2 and 3 are the outgoing agent's own summary and notes — and only then the \
         task packet. Do not start over, and do not discard uncommitted changes you have not \
         inspected.\n\n\
         {session}\n\n\
         ### Facts\n\n{body}\
         If the document's part 2 is empty, read the session above and write it yourself before you work.\n\n\
         Then the task packet:\n\n",
        seat = facts.seat,
        role = facts.role,
        old_cli = facts.old_cli,
        old_model = facts.old_model,
        new_cli = facts.new_cli,
        new_model = facts.new_model,
        doc = facts.doc,
        session = session,
        body = fact_lines(facts)
    )
}

/// The facts as the document's and the packet's lines (handover.md part A,
/// section 1).
fn fact_lines(facts: &Facts) -> String {
    let list = |items: &[String], limit: usize| -> String {
        if items.is_empty() {
            return "none".to_string();
        }
        let shown = items
            .iter()
            .take(limit)
            .map(|item| item.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if items.len() > limit {
            format!("{shown} (+{} more)", items.len() - limit)
        } else {
            shown
        }
    };
    let mut text = String::new();
    text.push_str(&format!(
        "- Seat, role and task: {}, {}, task {}, attempt {}, contract v{}.\n",
        facts.seat, facts.role, facts.task, facts.attempt, facts.contract_version
    ));
    text.push_str(&format!("- Why the handover: {}.\n", facts.why));
    text.push_str(&format!(
        "- Previous agent: {} on {}, session {}.\n",
        facts.old_cli,
        if facts.old_model.is_empty() {
            "an unrecorded model"
        } else {
            facts.old_model.as_str()
        },
        facts.session.as_deref().unwrap_or("not reported")
    ));
    text.push_str(&format!(
        "- Incoming agent: {} on {}.\n",
        facts.new_cli, facts.new_model
    ));
    match facts.worktree.as_deref() {
        Some(path) => text.push_str(&format!("- Worktree: {path}.\n")),
        None => text.push_str("- Worktree: not recorded by the engine.\n"),
    }
    text.push_str(&format!(
        "- Branch: {}.\n",
        facts.branch.as_deref().unwrap_or("could not be read")
    ));
    text.push_str(&format!(
        "- Last commit: {}.\n",
        facts.last_commit.as_deref().unwrap_or("could not be read")
    ));
    match facts.upstream.as_deref() {
        Some(upstream) => {
            text.push_str(&format!(
                "- Unpushed commits: {} (against {upstream}).\n",
                facts.unpushed
            ));
            if !facts.unpushed_list.is_empty() {
                text.push_str(&format!("  - {}\n", facts.unpushed_list.join("\n  - ")));
            }
        }
        None => text.push_str("- Unpushed commits: no upstream branch was found.\n"),
    }
    text.push_str(&format!(
        "- Uncommitted changes: {}.\n",
        list(&facts.changed, 10)
    ));
    text.push_str(&format!(
        "- Untracked files: {}.\n",
        list(&facts.untracked, 10)
    ));
    text.push_str(&format!(
        "- Stash: {}.\n",
        facts.stash.as_deref().unwrap_or("none")
    ));
    text.push_str(&format!("- Hand-in: {}.\n", facts.handin));
    text.push_str(&format!(
        "- Checks: acceptance `{}`; last result not recorded by the engine (the outgoing agent's summary says).\n",
        one_line(&facts.acceptance)
    ));
    text
}

/// The hand-in state of the attempt: handed in or not, with its candidate and
/// report when it was.
fn hand_in(attempt: &gear_store::AttemptRow) -> String {
    match attempt.state {
        gear_model::AttemptState::Submitted
        | gear_model::AttemptState::Accepted
        | gear_model::AttemptState::Rejected => format!(
            "handed in at {} (report {})",
            attempt.candidate.as_deref().unwrap_or("no candidate"),
            attempt.report.as_deref().unwrap_or("not recorded")
        ),
        _ => match attempt.candidate.as_deref() {
            Some(candidate) => {
                format!("not handed in by this attempt; its candidate so far is {candidate}")
            }
            None => "not handed in".to_string(),
        },
    }
}

fn handover_path(
    worktree: Option<&str>,
    voyage_dir: &str,
    project: &str,
    milestone: &str,
    seat: &gear_store::SeatRow,
    now: Ts,
) -> String {
    let folder = if milestone.is_empty() {
        "gear/handovers".to_string()
    } else {
        format!(
            "{}/handovers",
            gear_model::layout::voyage_dir(voyage_dir, project, milestone)
        )
    };
    let name = seat.name.replace(':', "-");
    let stamp = file_stamp(now);
    match worktree {
        Some(path) if !path.is_empty() => {
            format!("{}/{folder}/{name}-{stamp}.md", path.trim_end_matches('/'))
        }
        _ => format!("{folder}/{name}-{stamp}.md"),
    }
}

/// `YYYY-MM-DD-HHMM` for the document's name (UTC, as the captain's handover
/// names its file).
fn file_stamp(ts: i64) -> String {
    let secs = ts.rem_euclid(86_400);
    format!(
        "{}-{:02}{:02}",
        gear_app::plan::utc_date(ts),
        secs / 3_600,
        (secs % 3_600) / 60
    )
}

fn run(cfg: &EngineCfg, path: &str, args: &[&str]) -> Option<String> {
    cfg.review_git
        .run(path, args)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The path of one `git status --porcelain` line (a rename or copy names its
/// new path last).
fn porcelain_path(line: &str) -> Option<String> {
    let rest = line.get(3..)?.trim();
    let path = rest.rsplit(" -> ").next().unwrap_or(rest).trim();
    (!path.is_empty()).then(|| path.to_string())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Write the handover document at `facts.doc` (part 1 refreshed, parts 2 and 3
/// kept as the outgoing agent left them) and commit only that path on the
/// seat's branch. A git that refuses leaves the file in place, untracked.
pub fn write_document(cfg: &EngineCfg, facts: &Facts, commit: bool) -> GearResult<()> {
    // A worktree the engine cannot write into (a seat from an older engine, a
    // fake path in a test) still gets its block: the facts are in the packet,
    // the document is the edge case outside normal use.
    let Some(worktree) = facts.worktree.as_deref().filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    if !std::path::Path::new(worktree).is_dir() {
        return Ok(());
    }
    let path = std::path::Path::new(&facts.doc);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return Ok(());
        }
    }
    let existing = std::fs::read_to_string(path).ok();
    if std::fs::write(path, document(facts, existing.as_deref())).is_err() {
        return Ok(());
    }
    if !commit {
        return Ok(());
    }
    let relative = facts
        .doc
        .strip_prefix(&format!("{}/", worktree.trim_end_matches('/')))
        .unwrap_or(facts.doc.as_str())
        .to_string();
    // Only the handover path: the agent's own staged work is not swept into
    // this commit.
    if cfg.review_git.run(worktree, &["add", &relative]).is_err() {
        return Ok(());
    }
    let message = format!("gear: handover of {} for {}", facts.seat, facts.task);
    let _ = cfg
        .review_git
        .run(worktree, &["commit", "-m", &message, "--", &relative]);
    Ok(())
}

/// Store the block for the incoming agent's packet, tied to the generation
/// that will read it.
pub fn store_block(
    store: &mut Store,
    seat: &str,
    generation: i64,
    facts: &Facts,
) -> GearResult<()> {
    let value = serde_json::json!({
        "generation": generation,
        "task": facts.task,
        "attempt": facts.attempt,
        "doc": facts.doc,
        "block": block(facts),
    });
    store.write(|tx| tx.set_meta(&meta_key(seat), &value.to_string()))
}

/// The handover block for `seat`'s packet when it is on `generation` and the
/// handover is about `task`/`attempt`.
pub fn stored_block(
    store: &mut Store,
    seat: &str,
    generation: i64,
    task: &str,
    attempt: i64,
) -> GearResult<Option<String>> {
    let Some(raw) = store.read(|tx| tx.get_meta(&meta_key(seat)))? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    if value["generation"].as_i64() != Some(generation)
        || value["task"].as_str() != Some(task)
        || value["attempt"].as_i64() != Some(attempt)
    {
        return Ok(None);
    }
    Ok(value["block"].as_str().map(str::to_string))
}
