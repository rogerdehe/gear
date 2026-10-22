//! `gear admin …` (design/01 registration, /13 ops commands). User only:
//! `Env.admin` must be true and `Env.ctx` None (an agent never runs these),
//! else `Forbidden`.

use crate::Env;
use gear_model::{GearResult, Ts};
use gear_store::Store;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitResult {
    pub project: String,
    pub config_version: i64,
    /// Seat ids registered by this init (captain first, then lanes in config order).
    pub seats: Vec<String>,
}

/// `gear admin init <config>` — first registration of a project (slice 3, first
/// cut; re-init with changes comes later):
/// 1. caller check (see module doc);
/// 2. `config::parse` (errors as is) and `config::validate` — any issue →
///    `BadFormat` whose detail lists every `field: problem`;
/// 3. project already in the store → `BadFormat` "already registered";
/// 4. one transaction: `insert_project` (config text verbatim, config_version 1,
///    quotas from `[quota]`, plan_version 0) and one seat per role —
///    `<id>:captain` (Captain, profile = `captain.profile`) and `<id>:<lane>`
///    for each lane (Lane, its profile) — all generation 0, state Queued,
///    `state_reason` "registered". Onboarding (worktree, pane, agent) is the
///    engine's job afterwards, not init's.
/// Checks that need Herdr or git (session, repo, branch) are not part of this cut.
pub fn init(store: &mut Store, env: &Env, config_text: &str, now: Ts) -> GearResult<InitResult> {
    let _ = (store, env, config_text, now);
    todo!("T-57")
}
