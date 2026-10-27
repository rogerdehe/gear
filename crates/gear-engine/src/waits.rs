//! The declared waits (design §5.3).
//!
//! A task can wait for another one ("wait for T-9 to merge"). The wait, the
//! captain's item and the round's paused clock belong to the task that fills
//! this module; until it lands, the tick does nothing — no task is parked and
//! no message is written.

use gear_herdr::HerdrAct;
use gear_model::{GearResult, Ts};
use gear_store::Store;

use crate::EngineCfg;

/// The declared waits of every project (design §5.3).
///
/// Empty until the task that implements the wait lands.
pub fn tick(
    _store: &mut Store,
    _herdr: &mut dyn HerdrAct,
    _cfg: &EngineCfg,
    _now: Ts,
) -> GearResult<()> {
    Ok(())
}
