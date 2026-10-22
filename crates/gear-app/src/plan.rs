//! `gear plan apply` (design/05 "Apply flow"), first cut: the `# Queue`
//! section. One transaction via `run_request`; an invalid draft writes
//! nothing. `# Staff`, `# Verdicts` and `# Review` come in later tasks and are
//! rejected here when non-empty (`BadFormat`, naming the section).

use crate::draft::Draft;
use crate::identity::Fingerprint;
use crate::Env;
use gear_model::{GearResult, Ts};
use gear_store::Store;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApplyResult {
    /// The new plan version (base + 1).
    pub version: i64,
    /// Ids assigned to `## NEW` blocks, in draft order.
    pub created: Vec<String>,
    /// Existing tasks whose contract got a new version: (id, new version).
    pub contract_changed: Vec<(String, i64)>,
    /// Non-terminal tasks the draft did not mention (kept, ranked after the listed ones).
    pub not_mentioned: Vec<String>,
}

/// Apply `draft` (parsed from `text`, which is stored verbatim as the plan row).
///
/// 1. identity: captain seat (`require_seat` with role Captain, states Active);
/// 2. `draft.project` = the caller's project, else `BadFormat`;
/// 3. `draft.base` ≠ current `plan_version` → `StaleVersion`;
/// 4. non-empty Staff/Verdicts/Review → `BadFormat` (not in this cut);
/// 5. validation, nothing written on failure (`BadFormat`, detail names the task/line):
///    - an existing id must exist; a `NEW` block needs `lane`, `goal` and `accept`;
///    - every dep names an existing task or a task of this draft; no cycle;
///    - `state=` only on existing tasks: `cancelled` from any non-terminal state,
///      `ready` only from Draft (DONE→READY with `reopen` comes with verdicts);
/// 6. writes: `NEW` → `T-<n>` via `next_id`, state Ready, contract v1; an existing
///    task whose goal/accept/evidence differs from its current contract → new
///    contract version; rank = position in the draft (1, 2, …), unmentioned
///    non-terminal tasks keep their relative order after them; lane, milestone,
///    required, priority and deps as written; `insert_plan(version = base + 1)`;
/// 7. replay: same `request_id` + same text → the stored result.
pub fn apply(
    store: &mut Store,
    env: &Env,
    fp: Option<&Fingerprint>,
    draft: &Draft,
    text: &str,
    request_id: &str,
    now: Ts,
) -> GearResult<ApplyResult> {
    todo!("T-36")
}
