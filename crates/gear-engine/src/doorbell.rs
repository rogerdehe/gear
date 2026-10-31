//! Doorbells (design/07). `decide` is pure: callers gather inputs first.
//!
//! T-130: a message for a lane reaches it while it works. An urgent message,
//! and every captain instruction for a lane whose agent queues input typed
//! while it works (pi, claude), is typed at once — one doorbell per message,
//! and the text names the message ids. A working agent of a kind that does
//! not queue input (opencode, codex) still waits for idle, as before.

use gear_model::{MsgKind, Ts};

/// One waiting message a doorbell would name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorbellItem {
    /// `M-<n>`
    pub id: String,
    pub urgent: bool,
    /// Typed into the pane even while the agent works (T-130): urgent, or a
    /// captain instruction for an agent that queues input.
    pub at_once: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorbellInput {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub pane_id: String,
    /// From observe: present, reachable state, idle with no Busy flag.
    pub idle: bool,
    /// Live agent state; None means the seat is not reachable.
    pub status: Option<gear_herdr::AgentStatus>,
    /// Seat holds a delivery slot (Intended/Executing/Submitted/Unknown deliver/doorbell op).
    pub slot_busy: bool,
    /// When the last doorbell op for this seat was settled (Confirmed/Failed/Void), if any.
    pub last_settled_at: Option<Ts>,
    /// The seat has no open attempt (design §10.4): a lane in that state gets
    /// the text that tells it to stop working on earlier things.
    pub no_task: bool,
    /// This seat's waiting messages that no earlier doorbell named, in the
    /// order they are named: the actionable ones first, then the unshown
    /// replies/instructions. One doorbell per message, never repeated (T-130),
    /// is the caller's filter: an id covered by an earlier doorbell op is not
    /// in here.
    pub waiting: Vec<DoorbellItem>,
    /// The captain's voyage goal (user, 2026-10-28: the captain must never
    /// lose the destination): every captain doorbell repeats it. None for
    /// lanes and reviewers, and for a project with no departed voyage.
    pub goal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorbellAction {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub pane_id: String,
    /// One line, no message bodies, e.g.
    /// "GEAR: 2 item(s) waiting for you (1 urgent): M-3 M-7. Run the command
    /// `gear` (no arguments) in your own shell; it shows only your items."
    pub text: String,
    /// The message ids this doorbell is about (evidence target for confirmation).
    pub covers: Vec<String>,
}

/// Ring when: something new to show, no slot held, and either the message must
/// arrive while the seat works (T-130: [`DoorbellItem::at_once`]) or the agent
/// is idle and the cooldown since the last settled doorbell elapsed.
///
/// `at_once` bypasses the idle check and the cooldown: an urgent message (or a
/// captain instruction for a queueing agent) is typed at once, and the cooldown
/// is only there to keep ordinary reminders apart. The slot check is not
/// bypassed: a prompt in the same moment as a delivery is swallowed (T-71).
///
/// A working agent is told only about the messages that must reach it now; the
/// others wait for idle, as before. An idle agent is told about everything.
///
/// One action per ringing input, in input order. `covers` = the ids named, in
/// order. Text = [`text`]: the ids, and the exact command to run.
pub fn decide(inputs: &[DoorbellInput], cooldown_s: i64, now: Ts) -> Vec<DoorbellAction> {
    inputs
        .iter()
        .filter(|input| rings(input, cooldown_s, now))
        .map(|input| {
            let named = named(input);
            DoorbellAction {
                project: input.project.clone(),
                seat: input.seat.clone(),
                generation: input.generation,
                pane_id: input.pane_id.clone(),
                text: with_goal(text(&named, input.no_task), input.goal.as_deref()),
                covers: named.into_iter().map(|item| item.id).collect(),
            }
        })
        .collect()
}

/// Must this message be typed into the pane even while the agent works? (T-130)
///
/// An urgent message always (a codex or opencode agent takes it mid-turn too).
/// A captain instruction only when the agent queues input typed while it works
/// (pi, claude): it reads the text after the current step, so nothing is lost.
pub fn at_once(urgent: bool, kind: MsgKind, from_captain: bool, queues_input: bool) -> bool {
    urgent || (from_captain && kind == MsgKind::Instruction && queues_input)
}

/// [`at_once`] for one stored message and the seat it is addressed to: the seat
/// is a lane whose agent kind queues input, and `captain` is the project's
/// captain (so the message is a captain instruction). Used by the engine tick
/// and by `gear send` alike, so the two never drift apart.
pub fn reaches_while_working(
    message: &gear_store::MessageRow,
    seat: &gear_store::SeatRow,
    agent_kind: &str,
    captain: Option<&str>,
) -> bool {
    let queues_input =
        seat.role == gear_model::SeatRole::Lane && gear_herdr::queues_input(agent_kind);
    at_once(
        message.urgent,
        message.kind,
        captain == Some(message.sender.as_str()),
        queues_input,
    )
}

/// The one-line doorbell text.
///
/// With a task (`no_task` false):
/// `GEAR: <n> item(s) waiting for you[ (<urgent> urgent)]: <ids>. Run the command `gear` (no arguments) in your own shell; it shows only your items.`
///
/// Without a task (design §10.4 — a lane with no open attempt):
/// `GEAR: <n> item(s) for you: <ids>. Run the command `gear` (no arguments) in your own shell, answer them, then end your turn. You have no task now: do not continue earlier work.`
///
/// The command is always spelled out, because the backticks are lost as soon as
/// the text is pasted or shown: "Run gear now" reads as a command `gear now`
/// (T-248).
///
/// Never message bodies.
pub fn text(items: &[DoorbellItem], no_task: bool) -> String {
    let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
    if no_task {
        return format!(
            "GEAR: {} item(s) for you: {}. Run the command `gear` (no arguments) in your own shell, answer them, then end your turn. You have no task now: do not continue earlier work.",
            items.len(),
            ids.join(" ")
        );
    }
    let urgent = items.iter().filter(|item| item.urgent).count();
    let urgent = if urgent > 0 {
        format!(" ({urgent} urgent)")
    } else {
        String::new()
    };
    format!(
        "GEAR: {} item(s) waiting for you{urgent}: {}. Run the command `gear` (no arguments) in your own shell; it shows only your items.",
        items.len(),
        ids.join(" ")
    )
}

/// The doorbell text with the captain's goal after it, when there is one.
pub fn with_goal(text: String, goal: Option<&str>) -> String {
    match goal {
        Some(goal) => format!("{text}\n{goal}"),
        None => text,
    }
}

/// The line that reminds the captain of the voyage it sails (user,
/// 2026-10-28): the current voyage's done sentence and bar, as the user agreed
/// them, and where the voyage stands (T-337). None when the project has no
/// departed voyage or it is not described.
///
/// The text is the one every captain output uses
/// ([`gear_board::stations::goal_text`]), so the doorbell, the home page and
/// the station items never drift apart.
pub fn goal_line(tx: &gear_store::Tx<'_>, project: &str) -> gear_model::GearResult<Option<String>> {
    let milestone = gear_app::plan::current_voyage(tx, project)?;
    if milestone.is_empty() {
        return Ok(None);
    }
    gear_board::stations::goal_text(tx, project, &milestone)
}

/// Whether this seat may be rung right now.
fn rings(input: &DoorbellInput, cooldown_s: i64, now: Ts) -> bool {
    if input.waiting.is_empty() {
        return false; // nothing to show
    }
    if input.slot_busy {
        return false; // a delivery already occupies the seat
    }
    let at_once = input.waiting.iter().any(|item| item.at_once);
    match input.status {
        Some(gear_herdr::AgentStatus::Blocked) | None => return false,
        Some(gear_herdr::AgentStatus::Unknown) => {
            return input.waiting.iter().any(|item| item.urgent && item.at_once)
        }
        _ => {}
    }
    if !input.idle {
        // A working agent is only interrupted for a message that must reach
        // it now (T-130).
        return at_once;
    }
    if at_once {
        return true; // urgent even when idle: the cooldown is not a reason to wait
    }
    match input.last_settled_at {
        None => true,
        Some(settled) => now - settled >= cooldown_s,
    }
}

/// The messages this doorbell names: everything for an idle agent, only the
/// at-once ones for a working agent.
fn named(input: &DoorbellInput) -> Vec<DoorbellItem> {
    if input.idle {
        return input.waiting.clone();
    }
    input
        .waiting
        .iter()
        .filter(|item| {
            item.at_once && (input.status != Some(gear_herdr::AgentStatus::Unknown) || item.urgent)
        })
        .cloned()
        .collect()
}
