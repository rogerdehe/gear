//! Doorbells (design/07). `decide` is pure: callers gather inputs first.

use gear_model::Ts;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorbellInput {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub pane_id: String,
    /// From observe: present, reachable state, idle with no Busy flag.
    pub idle: bool,
    /// Seat holds a delivery slot (Intended/Executing/Submitted/Unknown deliver/doorbell op).
    pub slot_busy: bool,
    /// When the last doorbell op for this seat was settled (Confirmed/Failed/Void), if any.
    pub last_settled_at: Option<Ts>,
    /// Open/due-Waiting message ids for this seat.
    pub actionable: Vec<String>,
    /// Reply/Instruction message ids not yet shown in any view of this generation.
    pub unshown: Vec<String>,
    /// How many of `actionable` are urgent.
    pub urgent: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoorbellAction {
    pub project: String,
    pub seat: String,
    pub generation: i64,
    pub pane_id: String,
    /// One line, no message bodies, e.g. "3 items waiting for you (1 urgent); run gear first".
    pub text: String,
    /// The message ids this doorbell is about (evidence target for confirmation).
    pub covers: Vec<String>,
}

/// Ring when: something to show (actionable or unshown), agent idle, no slot
/// held, and cooldown elapsed since the last settled doorbell
/// (`last_settled_at` is None, or `now - last_settled_at >= cooldown_s`).
///
/// One action per ringing input, in input order. `covers` = `actionable` in
/// order, then the `unshown` ids not already in it. Text, exactly:
/// `GEAR: <n> item(s) waiting for you. Run `gear` to see them.` where n =
/// covers.len(); when urgent > 0 the first sentence becomes
/// `GEAR: <n> item(s) waiting for you (<urgent> urgent).` Never message bodies.
pub fn decide(inputs: &[DoorbellInput], cooldown_s: i64, now: Ts) -> Vec<DoorbellAction> {
    inputs
        .iter()
        .filter(|input| rings(input, cooldown_s, now))
        .map(|input| {
            let covers = covered_ids(input);
            let n = covers.len();
            let urgent = if input.urgent > 0 {
                format!(" ({} urgent)", input.urgent)
            } else {
                String::new()
            };
            DoorbellAction {
                project: input.project.clone(),
                seat: input.seat.clone(),
                generation: input.generation,
                pane_id: input.pane_id.clone(),
                text: format!("GEAR: {n} item(s) waiting for you{urgent}. Run `gear` to see them."),
                covers,
            }
        })
        .collect()
}

/// Whether this seat may be rung right now.
fn rings(input: &DoorbellInput, cooldown_s: i64, now: Ts) -> bool {
    if input.actionable.is_empty() && input.unshown.is_empty() {
        return false; // nothing to show
    }
    if !input.idle {
        return false; // the agent is busy: a doorbell would interrupt a turn
    }
    if input.slot_busy {
        return false; // a delivery already occupies the seat
    }
    match input.last_settled_at {
        None => true,
        Some(settled) => now - settled >= cooldown_s,
    }
}

/// The message ids this doorbell is about: `actionable` in order, then the
/// `unshown` ids not already covered.
fn covered_ids(input: &DoorbellInput) -> Vec<String> {
    let mut covers = input.actionable.clone();
    for id in &input.unshown {
        if !covers.contains(id) {
            covers.push(id.clone());
        }
    }
    covers
}
