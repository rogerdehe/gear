//! The overrun question (design §5.2).
//!
//! A round that passes its budget asks the captain whether it goes on. The
//! question, the answer and the round's clock belong to the task that fills
//! this module; until it lands, the tick does nothing — no round is asked and
//! no message is written.

use gear_herdr::HerdrAct;
use gear_model::{GearResult, Ts};
use gear_store::Store;

use crate::EngineCfg;

/// The overrun question of every project (design §5.2).
///
/// Empty until the task that implements the question and the clock lands.
pub fn tick(
    _store: &mut Store,
    _herdr: &mut dyn HerdrAct,
    _cfg: &EngineCfg,
    _now: Ts,
) -> GearResult<()> {
    Ok(())
}
