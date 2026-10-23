//! Changing an agent's model **in place** (T-33): `/model`, the picker filter,
//! Enter, the variant dialog when it opens, then the status bar must show the
//! wanted model. No exit, no restart — the conversation stays.
//!
//! Used right after an OpenCode agent starts (its status bar must be on the
//! chain's model) and by the quota fallback (T-15). The engine may restart in
//! the middle of a switch: every call reads the status bar first and does
//! nothing when the model is already there; the leading `esc` closes whatever
//! the last attempt left open, so a half-finished switch does not pile up
//! dialogs.
//!
//! Never types into a working agent: a busy agent is `Waiting`, not switched.

use gear_app::config::ModelSwitch;
use gear_herdr::{HerdrAct, Outcome};
use gear_model::{GearResult, Ts};
use gear_store::{Store, Tx};

use crate::observe::SeatObs;

/// What a switch attempt did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// The status bar already shows the model: nothing was sent.
    Already,
    /// The sequence was sent and the status bar now shows the model.
    Done,
    /// The agent is working: never type into it; the next tick tries again.
    Waiting,
    /// The picker, the variant or the bar did not confirm: retried once, then
    /// the seat is flagged instead of looping.
    Failed,
}

/// The keys the switch sends, from the profile and the config: a key sequence
/// the FakeHerdr can check step by step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchSteps {
    /// The key that closes a stray dialog or quota toast (the profile's
    /// `switch_escape`, else `esc`).
    pub escape: String,
    /// The picker command (the profile's `switch_command`, else `/model`).
    pub command: String,
    /// The default answer to a variant dialog.
    pub variant: String,
}

impl SwitchSteps {
    /// The steps of one CLI kind: OpenCode's `/model` picker (SOP
    /// `agent:opencode`, ops/bin/switch-model.sh). A kind without a known
    /// picker is not switched in place — the caller falls back to its own path.
    pub fn for_kind(kind: &str, labels: &ModelSwitch) -> Option<SwitchSteps> {
        let (escape, command) = match kind {
            "opencode" | "codex" | "claude" => ("esc", "/model"),
            _ => return None,
        };
        Some(SwitchSteps {
            escape: escape.to_string(),
            command: command.to_string(),
            variant: labels.variant.clone().unwrap_or_else(|| "high".to_string()),
        })
    }
}

/// The status bar of an OpenCode pane, as far as the switch needs it: the
/// visible tail lines that carry the model.
fn shows_model(tail: &str, bar: &str) -> bool {
    tail.lines().any(|line| line.contains(bar))
}

/// Switch `seat`'s agent from `from` to `to` in place.
///
/// `obs` is this tick's observation (idle or not); `labels` are the picker and
/// bar texts from the config. The attempt counter is durable: a second failed
/// try returns `Failed` and the caller flags the seat.
pub fn switch_in_place(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat_id: &str,
    profile: &gear_herdr::Profile,
    obs: Option<&SeatObs>,
    labels: &ModelSwitch,
    now: Ts,
) -> GearResult<SwitchOutcome> {
    // 1. never type into a working agent (design/09): the switch waits.
    if obs.map(|obs| !obs.idle).unwrap_or(true) {
        return Ok(SwitchOutcome::Waiting);
    }
    let Some(pane) = pane_of(store, seat_id)? else {
        return Ok(SwitchOutcome::Failed);
    };
    let Some(steps) = SwitchSteps::for_kind(&profile.kind, labels) else {
        return Ok(SwitchOutcome::Failed);
    };

    // 2. read the status bar first: a switch that already took effect (or a
    //    restart in the middle of one) sends nothing.
    let Ok(tail) = herdr.read_tail(&pane, profile.tail_lines) else {
        return Ok(SwitchOutcome::Waiting);
    };
    if shows_model(&tail, &labels.bar) {
        clear_attempts(store, seat_id)?;
        return Ok(SwitchOutcome::Already);
    }

    // 3. the sequence: esc, /model, Enter, the filter, Enter, the variant when
    //    the dialog opens, Enter.
    let keys = |herdr: &mut dyn HerdrAct, key: &str| matches!(herdr.send_keys(&pane, &[key.to_string()]), Outcome::Ok(()));
    let text = |herdr: &mut dyn HerdrAct, text: &str| matches!(herdr.pane_run(&pane, text), Outcome::Ok(()));
    let ok = keys(herdr, &steps.escape)
        && text(herdr, &steps.command)
        && keys(herdr, "enter")
        && text(herdr, &labels.picker)
        && keys(herdr, "enter");
    if !ok {
        return failed(store, seat_id, now);
    }
    // The variant dialog opens after the filter is picked (L-30).
    if let Ok(after) = herdr.read_tail(&pane, profile.tail_lines) {
        if after.contains("Select variant") && !(text(herdr, &steps.variant) && keys(herdr, "enter")) {
            return failed(store, seat_id, now);
        }
    }

    // 4. the status bar decides.
    let confirmed = herdr
        .read_tail(&pane, profile.tail_lines)
        .map(|tail| shows_model(&tail, &labels.bar))
        .unwrap_or(false);
    if confirmed {
        clear_attempts(store, seat_id)?;
        return Ok(SwitchOutcome::Done);
    }
    failed(store, seat_id, now)
}

/// One failed attempt: retry once, then `Failed` (the caller flags the seat).
fn failed(store: &mut Store, seat_id: &str, _now: Ts) -> GearResult<SwitchOutcome> {
    let attempts = bump_attempts(store, seat_id)?;
    Ok(if attempts >= 2 { SwitchOutcome::Failed } else { SwitchOutcome::Waiting })
}

/// The pane of the seat's active binding.
fn pane_of(store: &mut Store, seat_id: &str) -> GearResult<Option<String>> {
    let seat = store.read(|tx| tx.get_seat(seat_id))?;
    let Some(seat) = seat else { return Ok(None) };
    let binding = store.read(|tx| tx.get_binding(seat_id, seat.generation))?;
    Ok(binding.map(|binding| binding.pane_id))
}

fn attempts_key(seat_id: &str) -> String {
    format!("engine.switch_attempts.{seat_id}")
}

fn bump_attempts(store: &mut Store, seat_id: &str) -> GearResult<i64> {
    store.write(|tx: &Tx<'_>| {
        let key = attempts_key(seat_id);
        let n = tx
            .get_meta(&key)?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0)
            + 1;
        tx.set_meta(&key, &n.to_string())?;
        Ok(n)
    })
}

fn clear_attempts(store: &mut Store, seat_id: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(&attempts_key(seat_id), "0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{Presence, SeatObs};
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::SeatRole;
    use gear_store::testing;
    use std::collections::BTreeSet;

    const T0: i64 = 1_800_000_000;

    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p1", "opencode", T0)
        })
        .unwrap();
        s
    }

    fn labels() -> ModelSwitch {
        ModelSwitch {
            picker: "DeepSeek V4.1 Flash OpenCode Go".into(),
            bar: "DeepSeek V4.1 Flash OpenCode Go".into(),
            variant: None,
        }
    }

    fn idle() -> SeatObs {
        SeatObs {
            seat: gear_store::SeatRow {
                id: "p:heat".into(),
                project: "p".into(),
                role: SeatRole::Lane,
                name: "heat".into(),
                profile: "opencode".into(),
                generation: 1,
                state: gear_model::SeatState::Active,
                state_since: T0,
                state_reason: None,
                task_id: None,
            },
            presence: Presence::Present,
            status: Some(gear_herdr::AgentStatus::Idle),
            flags: BTreeSet::new(),
            idle: true,
        }
    }

    fn texts(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneRun { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect()
    }

    fn keys(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::SendKeys { keys, .. } => keys.first().cloned(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_wrong_model_is_switched_with_the_key_sequence() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        let profile = gear_herdr::builtin("opencode").unwrap();
        // The status bar starts on another model.
        herdr.set_screen("w1:p1", "Build auto · MiniMax-M3.1-Flash MiniMax · high");

        let outcome = switch_in_place(&mut s, &mut herdr, "p:heat", &profile, Some(&idle()), &labels(), T0).unwrap();
        // The bar is not checked against the wanted model by the fake's screen
        // (the switch cannot confirm), so the first try is a retry.
        assert_eq!(outcome, SwitchOutcome::Waiting);
        assert_eq!(keys(&herdr), vec!["esc", "enter", "enter"], "{:?}", keys(&herdr));
        assert_eq!(
            texts(&herdr),
            vec!["/model", "DeepSeek V4.1 Flash OpenCode Go"],
            "{:?}",
            texts(&herdr)
        );
        assert!(herdr.calls.iter().all(|call| !matches!(call, Call::AgentStart { .. })), "no restart");
    }

    #[test]
    fn an_already_correct_model_sends_nothing() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        let profile = gear_herdr::builtin("opencode").unwrap();
        herdr.set_screen("w1:p1", "Build auto · DeepSeek V4.1 Flash OpenCode Go · high");

        let outcome = switch_in_place(&mut s, &mut herdr, "p:heat", &profile, Some(&idle()), &labels(), T0).unwrap();
        assert_eq!(outcome, SwitchOutcome::Already);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }

    #[test]
    fn a_working_agent_is_never_typed_into() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Working);
        let profile = gear_herdr::builtin("opencode").unwrap();
        let mut busy = idle();
        busy.idle = false;
        busy.status = Some(gear_herdr::AgentStatus::Working);

        let outcome = switch_in_place(&mut s, &mut herdr, "p:heat", &profile, Some(&busy), &labels(), T0).unwrap();
        assert_eq!(outcome, SwitchOutcome::Waiting);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }

    #[test]
    fn a_variant_dialog_is_answered() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        let profile = gear_herdr::builtin("opencode").unwrap();
        herdr.set_screen("w1:p1", "Build auto · MiniMax-M3.1-Flash MiniMax · high\nSelect variant");

        // The fake's screen does not change on input, but the dialog is open:
        // the variant is typed, and the bar check fails (no confirmation).
        let outcome = switch_in_place(&mut s, &mut herdr, "p:heat", &profile, Some(&idle()), &labels(), T0).unwrap();
        assert_eq!(outcome, SwitchOutcome::Waiting);
        let sent = texts(&herdr);
        assert!(sent.contains(&"high".to_string()), "{sent:?}");
    }

    #[test]
    fn two_failed_tries_flag_the_seat() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent("w1:p1", "opencode", gear_herdr::AgentStatus::Idle);
        let profile = gear_herdr::builtin("opencode").unwrap();
        herdr.set_screen("w1:p1", "Build auto · MiniMax-M3.1-Flash MiniMax · high");

        assert_eq!(
            switch_in_place(&mut s, &mut herdr, "p:heat", &profile, Some(&idle()), &labels(), T0).unwrap(),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            switch_in_place(&mut s, &mut herdr, "p:heat", &profile, Some(&idle()), &labels(), T0 + 10).unwrap(),
            SwitchOutcome::Failed
        );
    }
}
