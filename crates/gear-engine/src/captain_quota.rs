//! Captain quota: drain current work, wake once, then wait for a fresh view.

use std::collections::BTreeMap;

use gear_herdr::{HerdrAct, Profile};
use gear_model::{GearResult, OpKind, OpState, SeatRole, Ts};
use gear_store::{CaptainQuota, NewOp, Store};

use crate::observe::{Presence, SeatObs};

pub const WAKE_TEXT: &str = "Your quota is back — run gear.";

pub fn update(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    profiles: &BTreeMap<String, Profile>,
    project: &str,
    observed: &[SeatObs],
    now: Ts,
) -> GearResult<()> {
    let Some(obs) = observed
        .iter()
        .find(|obs| obs.seat.role == SeatRole::Captain)
    else {
        return Ok(());
    };
    let captain = &obs.seat;
    let counter = format!(
        "engine.captain_quota_ticks.{}.{}",
        captain.id, captain.generation
    );
    let mut state = store.read(|tx| tx.captain_quota(project))?;
    if state
        .as_ref()
        .is_some_and(|state| state.generation != captain.generation)
    {
        store.write(|tx| tx.set_captain_quota(project, None))?;
        state = None;
    }
    if let Some(mut paused) = state.clone().filter(|state| !state.resumed) {
        if let Some(id) = paused.wake_op.as_deref() {
            let op = store.read(|tx| tx.get_op(project, id))?;
            if op.as_ref().is_some_and(|op| op.state == OpState::Failed) {
                // A refused prompt is proven not to have run. Unknown effects
                // are never resent; they wait for the captain's view.
                paused.wake_op = None;
            } else if op.as_ref().is_some_and(|op| {
                matches!(
                    op.state,
                    OpState::Submitted | OpState::Unknown | OpState::Confirmed
                )
            }) {
                let views = store.read(|tx| tx.views_since(&captain.id, captain.generation, 0))?;
                if views
                    .iter()
                    .skip(paused.views_before_wake)
                    .any(|view| view.ts >= paused.reset_at)
                {
                    paused.resumed = true;
                    store.write(|tx| {
                        tx.set_captain_quota(project, Some(&paused))?;
                        tx.set_meta(&counter, "0")
                    })?;
                    return Ok(());
                }
            }
        }
        if now >= paused.reset_at
            && paused.wake_op.is_none()
            && obs.presence == Presence::Present
            && obs.idle
            && crate::observe::reachable(captain.state)
        {
            // Intent and view baseline are atomic with the episode. Recovery
            // finds this op rather than creating another wake-up.
            store.write(|tx| {
                paused.views_before_wake =
                    tx.views_since(&captain.id, captain.generation, 0)?.len();
                let op = tx.insert_op(&NewOp {
                    project: project.into(),
                    seat: captain.id.clone(),
                    generation: captain.generation,
                    kind: OpKind::Doorbell,
                    task: None,
                    attempt: None,
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({
                        "text": WAKE_TEXT, "covers": [], "captain_quota_wake": true,
                        "views_before": paused.views_before_wake,
                    }),
                    created: now,
                })?;
                paused.wake_op = Some(op.id);
                tx.set_captain_quota(project, Some(&paused))
            })?;
        } else {
            store.write(|tx| tx.set_captain_quota(project, Some(&paused)))?;
        }
        return Ok(());
    }

    let Some(profile) = profiles.get(&captain.profile) else {
        return Ok(());
    };
    let Some(bar) = &profile.quota_bar else {
        return Ok(());
    };
    if obs.presence != Presence::Present {
        return Ok(());
    }
    let binding = store.read(|tx| tx.get_binding(&captain.id, captain.generation))?;
    let Some(binding) = binding else {
        return Ok(());
    };
    // Reading the footer is safe even while the captain is working. It does
    // not inspect the conversation or type anything into that agent.
    let reading = herdr
        .read_screen(&binding.pane_id)
        .ok()
        .and_then(|screen| bar.read(&screen));
    let Some(reading) = reading else {
        store.write(|tx| tx.set_meta(&counter, "0"))?;
        return Ok(());
    };
    if !reading.exhausted {
        store.write(|tx| {
            tx.set_meta(&counter, "0")?;
            tx.set_captain_quota(project, None)
        })?;
        return Ok(());
    }
    // The old 100% bar can remain after the captain acknowledges the reset.
    // Do not freeze again on that stale reading; a healthy bar or a different
    // window/reset signature starts the next episode.
    if state
        .as_ref()
        .is_some_and(|state| state.resumed && state.bar == reading.signature)
    {
        return Ok(());
    }
    let count = store.write(|tx| {
        let count = tx
            .get_meta(&counter)?
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        tx.set_meta(&counter, &count.to_string())?;
        Ok(count)
    })?;
    if count < profile.confirm_rounds.max(1) {
        return Ok(());
    }
    let config = gear_app::config::latest(store, project)?;
    let fallback = now + i64::from(config.models.quota_reset_h.max(1)) * 3600;
    let reset_at = reading
        .resets
        .iter()
        .map(|hint| {
            hint.as_deref()
                .and_then(|hint| reset_time(hint, now))
                .unwrap_or(fallback)
        })
        .max()
        .unwrap_or(fallback);
    store.write(|tx| {
        tx.set_captain_quota(
            project,
            Some(&CaptainQuota {
                generation: captain.generation,
                detected_at: now,
                reset_at,
                bar: reading.signature,
                wake_op: None,
                views_before_wake: 0,
                resumed: false,
            }),
        )
    })
}

/// The bar is rendered with the machine's local `date`, so use the same local
/// calendar for HH:MM hints. Date-only/invalid hints use the policy
/// fallback. No subprocess or global TZ mutation is needed.
fn reset_time(hint: &str, now: Ts) -> Option<Ts> {
    let stamp = now as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers address valid storage; localtime_r initializes tm
    // on success and returns null on failure.
    if unsafe { libc::localtime_r(&stamp, local.as_mut_ptr()) }.is_null() {
        return None;
    }
    let mut target = unsafe { local.assume_init() };
    target.tm_sec = 0;
    target.tm_isdst = -1;
    if let Some((hour, minute)) = hint.split_once(':') {
        let hour = hour.parse::<i32>().ok().filter(|h| (0..24).contains(h))?;
        let minute = minute.parse::<i32>().ok().filter(|m| (0..60).contains(m))?;
        target.tm_hour = hour;
        target.tm_min = minute;
        let mut time = unsafe { libc::mktime(&mut target) } as Ts;
        if time <= now {
            target.tm_mday += 1;
            target.tm_isdst = -1;
            time = unsafe { libc::mktime(&mut target) } as Ts;
        }
        return (time > now).then_some(time);
    }
    // A date-only weekly label does not expose an exact reset time.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_reset_hints_use_the_policy_fallback() {
        for hint in ["", "99:10", "00:99", "Jan99", "Feb31", "Oct23", "tomorrow"] {
            assert_eq!(reset_time(hint, 1_800_000_000), None, "{hint}");
        }
    }
    #[test]
    fn a_clock_hint_rolls_over_local_midnight() {
        let mut local: libc::tm = unsafe { std::mem::zeroed() };
        local.tm_year = 126;
        local.tm_mon = 0;
        local.tm_mday = 10;
        local.tm_hour = 23;
        local.tm_min = 55;
        local.tm_isdst = -1;
        let now = unsafe { libc::mktime(&mut local) } as Ts;
        assert_eq!(reset_time("00:10", now), Some(now + 15 * 60));
    }
}
