//! Fixed error codes (design/02). CLI prints `ERROR <CODE>: <detail>`.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// Caller has no valid seat/binding (or no GEAR_CTX for an agent call).
    NotRegistered,
    /// Caller's generation is not the seat's current active generation.
    StaleGeneration,
    /// Draft/plan base version is out of date.
    StaleVersion,
    /// Target seat / message / task does not exist or cannot receive.
    NoTarget,
    /// Caller role/state may not perform this action.
    Forbidden,
    /// Input does not match the strict format.
    BadFormat,
    /// Requested state change is not a legal transition.
    BadTransition,
    /// SQLite lock timeout; nothing was written, safe to retry.
    Locked,
    /// The store's schema version is newer than this binary's (T-41): the
    /// schema only migrates up, so the store is refused for writing.
    SchemaTooNew,
    /// Same request_id reused with different arguments.
    RequestConflict,
    /// Quota exhausted (item queued, not rejected, where the caller is a plan).
    Quota,
    /// Outcome unknown (exit code 3). Retry with the same request_id.
    Unconfirmed,
    /// Engine heartbeat stale. Informational: writes still succeed.
    EngineDown,
    /// A round passed its budget and the hand-in carries no friction report
    /// (design §6.2).
    FrictionRequired,
    /// A departed voyage's staffing (`quota`, `lanes`, `temps`, `models`) may
    /// not change at a re-init without the user's own words (T-392).
    StaffLocked,
    /// A departed voyage's goal (`title`, `done`, `bar`, stations) belongs to
    /// the user and no apply may change it (T-331).
    GoalLocked,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotRegistered => "NOT_REGISTERED",
            ErrorCode::StaleGeneration => "STALE_GENERATION",
            ErrorCode::StaleVersion => "STALE_VERSION",
            ErrorCode::NoTarget => "NO_TARGET",
            ErrorCode::Forbidden => "FORBIDDEN",
            ErrorCode::BadFormat => "BAD_FORMAT",
            ErrorCode::BadTransition => "BAD_TRANSITION",
            ErrorCode::Locked => "LOCKED",
            ErrorCode::SchemaTooNew => "SCHEMA_TOO_NEW",
            ErrorCode::RequestConflict => "REQUEST_CONFLICT",
            ErrorCode::Quota => "QUOTA",
            ErrorCode::Unconfirmed => "UNCONFIRMED",
            ErrorCode::EngineDown => "ENGINE_DOWN",
            ErrorCode::FrictionRequired => "FRICTION_REQUIRED",
            ErrorCode::StaffLocked => "STAFF_LOCKED",
            ErrorCode::GoalLocked => "GOAL_LOCKED",
        }
    }

    /// Process exit code used by the CLI.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Unconfirmed => 3,
            _ => 1,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct GearError {
    pub code: ErrorCode,
    pub detail: String,
}

impl GearError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        GearError { code, detail: detail.into() }
    }
}

pub type GearResult<T> = Result<T, GearError>;
