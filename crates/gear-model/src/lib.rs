//! GEAR state model (design/02, design/03).
//!
//! Everything here is pure: enums for every state machine, the legal
//! transitions between their values, the fixed error codes, and the caller
//! context that a GEAR-onboarded agent inherits through `GEAR_CTX`.
//!
//! Other crates must change any state only after `can_transition` /
//! `transition` accepts it. The transition tables use exhaustive `match`, so
//! adding a state forces every table to be revisited at compile time.

pub mod branch;
pub mod commit;
pub mod ctx;
pub mod error;
pub mod layout;
pub mod state;

pub use ctx::GearCtx;
pub use error::{ErrorCode, GearError, GearResult};
pub use state::{AttemptState, MsgKind, MsgState, OpKind, OpState, SeatRole, SeatState, TaskState};

/// Unix epoch seconds. All stored times use this; callers inject `now`.
pub type Ts = i64;
