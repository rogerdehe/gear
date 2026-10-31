//! GEAR application layer: what each `gear` command does (design/03, /07, /13).
//!
//! Rules: commands only change data (one short transaction each, via
//! `gear_store::Store::write` + `run_request`). Herdr is touched read-only, and
//! only **before** the transaction (identity fingerprint). No side effects.
//!
//! Slice 1 (messaging loop): identity, mail::{send, reply, select_batch}, home.
//! Slice 2 adds plan/tasks/verdicts.

/// Scratch directories of the unit tests, removed when the test is done (T-412).
#[cfg(test)]
pub(crate) mod testtmp;

pub mod admin;
pub mod base;
pub mod captain;
pub mod charter;
pub mod config;
pub mod coverage;
pub mod decision;
pub mod draft;
pub mod friction;
pub mod github;
pub mod goal;
pub mod home;
pub mod identity;
pub mod kickoff;
pub mod mail;
pub mod plan;
pub mod queue;
pub mod review;
pub mod seats;
pub mod skill;
pub mod standard;
pub mod station;
pub mod tasks;
pub mod time;
pub mod voyage;

use gear_model::GearCtx;

/// What a `gear` process knows about where it runs. Built once from the
/// process environment by the CLI; constructed directly in tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    /// Parsed `GEAR_CTX`; None for the user's own shell or unmanaged agents.
    pub ctx: Option<GearCtx>,
    /// `HERDR_PANE_ID`, used only as an extra consistency check.
    pub pane_id: Option<String>,
    /// `GEAR_ADMIN=1` (user-only commands).
    pub admin: bool,
}
