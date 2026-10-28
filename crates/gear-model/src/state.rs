//! State machines and their legal transitions (design/02, /03, /07, /08).
//!
//! Stored as lowercase snake strings in SQLite via `as_str` / `FromStr`.

use crate::error::{ErrorCode, GearError, GearResult};
use std::str::FromStr;

macro_rules! string_enum {
    ($(#[$m:meta])* $name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($var),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }
        }

        impl FromStr for $name {
            type Err = GearError;
            fn from_str(s: &str) -> GearResult<Self> {
                match s {
                    $($s => Ok($name::$var),)+
                    _ => Err(GearError::new(
                        ErrorCode::BadFormat,
                        format!(concat!("unknown ", stringify!($name), ": {:?}"), s),
                    )),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(TaskState {
    Draft => "draft", Ready => "ready", Running => "running", Waiting => "waiting",
    Verifying => "verifying", Done => "done", Failed => "failed", Cancelled => "cancelled",
});

string_enum!(AttemptState {
    Delivered => "delivered", Started => "started", Stopped => "stopped",
    Submitted => "submitted", Accepted => "accepted", Rejected => "rejected",
    Abandoned => "abandoned", Unknown => "unknown",
});

string_enum!(SeatRole { Captain => "captain", Lane => "lane", Temp => "temp" });

string_enum!(SeatState {
    Starting => "starting", Active => "active", Standby => "standby", Parked => "parked",
    Held => "held", Retiring => "retiring", Retired => "retired", Missing => "missing",
    NeedsCheck => "needs_check", Unavailable => "unavailable", Handover => "handover",
    Queued => "queued", Releasing => "releasing", Released => "released", Unknown => "unknown",
});

string_enum!(MsgKind {
    Progress => "progress", Question => "question", Blocked => "blocked", Result => "result",
    Ack => "ack", Instruction => "instruction", Reply => "reply", System => "system",
    Review => "review",
});

string_enum!(MsgState { Open => "open", Waiting => "waiting", Resolved => "resolved", Info => "info" });

string_enum!(OpKind {
    Onboard => "onboard", Deliver => "deliver", Doorbell => "doorbell",
    Release => "release", Title => "title", Notify => "notify",
    // Merge a passed task's branches into the integration branch (T-12/T-13).
    Merge => "merge",
    // One call to the project's forge (T-91): the claim, a comment, a merge, a
    // close. The engine's, not a seat's: the payload says which call.
    Forge => "forge",
});

string_enum!(OpState {
    Intended => "intended", Executing => "executing", Submitted => "submitted",
    Confirmed => "confirmed", Failed => "failed", Unknown => "unknown", Void => "void",
});

impl TaskState {
    pub fn can_transition(self, to: TaskState) -> bool {
        use TaskState::*;
        match (self, to) {
            (Draft, Ready) => true,
            (Ready, Running) => true, // delivery confirmed
            (Running, Waiting | Verifying) => true,
            (Running, Ready) => true,   // preempt safe point checked
            (Waiting, Running) => true, // wait cleared: same attempt continues
            (Waiting, Ready) => true,   // old attempt settled, re-dispatch
            (Verifying, Done) => true,  // verdict pass
            (Verifying, Ready) => true, // verdict fail (rework)
            (Done, Ready) => true,      // explicit reopen with reason
            (Draft | Ready | Running | Waiting | Verifying, Cancelled | Failed) => true,
            _ => false,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Done | TaskState::Failed | TaskState::Cancelled
        )
    }
}

impl AttemptState {
    pub fn can_transition(self, to: AttemptState) -> bool {
        use AttemptState::*;
        match (self, to) {
            (Delivered, Started | Unknown) => true,
            (Started, Stopped | Submitted | Abandoned) => true,
            (Stopped, Started | Submitted | Abandoned) => true,
            (Submitted, Accepted | Rejected) => true,
            // UNKNOWN only leaves via evidence to a proven state; never straight to Abandoned.
            (Unknown, Started | Stopped | Submitted) => true,
            // A task cancelled from the plan (T-93) closes its open attempt: a
            // delivered prompt and a handed-in result are abandoned directly.
            (Delivered | Submitted, Abandoned) => true,
            _ => false,
        }
    }

    /// "In progress" attempts occupy the seat (design/06).
    pub fn occupies_seat(self) -> bool {
        use AttemptState::*;
        matches!(self, Delivered | Started | Stopped | Unknown)
    }

    pub fn is_terminal(self) -> bool {
        use AttemptState::*;
        matches!(self, Accepted | Rejected | Abandoned)
    }
}

impl SeatState {
    /// Seat transitions depend on the role (design/03, /10).
    pub fn can_transition(role: SeatRole, from: SeatState, to: SeatState) -> bool {
        use SeatState::*;
        match role {
            SeatRole::Captain => matches!(
                (from, to),
                (Queued, Starting | Active) // Active: the skeleton binds the user's existing captain pane at init
                    | (Starting, Active | Unknown)
                    | (Unknown, Active | Unavailable | Handover)
                    | (Active, Unavailable | Handover)
                    | (Unavailable, Handover)
                    | (Handover, Active)
            ),
            SeatRole::Lane => matches!(
                (from, to),
                (Queued, Starting | NeedsCheck)
                    | (Starting, Active | NeedsCheck | Unknown)
                    | (Unknown, Active | NeedsCheck)
                    | (Active, Standby | Parked | Held | Retiring | Missing | NeedsCheck)
                    | (Standby, Active | Parked | Held | Retiring | Missing | NeedsCheck)
                    | (Parked, Active | Standby | Held | Retiring | Missing | NeedsCheck)
                    | (Held, Active | Standby | Retiring | Missing | NeedsCheck)
                    | (Retiring, Retired | Missing)
                    | (Retired, Queued) // registered again: a new voyage reuses the name (T-54)
                    | (Missing, Starting | NeedsCheck | Retired)
                    // Retried by the captain (T-82): the engine onboards it again.
                    | (NeedsCheck, Queued | Active | Starting | Retired)
                    | (Unknown, Queued)
                    // Released by the captain (T-133): its agent continues (Active)
                    // or is gone and the engine onboards it again (Queued).
                    | (Held, Queued)
            ),
            SeatRole::Temp => matches!(
                (from, to),
                (Queued, Starting)
                    | (Starting, Active | Unknown | Queued)
                    | (Unknown, Active | Released | NeedsCheck)
                    | (Active, Releasing | Missing | NeedsCheck)
                    // Design §3.6: a reviewer that handed in its verdict
                    // waits for the next one: its agent stays alive, its pane
                    // stays open, and its job binding ends.
                    | (Active, Standby)
                    // The idle reviewers the rule releases (and the captain's
                    // `- release <reviewer>`) go on their way out.
                    | (Standby, Releasing)
                    // A reviewer that is still starting and whose job is gone
                    // is cancelled: its start op is voided, its pane closed,
                    // the seat free (B03-05).
                    | (Starting, Released)
                    | (Releasing, Released | Unknown)
                    | (Released, Starting | Retired)
                    | (Missing, Starting | NeedsCheck | Retired)
                    // Retried by the captain (T-82): the engine onboards it again.
                    | (NeedsCheck, Queued | Active | Starting | Retired)
                    // T-259: a reviewer parked in NeedsCheck with no job whose
                    // agent is gone holds a slot no human can bring back: the
                    // engine frees it.
                    | (NeedsCheck, Released)
                    | (Unknown, Queued)
            ),
        }
    }

    /// States in which an agent is expected to be present in the pane (design/08).
    pub fn expects_agent(self) -> bool {
        use SeatState::*;
        matches!(
            self,
            Active | Standby | Parked | Held | Retiring | Unavailable | Handover
        )
    }
}

impl MsgState {
    pub fn can_transition(self, to: MsgState) -> bool {
        use MsgState::*;
        matches!(
            (self, to),
            (Open, Waiting | Resolved) | (Waiting, Open | Resolved) | (Info, Resolved)
        )
    }
}

impl OpState {
    pub fn can_transition(self, to: OpState) -> bool {
        use OpState::*;
        matches!(
            (self, to),
            (Intended, Executing | Void)
                | (Executing, Submitted | Failed | Unknown)
                // T-149 (design §10.1): an op whose text was never typed — the
                // agent had lost its name (it was renamed) or the pane could
                // not be read — goes back to Intended: the next tick tries it
                // again, and nothing is restarted.
                | (Executing, Intended)
                | (Submitted, Confirmed | Failed)
                | (Unknown, Confirmed | Failed)
        )
    }

    /// Ops that hold the seat's single delivery slot (design/07, /09).
    pub fn holds_delivery_slot(self) -> bool {
        use OpState::*;
        matches!(self, Intended | Executing | Submitted | Unknown)
    }
}

/// Return `BadTransition` unless `ok`. Helper for callers of `can_transition`.
pub fn require_transition(ok: bool, what: &str, from: &str, to: &str) -> GearResult<()> {
    if ok {
        Ok(())
    } else {
        Err(GearError::new(
            ErrorCode::BadTransition,
            format!("{what}: {from} -> {to}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_roundtrip() {
        for s in TaskState::ALL {
            assert_eq!(s.as_str().parse::<TaskState>().unwrap(), *s);
        }
        for s in SeatState::ALL {
            assert_eq!(s.as_str().parse::<SeatState>().unwrap(), *s);
        }
        for s in OpState::ALL {
            assert_eq!(s.as_str().parse::<OpState>().unwrap(), *s);
        }
        assert!("bogus".parse::<MsgKind>().is_err());
    }

    #[test]
    fn done_only_via_verdict_or_reopen() {
        for from in TaskState::ALL {
            let ok = from.can_transition(TaskState::Done);
            assert_eq!(ok, *from == TaskState::Verifying, "{from} -> done");
        }
        assert!(TaskState::Done.can_transition(TaskState::Ready));
        assert!(!TaskState::Done.can_transition(TaskState::Running));
    }

    #[test]
    fn unknown_attempt_never_abandoned_directly() {
        assert!(!AttemptState::Unknown.can_transition(AttemptState::Abandoned));
        assert!(AttemptState::Unknown.occupies_seat());
        assert!(!AttemptState::Submitted.occupies_seat());
    }

    #[test]
    fn captain_never_auto_recovers() {
        use SeatState::*;
        assert!(!SeatState::can_transition(
            SeatRole::Captain,
            Unavailable,
            Active
        ));
        assert!(SeatState::can_transition(
            SeatRole::Captain,
            Unavailable,
            Handover
        ));
        assert!(!SeatState::can_transition(
            SeatRole::Captain,
            Active,
            Missing
        ));
    }

    #[test]
    fn released_temp_is_not_missing() {
        assert!(!SeatState::Released.expects_agent());
        assert!(!SeatState::Queued.expects_agent());
        assert!(SeatState::Active.expects_agent());
    }

    /// T-54: a retired lane may be registered again — a new voyage reuses the
    /// name — and the engine onboards it once more.
    #[test]
    fn a_retired_lane_may_be_registered_again() {
        use SeatState::*;
        assert!(SeatState::can_transition(SeatRole::Lane, Retired, Queued));
        assert!(SeatState::can_transition(SeatRole::Lane, Queued, Starting));
        // A temp seat is not a lane: it keeps its own rules.
        assert!(!SeatState::can_transition(SeatRole::Temp, Retired, Queued));
    }

    /// T-259: a reviewer parked in NeedsCheck with no job whose agent is gone
    /// is released by the engine — its slot is free again.
    #[test]
    fn a_needs_check_temp_may_be_released() {
        use SeatState::*;
        assert!(SeatState::can_transition(
            SeatRole::Temp,
            NeedsCheck,
            Released
        ));
        // A lane's NeedsCheck is a human's to retry: it is not released by the
        // engine on its own.
        assert!(!SeatState::can_transition(
            SeatRole::Lane,
            NeedsCheck,
            Released
        ));
    }

    #[test]
    fn delivery_slot() {
        assert!(OpState::Submitted.holds_delivery_slot());
        assert!(OpState::Unknown.holds_delivery_slot());
        assert!(!OpState::Confirmed.holds_delivery_slot());
        assert!(
            !OpState::Confirmed.can_transition(OpState::Failed),
            "a settled op stays settled"
        );
        // T-97: a delivery that is never confirmed fails past its check
        // budget, which frees the seat's slot — the one way a submitted op
        // ends without being confirmed.
        assert!(OpState::Submitted.can_transition(OpState::Failed));
        assert!(!OpState::Failed.holds_delivery_slot());
    }
}
