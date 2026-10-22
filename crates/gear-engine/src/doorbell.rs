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
    todo!("slice1")
}
