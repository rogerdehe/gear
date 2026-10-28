//! `gear` home page (design/13). Read-only for business state; records one
//! ViewRecord of exactly what was displayed (evidence for op confirmation).

use crate::identity::{require_seat, Fingerprint};
use crate::mail;
use crate::Env;
use gear_model::{GearResult, SeatRole, Ts};
use gear_store::{Store, Tx, ViewRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    /// Text printed to the agent (≈120 lines max).
    pub text: String,
    /// What was shown; written by [`record_view`] only after the text was printed.
    pub view: ViewRecord,
}

/// Build the home page for the caller's seat.
///
/// Slice 1 content:
/// - identity line: seat, role, generation;
/// - captain: the inbox batch from `mail::select_batch` (thread id, message ids,
///   original bodies, "+N more"), then unshown Reply/Instruction messages;
/// - lane/temp: unshown Reply/Instruction messages addressed to it (originals);
/// - lane/temp (T-51): a "current task" line for the seat's open attempt
///   (`open_attempts_of_seat`, first): `task <T-n> attempt <n> contract v<adopted>
///   — <goal of that contract version>`; the view records `shown_task`,
///   `shown_attempt`, `shown_contract` (the engine confirms deliveries from them);
///   no open attempt → no line, those fields None;
/// - footer: suggested next command.
/// "Unshown" = created after the seat's latest view that listed it.
pub fn build(store: &mut Store, env: &Env, fp: Option<&Fingerprint>, now: Ts) -> GearResult<Home> {
    build_with_agents(store, env, fp, now, None)
}

/// Build with an optional Herdr snapshot read before the store transaction.
pub fn build_with_agents(
    store: &mut Store, env: &Env, fp: Option<&Fingerprint>, now: Ts,
    agents: Option<&[gear_herdr::AgentInfo]>,
) -> GearResult<Home> {
    let (text, view) = store.read(|tx| {
        let (ctx, seat) = require_seat(tx, env, fp, None, None)?;
        let seat_id = seat.id.clone();

        let mut lines: Vec<String> = Vec::new();
        if seat.role == SeatRole::Captain {
            lines.extend(gear_board::voyage::reports(tx, &ctx.project, now)?
                .iter().flat_map(|v| v.lines()));
            if !lines.is_empty() {
                lines.push(String::new());
            }
        }
        lines.push(format!(
            "seat {seat_id} · {} · generation {}",
            seat.role.as_str(),
            ctx.generation
        ));

        // The captain's snapshot: the plan at a glance, before the inbox
        // (T-9). The model is the board's, so the two views never disagree.
        if seat.role == SeatRole::Captain {
            lines.push(String::new());
            lines.extend(snapshot(tx, &ctx.project, now, agents)?);
        }

        // A message counts as shown when a recorded view of this seat and
        // generation lists its id. The ids decide, not the timestamps: a
        // message created in the same second as the view must not be lost,
        // and a message no view ever listed stays unshown.
        let mut already_shown: Vec<String> = Vec::new();
        for view in tx.views_since(&seat_id, ctx.generation, 0)? {
            already_shown.extend(view.shown_msgs);
        }

        let mut shown_msgs: Vec<String> = Vec::new();
        let mut shown_task: Option<String> = None;
        let mut shown_attempt: Option<i64> = None;
        let mut shown_contract: Option<i64> = None;

        // A lane or temp sees the task of its open attempt (design/13: the
        // current task comes right after the identity line). A pending contract
        // update is shown as the version the lane will work to, and recording
        // this view adopts it (T-8, design/06).
        if seat.role != SeatRole::Captain {
            if let Some(attempt) = tx.open_attempts_of_seat(&seat_id)?.first() {
                let version = attempt.pending_update.unwrap_or(attempt.adopted_version);
                let goal = tx
                    .get_contract(&ctx.project, &attempt.task, version)?
                    .map(|contract| contract.goal)
                    .unwrap_or_default();
                lines.push(format!(
                    "task {} attempt {} contract v{} — {}",
                    attempt.task, attempt.n, version, goal
                ));
                shown_task = Some(attempt.task.clone());
                shown_attempt = Some(attempt.n);
                shown_contract = Some(version);
            }
        }

        if seat.role == SeatRole::Captain {
            // The inbox batch: thread, message ids, the original bodies.
            let batch = mail::select_batch(tx, &ctx.project, &seat_id, now, mail::BatchLimits::default())?;
            if !batch.is_empty() {
                lines.push(String::new());
                lines.push(format!("INBOX ({} threads)", batch.len()));
                for thread in &batch {
                    let ids: Vec<&str> = thread.open.iter().map(|message| message.id.as_str()).collect();
                    let urgent = if thread.urgent { " [urgent]" } else { "" };
                    lines.push(format!("  {}{urgent} {}", thread.thread, ids.join(" ")));
                    for message in &thread.open {
                        lines.push(format!("    {}", message.body));
                        shown_msgs.push(message.id.clone());
                    }
                    if thread.more > 0 {
                        lines.push(format!("    +{} more", thread.more));
                    }
                }
            }
        }

        // The replies and instructions the seat has not seen yet. A message
        // the batch above already printed is not printed again.
        // Only this job's messages count (T-71): a warm seat keeps its id
        // across jobs, so another task's answers must not read as
        // instructions for this one — even when they were written after this
        // job started (F1 of the T-71 review).
        let since = crate::seats::generation_since(tx, &seat_id, ctx.generation)?;
        let unshown: Vec<_> = tx
            .messages_to_since(&ctx.project, &seat_id, since)?
            .into_iter()
            .filter(|message| {
                !already_shown.contains(&message.id)
                    && !shown_msgs.contains(&message.id)
                    && crate::seats::message_belongs(&seat, message, since)
            })
            .collect();
        if !unshown.is_empty() {
            lines.push(String::new());
            lines.push(format!("UNSHOWN ({})", unshown.len()));
            for message in &unshown {
                lines.push(format!("  {} {}  {}", message.id, message.thread, message.body));
                shown_msgs.push(message.id.clone());
            }
        }

        // The command that helps the seat move on.
        lines.push(String::new());
        lines.push(match seat.role {
            SeatRole::Captain => match shown_msgs.first() {
                Some(first) => match tx.get_message(&ctx.project, first)? {
                    Some(message) if crate::station::is_station(&message) => {
                        let milestone = message.thread.trim_start_matches("station:");
                        // The voyage's plan file, where the project keeps it
                        // (T-257): `gear/voyages/<M>/plan.md` by default.
                        let voyage_dir =
                            crate::tasks::conventions_of(tx, &ctx.project).voyage_dir;
                        let plan = gear_model::layout::plan_file(
                            &voyage_dir,
                            &ctx.project,
                            milestone,
                        );
                        format!("next: gear plan apply {plan} (# Decisions: station {first}: keep course|time-box|cut|move)")
                    }
                    // A review fail (or a failed merge) is settled where it
                    // can be: by a `# Verdicts` line, not by a reply (T-90).
                    Some(message) => match crate::tasks::verdict_hint(tx, &ctx.project, &message)? {
                        Some(hint) => format!("next: {}", verdict_command(&hint)),
                        None => format!("next: gear reply {first} --resolve -m \"<what was done>\""),
                    },
                    None => format!("next: gear reply {first} --resolve -m \"<what was done>\""),
                },
                None => "next: gear".to_string(),
            },
            _ => "next: gear send --kind progress".to_string(),
        });

        let view = ViewRecord {
            project: ctx.project.clone(),
            seat: seat_id,
            generation: ctx.generation,
            ts: now,
            shown_msgs,
            shown_ops: Vec::new(),
            shown_task,
            shown_attempt,
            shown_contract,
        };
        Ok((lines.join("\n"), view))
    })?;
    Ok(Home { text, view })
}

/// Persist the view. The CLI calls this **after** printing succeeded; if
/// printing fails the view is not recorded (design/13).
///
/// Recording a lane's view also **adopts** the contract version that page
/// showed, and only that one (T-8, design/06; review R1): a page read as v2
/// adopts v2 even when a newer update landed while it was being printed. A
/// page that was never printed adopts nothing, and a view of a handed-in
/// attempt adopts nothing (R2).
pub fn record_view(store: &mut Store, view: &ViewRecord) -> GearResult<()> {
    store.write(|tx| {
        tx.insert_view(view)?;
        if let (Some(task), Some(attempt), Some(shown)) =
            (&view.shown_task, view.shown_attempt, view.shown_contract)
        {
            crate::tasks::adopt_contract(tx, &view.project, task, attempt, shown, view.ts)?;
        }
        Ok(())
    })
}

/// The command that settles an item with a `# Verdicts` line (T-90): the
/// voyage's plan file (`hint.plan`, the project's own folder, T-257), then the
/// line with the real values.
///
/// A failed merge is settled by a `# Staff` line instead (B5): `- remerge
/// T-<n>` retries the merge, which is what a merge that failed needs — a
/// rework is the other way round.
pub fn verdict_command(hint: &crate::tasks::VerdictHint) -> String {
    // The voyage's plan file, where the project keeps it (T-257).
    let path = &hint.plan;
    match &hint.remerge {
        Some(task) => format!("gear plan apply {path} (# Staff: - remerge {task})"),
        None => format!("gear plan apply {path} (# Verdicts: {})", hint.line),
    }
}

/// What kind of plan line settles an item (B5): a `# Staff` line for a failed
/// merge, a `# Verdicts` line for the rest.
pub fn verdict_section(hint: &crate::tasks::VerdictHint) -> &'static str {
    if hint.remerge.is_some() {
        "# Staff"
    } else {
        "# Verdicts"
    }
}

/// The line the snapshot carries when the config file a project was registered
/// from no longer matches the config the project runs on (T-321): the drift
/// line when the text differs, the "cannot be read" line when the file is
/// gone. `None` when they agree, or when the store never recorded a path (a
/// project registered before T-321, or one registered without the CLI).
fn config_drift(tx: &Tx<'_>, project: &str) -> GearResult<Option<String>> {
    let Some(path) = tx.get_meta(&crate::admin::config_path_key(project))? else {
        return Ok(None);
    };
    let Some(row) = tx.get_project(project)? else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(text) if text == row.config_toml => Ok(None),
        Ok(_) => Ok(Some(format!(
            "! config drift: {path} differs from the applied config v{}: re-apply it with GEAR_ADMIN=1 gear admin init {path}, or restore the file",
            row.config_version
        ))),
        Err(_) => Ok(Some(format!(
            "! config file {path} cannot be read (config v{} still runs)",
            row.config_version
        ))),
    }
}

/// The most lanes the snapshot lists; the rest is one line of count, so the
/// snapshot stays under 15 lines.
const SNAPSHOT_LANES: usize = 10;

/// The most unconfirmed deliveries the snapshot names; the rest is one line of
/// count, so the snapshot stays short (T-271).
const SNAPSHOT_DELIVERIES: usize = 3;

/// The captain's snapshot: the totals, one line per lane (the task it is on
/// and how much is queued), the deliveries in flight and the temp slots.
///
/// The model is the board's ([`gear_board::project`]), so the two views of
/// the plan never disagree. Read-only, like the rest of the page.
fn snapshot(tx: &Tx<'_>, project: &str, now: Ts, agents: Option<&[gear_herdr::AgentInfo]>) -> GearResult<Vec<String>> {
    let plan = gear_board::project_with_agents(tx, project, now, agents)?;
    let mut lines = vec![format!(
        "SNAPSHOT {} tasks · {} done · {} in progress · {} waiting",
        plan.totals.tasks, plan.totals.done, plan.totals.in_progress, plan.totals.waiting
    )];
    // T-321: the config file the project was registered from, against the
    // config the project runs on — the second line, before the lanes.
    if let Some(drift) = config_drift(tx, project)? {
        lines.push(drift);
    }
    for lane in plan.lanes.iter().take(SNAPSHOT_LANES) {
        // The captain's tail turns off a lane's idle and stall warnings; the
        // snapshot says so, or a silenced lane reads as a quiet one (T-260).
        let mark = if crate::plan::lane_in_tail_at(tx, project, &lane.lane, now)? {
            " · tail: warnings off"
        } else {
            ""
        };
        lines.push(format!("  {}{mark}", gear_board::lane_line(lane)));
    }
    if plan.lanes.len() > SNAPSHOT_LANES {
        lines.push(format!("+{} more lanes", plan.lanes.len() - SNAPSHOT_LANES));
    }
    lines.push(format!(
        "deliveries not confirmed: {} · temps {} running · {} starting · {} blocked ({} of {})",
        plan.unconfirmed_deliveries,
        plan.temps.running,
        plan.temps.starting,
        plan.temps.blocked,
        plan.temps.in_use,
        plan.temps.quota
    ));
    // Which deliveries those are (T-271): the count alone made the captain ask
    // the store. The oldest first, three lines, the rest as a count.
    for item in plan.unconfirmed.iter().take(SNAPSHOT_DELIVERIES) {
        lines.push(format!(
            "    {} {} {} {}m",
            item.op,
            item.seat,
            item.task.as_deref().unwrap_or("-"),
            item.age_s / 60
        ));
    }
    if plan.unconfirmed.len() > SNAPSHOT_DELIVERIES {
        lines.push(format!("    +{} more", plan.unconfirmed.len() - SNAPSHOT_DELIVERIES));
    }
    // The voyages (T-22): the sentence each phase is judged by, its bar and its
    // planned landing, so the captain sees the bar before the inbox. Each
    // voyage is independent: the snapshot lists the ones with open work, and
    // when there is none, the one the board measures (T-246).
    let open: std::collections::BTreeSet<String> = tx
        .tasks_of_project(project)?
        .into_iter()
        .filter(|task| !task.state.is_terminal())
        .filter_map(|task| task.milestone)
        .collect();
    let mut voyages: Vec<_> = tx
        .voyages_of_project(project)?
        .into_iter()
        .filter(|voyage| {
            if open.is_empty() {
                // No open work: the voyage the board measures is the one that
                // matters.
                plan.workload.as_ref().is_some_and(|measured| measured.milestone == voyage.milestone)
            } else {
                open.contains(&voyage.milestone)
            }
        })
        .collect();
    // T-301: the earliest departure first, a voyage that has not departed
    // last; the id breaks the tie. An id is any word, so the id alone is no
    // order (`M10` would come before `M9`).
    voyages.sort_by(|left, right| {
        (left.departure.unwrap_or(i64::MAX), &left.milestone)
            .cmp(&(right.departure.unwrap_or(i64::MAX), &right.milestone))
    });
    for voyage in &voyages {
        lines.push(format!("  {}", crate::plan::voyage_line(voyage)));
    }
    Ok(lines)
}
