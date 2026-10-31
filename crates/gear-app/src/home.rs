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
    /// Every project's view, when the page showed several (T-486); one entry
    /// for a single-project page, equal to `view`.
    pub views: Vec<ViewRecord>,
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
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    now: Ts,
    agents: Option<&[gear_herdr::AgentInfo]>,
) -> GearResult<Home> {
    let (text, views) = store.read(|tx| {
        let (ctx, seat) = require_seat(tx, env, fp, None, None)?;
        let seat_id = seat.id.clone();

        // A captain whose pane holds more than one project (T-486): every
        // held project's voyage line and its inbox, urgent items of all
        // projects first, each item line naming its project. One project
        // keeps the page of design/13 exactly as it was.
        if seat.role == SeatRole::Captain {
            let pane = fp
                .map(|fp| fp.pane_id.clone())
                .or_else(|| env.pane_id.clone());
            let held: Vec<gear_store::SeatRow> = match pane {
                Some(pane) => crate::identity::captains_on_pane(tx, &pane)?,
                None => Vec::new(),
            };
            if held.len() > 1 {
                let (text, views) = multi_page(tx, env, &seat, &ctx, &seat_id, now, &held)?;
                return Ok((text, views));
            }
        }

        let mut lines: Vec<String> = Vec::new();
        if seat.role == SeatRole::Captain {
            lines.extend(
                gear_board::voyage::reports(tx, &ctx.project, now)?
                    .iter()
                    .flat_map(|v| v.lines()),
            );
            // The voyage's goal, bar and course (T-336): the same one text
            // every captain output uses, right under the voyage report lines.
            let milestone = crate::plan::current_voyage(tx, &ctx.project)?;
            if !milestone.is_empty() {
                if let Some(goal) = gear_board::stations::goal_text(tx, &ctx.project, &milestone)? {
                    lines.push(goal);
                }
            }
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
        // and a message no view ever listed stays unshown. Only this
        // project's views count (T-486): a bare id is not unique across
        // projects.
        let mut already_shown: Vec<String> = Vec::new();
        for view in tx.views_since(&seat_id, ctx.generation, 0)? {
            if view.project == ctx.project {
                already_shown.extend(view.shown_msgs);
            }
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
                // A round that could not continue the earlier conversation
                // because its CLI changed (T-481, design/10 §resume rules):
                // the seat sees why, and its packet names the session its
                // earlier work ran in.
                if tx
                    .session_recovery(&ctx.project, &attempt.task, attempt.n)?
                    .is_some()
                {
                    lines.push("round: new-session recovery".to_string());
                }
                shown_task = Some(attempt.task.clone());
                shown_attempt = Some(attempt.n);
                shown_contract = Some(version);
            }
        }

        if seat.role == SeatRole::Captain {
            // The inbox batch: thread, message ids, the original bodies.
            let batch = mail::select_batch(
                tx,
                &ctx.project,
                &seat_id,
                now,
                mail::BatchLimits::default(),
            )?;
            if !batch.is_empty() {
                lines.push(String::new());
                lines.push(format!("INBOX ({} threads)", batch.len()));
                for thread in &batch {
                    let ids: Vec<&str> = thread
                        .open
                        .iter()
                        .map(|message| message.id.as_str())
                        .collect();
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
                lines.push(format!(
                    "  {} {}  {}",
                    message.id, message.thread, message.body
                ));
                shown_msgs.push(message.id.clone());
            }
        }

        // The command that helps the seat move on.
        lines.push(String::new());
        let first = shown_msgs.first().map(|id| (id.as_str(), id.as_str()));
        lines.push(next_command(tx, &ctx.project, first)?);

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
        Ok((lines.join("\n"), vec![view]))
    })?;
    // The caller's own project's view (T-486 F2): a multi-project page has
    // one view per held project, and `Home.view` is the one of the project
    // the caller runs in, not the first held one.
    let caller = env.ctx.as_ref().map(|ctx| ctx.project.clone());
    let view = caller
        .as_ref()
        .and_then(|project| views.iter().find(|view| &view.project == project))
        .or_else(|| views.first())
        .cloned()
        .unwrap_or_else(|| ViewRecord {
            project: caller.unwrap_or_default(),
            seat: String::new(),
            generation: 0,
            ts: now,
            shown_msgs: Vec::new(),
            shown_ops: Vec::new(),
            shown_task: None,
            shown_attempt: None,
            shown_contract: None,
        });
    Ok(Home { text, view, views })
}

/// The page of a captain whose pane holds several projects (T-486): urgent
/// items of every project first, then each project's voyage line and the
/// rest of its inbox, every item line carrying the qualified id
/// (`<project>:M-<n>`) `gear reply` takes. One view per project, so each
/// project's messages count as shown only there.
fn multi_page(
    tx: &Tx<'_>,
    env: &Env,
    seat: &gear_store::SeatRow,
    ctx: &gear_model::GearCtx,
    seat_id: &str,
    now: Ts,
    held: &[gear_store::SeatRow],
) -> GearResult<(String, Vec<ViewRecord>)> {
    let _ = env;
    // Each held project's own captain seat: its messages are addressed to
    // `<project>:captain`, and its view is recorded under that seat.
    let mut held: Vec<&gear_store::SeatRow> = held.iter().collect();
    held.sort_by(|a, b| a.project.cmp(&b.project));
    held.dedup_by(|a, b| a.project == b.project);

    let mut already: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for held_seat in &held {
        let mut shown = Vec::new();
        for view in tx.views_since(&held_seat.id, held_seat.generation, 0)? {
            if view.project == held_seat.project {
                shown.extend(view.shown_msgs);
            }
        }
        already.insert(held_seat.project.clone(), shown);
    }

    let mut batches = Vec::new();
    for held_seat in &held {
        let batch = mail::select_batch(
            tx,
            &held_seat.project,
            &held_seat.id,
            now,
            mail::BatchLimits::default(),
        )?;
        batches.push((held_seat, batch));
    }

    let mut lines = vec![format!(
        "seat {seat_id} · {} · generation {}",
        seat.role.as_str(),
        ctx.generation
    )];
    let mut views: Vec<ViewRecord> = Vec::new();
    let mut first: Option<(String, String)> = None;

    // 1. Urgent items of all projects, before anything else.
    let mut urgent_lines: Vec<String> = Vec::new();
    let mut urgent_threads = 0;
    for (held_seat, batch) in &batches {
        let project = &held_seat.project;
        for thread in batch.iter().filter(|thread| thread.urgent) {
            urgent_threads += 1;
            let ids: Vec<String> = thread
                .open
                .iter()
                .map(|message| format!("{project}:{}", message.id))
                .collect();
            urgent_lines.push(format!("  {} [urgent] {}", thread.thread, ids.join(" ")));
            for message in &thread.open {
                urgent_lines.push(format!("    {}", message.body));
            }
            if thread.more > 0 {
                urgent_lines.push(format!("    +{} more", thread.more));
            }
            if first.is_none() {
                if let Some(message) = thread.open.first() {
                    first = Some((project.clone(), message.id.clone()));
                }
            }
        }
    }
    if urgent_threads > 0 {
        lines.push(String::new());
        lines.push(format!("URGENT ({urgent_threads} threads)"));
        lines.extend(urgent_lines);
    }

    // 2. Each held project: its voyage line and the rest of its inbox.
    for (held_seat, batch) in &batches {
        let project = &held_seat.project;
        let project_seat = &held_seat.id;
        let generation = held_seat.generation;
        let since = crate::seats::generation_since(tx, project_seat, generation)?;
        lines.push(String::new());
        lines.push(project.clone());
        for report in gear_board::voyage::reports(tx, project, now)? {
            lines.extend(report.lines());
        }
        let milestone = crate::plan::current_voyage(tx, project)?;
        if !milestone.is_empty() {
            if let Some(goal) = gear_board::stations::goal_text(tx, project, &milestone)? {
                lines.push(goal);
            }
        }

        let mut shown_msgs: Vec<String> = Vec::new();
        // Urgent items were printed above; their ids still count as shown.
        for thread in batch.iter().filter(|thread| thread.urgent) {
            for message in &thread.open {
                shown_msgs.push(message.id.clone());
            }
        }
        let normal: Vec<&mail::ThreadView> = batch.iter().filter(|thread| !thread.urgent).collect();
        if !normal.is_empty() {
            lines.push(format!("INBOX ({} threads)", normal.len()));
            for thread in normal {
                let ids: Vec<String> = thread
                    .open
                    .iter()
                    .map(|message| format!("{project}:{}", message.id))
                    .collect();
                lines.push(format!("  {} {}", thread.thread, ids.join(" ")));
                for message in &thread.open {
                    lines.push(format!("    {}", message.body));
                    shown_msgs.push(message.id.clone());
                }
                if thread.more > 0 {
                    lines.push(format!("    +{} more", thread.more));
                }
                if first.is_none() {
                    if let Some(message) = thread.open.first() {
                        first = Some((project.clone(), message.id.clone()));
                    }
                }
            }
        }

        // Replies and instructions this project's own views have not shown.
        let project_already = already.get(project).cloned().unwrap_or_default();
        let unshown: Vec<_> = tx
            .messages_to_since(project, project_seat, since)?
            .into_iter()
            .filter(|message| {
                !project_already.contains(&message.id)
                    && !shown_msgs.contains(&message.id)
                    && crate::seats::message_belongs(held_seat, message, since)
            })
            .collect();
        if !unshown.is_empty() {
            lines.push(format!("UNSHOWN ({})", unshown.len()));
            for message in &unshown {
                lines.push(format!(
                    "  {project}:{} {}  {}",
                    message.id, message.thread, message.body
                ));
                shown_msgs.push(message.id.clone());
                if first.is_none() {
                    first = Some((project.clone(), message.id.clone()));
                }
            }
        }

        views.push(ViewRecord {
            project: project.clone(),
            seat: project_seat.clone(),
            generation,
            ts: now,
            shown_msgs,
            shown_ops: Vec::new(),
            shown_task: None,
            shown_attempt: None,
            shown_contract: None,
        });
    }

    lines.push(String::new());
    lines.push(match &first {
        Some((project, id)) => {
            let shown = format!("{project}:{id}");
            next_command(tx, project, Some((id.as_str(), shown.as_str())))?
        }
        None => "next: gear".to_string(),
    });
    Ok((lines.join("\n"), views))
}

/// The command that helps the seat move on (design/13). `first` is the first
/// shown message: its bare id for the lookup, and the id the page printed
/// (qualified when the captain holds several projects).
fn next_command(tx: &Tx<'_>, project: &str, first: Option<(&str, &str)>) -> GearResult<String> {
    let Some((bare, shown)) = first else {
        return Ok("next: gear".to_string());
    };
    match tx.get_message(project, bare)? {
        Some(message) if crate::station::is_station(&message) => {
            let milestone = message.thread.trim_start_matches("station:");
            // The voyage's plan file, where the project keeps it
            // (T-257): `gear/voyages/<M>/plan.md` by default.
            let voyage_dir = crate::tasks::conventions_of(tx, project).voyage_dir;
            let plan = gear_model::layout::plan_file(&voyage_dir, project, milestone);
            Ok(format!(
                "next: gear plan apply {plan} (# Decisions: station {shown}: keep course|time-box|cut|move)"
            ))
        }
        // A review fail (or a failed merge) is settled where it can be:
        // by a `# Verdicts` line, not by a reply (T-90).
        Some(message) => match crate::tasks::verdict_hint(tx, project, &message)? {
            Some(hint) => Ok(format!("next: {}", verdict_command(&hint))),
            None => Ok(format!(
                "next: gear reply {shown} --resolve -m \"<what was done>\""
            )),
        },
        None => Ok(format!(
            "next: gear reply {shown} --resolve -m \"<what was done>\""
        )),
    }
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
fn snapshot(
    tx: &Tx<'_>,
    project: &str,
    now: Ts,
    agents: Option<&[gear_herdr::AgentInfo]>,
) -> GearResult<Vec<String>> {
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
        lines.push(format!(
            "    +{} more",
            plan.unconfirmed.len() - SNAPSHOT_DELIVERIES
        ));
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
                plan.workload
                    .as_ref()
                    .is_some_and(|measured| measured.milestone == voyage.milestone)
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
