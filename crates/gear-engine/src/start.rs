//! A start that a busy shell refused (T-95).
//!
//! A fresh worktree's shell may still be running the environment line when the
//! agent start comes (a slow shell init — nvm, a plugin manager): Herdr then
//! refuses with `agent_pane_busy`. That is "not yet", not a failure. The seat
//! keeps its start pending in `Starting` — on the same binding, pane and
//! generation — and every later tick tries `agent_start` once more. Nothing
//! before the start is run again: the worktree, `begin_onboard` and the
//! environment line are done exactly once.
//!
//! The plan is durable (the store's meta), so an engine restart continues the
//! wait instead of starting the seat over.

use gear_model::GearResult;
use gear_store::Store;

/// The pane line that says the CLI is not there (T-286): the first line of
/// `screen` that names `kind` and one of the shell's "not found" texts — zsh
/// `zsh: command not found: codex`, bash `bash: codex: command not found`,
/// dash `sh: 1: codex: not found`. `None` when the screen says nothing of the
/// sort; a failed start without this line keeps its own reason.
pub(crate) fn cli_missing(screen: &str, kind: &str) -> Option<String> {
    screen
        .lines()
        .find(|line| {
            line.contains(kind)
                && (line.contains("command not found") || line.contains(": not found"))
        })
        .map(|line| line.trim().to_string())
}

/// The reason a seat whose CLI is not installed carries (T-286): no model move
/// — the seat waits for the install, not for the next entry of its chain.
pub(crate) fn cli_missing_text(kind: &str, line: &str) -> String {
    format!(
        "the {kind} CLI is not installed or not on PATH (the pane says: {line}); install it or fix PATH, then resolve this item — GEAR does not move this seat to another model for it"
    )
}

/// How many ticks a busy refusal may keep a start pending: one tick is one
/// attempt, so a shell gets twenty of them — 80 s at the default 4 s tick —
/// before a human is asked. Past that the shell is not coming back on its own.
pub const MAX_TICKS: i64 = 20;

/// The start of a seat that Herdr refused because the pane was busy: everything
/// the next attempt needs, and how many refusals it has seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingStart {
    /// The pane of the binding (`generation`): the start is never moved.
    pub pane: String,
    /// The generation `begin_onboard` booked; the agent must be this one.
    pub generation: i64,
    /// `<project>-<lane>`, as Herdr names the agent.
    pub agent_name: String,
    /// The Herdr agent kind.
    pub kind: String,
    /// The start arguments, already rendered for the worktree the agent runs
    /// in (T-76): the retry must not render them again for another directory.
    pub args: Vec<String>,
    /// Busy refusals recorded so far, this one included.
    pub tries: i64,
}

/// The pending start of `seat_id`, if one is unfinished.
pub fn pending(store: &mut Store, seat_id: &str) -> GearResult<Option<PendingStart>> {
    let Some(raw) = store.read(|tx| tx.get_meta(&pending_key(seat_id)))? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(record) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    let text = |key: &str| {
        record
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let pane = text("pane");
    let generation = record
        .get("generation")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let agent_name = text("agent_name");
    let kind = text("kind");
    // A record without the binding it belongs to cannot be retried safely: the
    // start would go to a pane nobody checked.
    if pane.is_empty() || generation < 1 || agent_name.is_empty() || kind.is_empty() {
        return Ok(None);
    }
    let args = record
        .get("args")
        .and_then(|value| value.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(PendingStart {
        pane,
        generation,
        agent_name,
        kind,
        args,
        tries: record
            .get("tries")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
    }))
}

/// Keep `pending` as the seat's unfinished start.
pub fn set_pending(store: &mut Store, seat_id: &str, pending: &PendingStart) -> GearResult<()> {
    let record = serde_json::json!({
        "pane": pending.pane,
        "generation": pending.generation,
        "agent_name": pending.agent_name,
        "kind": pending.kind,
        "args": pending.args,
        "tries": pending.tries,
    });
    store.write(|tx| tx.set_meta(&pending_key(seat_id), &record.to_string()))
}

/// Forget a seat's unfinished start (the agent is up, or the seat parked).
pub fn clear_pending(store: &mut Store, seat_id: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(&pending_key(seat_id), ""))
}

/// The meta key of a seat's pending start, for callers that clear it inside
/// their own transaction.
pub fn pending_key(seat_id: &str) -> String {
    format!("engine.start.{seat_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_start() -> PendingStart {
        PendingStart {
            pane: "w1:p9".into(),
            generation: 2,
            agent_name: "toy-heat".into(),
            kind: "codex".into(),
            args: vec!["--model".into(), "m".into(), "-C".into(), "/w".into()],
            tries: 3,
        }
    }

    #[test]
    fn the_three_shells_that_say_a_cli_is_missing() {
        for screen in [
            "zsh: command not found: codex",
            "bash: codex: command not found",
            "sh: 1: codex: not found",
        ] {
            assert_eq!(
                cli_missing(screen, "codex").as_deref(),
                Some(screen),
                "{screen}"
            );
        }
        // Another CLI missing says nothing about this one.
        assert_eq!(cli_missing("command not found: pi", "codex"), None);
    }

    #[test]
    fn a_pending_start_round_trips() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(pending(&mut store, "toy:heat").unwrap(), None);

        set_pending(&mut store, "toy:heat", &pending_start()).unwrap();
        assert_eq!(
            pending(&mut store, "toy:heat").unwrap(),
            Some(pending_start())
        );

        clear_pending(&mut store, "toy:heat").unwrap();
        assert_eq!(pending(&mut store, "toy:heat").unwrap(), None);
    }

    #[test]
    fn a_damaged_record_is_no_pending_start() {
        let mut store = Store::open_in_memory().unwrap();
        // A start without its pane cannot be retried on the same binding.
        store
            .write(|tx| {
                tx.set_meta(
                    &pending_key("toy:heat"),
                    r#"{"generation":2,"agent_name":"toy-heat","kind":"codex"}"#,
                )
            })
            .unwrap();
        assert_eq!(pending(&mut store, "toy:heat").unwrap(), None);

        store
            .write(|tx| tx.set_meta(&pending_key("toy:heat"), "not json"))
            .unwrap();
        assert_eq!(pending(&mut store, "toy:heat").unwrap(), None);
    }
}
