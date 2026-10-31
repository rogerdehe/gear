//! A delivery that was never confirmed (T-97).
//!
//! A prompt Herdr accepted is not a task the lane received: a quota error, a
//! dialog or a prompt the agent swallowed can leave the delivery unconfirmed
//! for good. The live case (multi, T-3) was a delivery that stayed `submitted`
//! past its check budget: the seat was flagged every tick, the captain
//! restarted the agent by hand, and the task was never delivered again — the
//! unsettled op held the seat's delivery slot forever.
//!
//! Now the budget spent, the op fails (the slot is free again) and the captain
//! gets one item. The open attempt is delivered again — but at most once per
//! **agent session**: re-sending into the same session repeats the same
//! failure. A hand restart of the agent (the pane reports a new session) or a
//! new generation (a restart GEAR did) is the signal. A model switch typed into
//! the agent after the delivery asks for it at once (`retry_now`): the switch
//! retires the prompt it could not leave alone.

use gear_model::GearResult;
use gear_store::Tx;

/// One delivery that was sent and never confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedDelivery {
    pub task: String,
    pub attempt: i64,
    /// The seat generation the delivery was sent in.
    pub generation: i64,
    /// The agent session it was sent into (the binding's). A kind that reports
    /// no session has `None`, and only a new generation then retries.
    pub session: Option<String>,
    /// Send the attempt again as soon as the seat is idle: a model switch typed
    /// into the agent after the delivery (T-97, the quota path).
    pub retry_now: bool,
}

/// The delivery a seat last failed to have confirmed, if any.
pub fn failed(tx: &Tx<'_>, seat: &str) -> GearResult<Option<FailedDelivery>> {
    let Some(raw) = tx.get_meta(&key(seat))? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(record) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    let text = |name: &str| {
        record
            .get(name)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let task = text("task");
    let attempt = record
        .get("attempt")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    if task.is_empty() || attempt < 1 {
        return Ok(None);
    }
    Ok(Some(FailedDelivery {
        task,
        attempt,
        generation: record
            .get("generation")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        session: record
            .get("session")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .filter(|session| !session.is_empty()),
        retry_now: record
            .get("retry_now")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    }))
}

/// Record `failed` as the seat's last unconfirmed delivery.
pub fn record(tx: &Tx<'_>, seat: &str, failed: &FailedDelivery) -> GearResult<()> {
    let record = serde_json::json!({
        "task": failed.task,
        "attempt": failed.attempt,
        "generation": failed.generation,
        "session": failed.session,
        "retry_now": failed.retry_now,
    });
    tx.set_meta(&key(seat), &record.to_string())
}

/// Forget a seat's unconfirmed delivery: the attempt was delivered again, or
/// the seat moved on.
pub fn clear(tx: &Tx<'_>, seat: &str) -> GearResult<()> {
    tx.set_meta(&key(seat), "")
}

/// The meta key of a seat's unconfirmed delivery.
pub fn key(seat: &str) -> String {
    format!("engine.delivery.{seat}")
}

/// The meta key of the agent session a delivery's prompt went into (T-97).
pub fn sent_key(op_id: &str) -> String {
    format!("engine.deliver_session.{op_id}")
}

/// Fix the session a delivery's prompt goes into, when the op is prepared for
/// the send (T-97). A hand restart may refresh the binding before the op fails;
/// the record must name the session that actually received the prompt, so the
/// new session can be delivered again and the old one is not blamed.
pub fn record_sent(tx: &Tx<'_>, op_id: &str, session: Option<&str>) -> GearResult<()> {
    tx.set_meta(&sent_key(op_id), session.unwrap_or(""))
}

/// The session recorded when the prompt was sent: `Some(None)` when the kind
/// reported none, `None` when nothing was recorded (an op prepared before
/// T-97's snapshot).
pub fn sent(tx: &Tx<'_>, op_id: &str) -> GearResult<Option<Option<String>>> {
    let Some(raw) = tx.get_meta(&sent_key(op_id))? else {
        return Ok(None);
    };
    Ok(Some(Some(raw).filter(|session| !session.is_empty())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_store::Store;

    fn failed_delivery() -> FailedDelivery {
        FailedDelivery {
            task: "T-3".into(),
            attempt: 2,
            generation: 4,
            session: Some("ses_a".into()),
            retry_now: false,
        }
    }

    #[test]
    fn an_unconfirmed_delivery_round_trips() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.read(|tx| failed(tx, "p:heat")).unwrap(), None);

        store
            .write(|tx| record(tx, "p:heat", &failed_delivery()))
            .unwrap();
        assert_eq!(
            store.read(|tx| failed(tx, "p:heat")).unwrap(),
            Some(failed_delivery())
        );

        store.write(|tx| clear(tx, "p:heat")).unwrap();
        assert_eq!(store.read(|tx| failed(tx, "p:heat")).unwrap(), None);
    }

    #[test]
    fn a_damaged_record_is_no_unconfirmed_delivery() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| tx.set_meta(&key("p:heat"), r#"{"task":"T-3"}"#))
            .unwrap();
        assert_eq!(store.read(|tx| failed(tx, "p:heat")).unwrap(), None);

        store
            .write(|tx| tx.set_meta(&key("p:heat"), "not json"))
            .unwrap();
        assert_eq!(store.read(|tx| failed(tx, "p:heat")).unwrap(), None);
    }
}
