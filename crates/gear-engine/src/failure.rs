//! T-607: a project tick that keeps failing rings the captain.
//!
//! On 2026-11-05 the engine logged `project_failed` every 4 s for 40 minutes
//! and no item opened: a project whose tick fails is reported in
//! [`crate::TickReport::project_failed`], but nobody was told. This module
//! counts the consecutive failed ticks of a project in the store's `meta`
//! (`engine.project_failed.<project>`: the count and the first failure's
//! time) and, once the spell is long enough (15 ticks in a row **and** 60 s
//! since the first), opens one urgent `System` item to the project's captain
//! on thread `engine-failed:<project>`. The item's body is updated while the
//! failures go on; the first good tick resolves it and resets the count.
//!
//! A store write that fails is logged in the engine log and the tick goes on:
//! the failure item is a report, never a reason to stop the engine.

use std::collections::BTreeMap;

use gear_model::{GearError, GearResult, MsgKind, MsgState, Ts};
use gear_store::{NewMessage, Store};

/// Consecutive failed ticks that make a spell worth the captain's attention.
const FAIL_TICKS: i64 = 15;
/// The spell must also have lasted this long (the later of the two, T-607).
const FAIL_FOR_S: i64 = 60;

/// The key of a project's failed-tick counter: `{"count":N,"first":T}`.
fn count_key(project: &str) -> String {
    format!("engine.project_failed.{project}")
}

/// The key of the item opened for the spell.
fn item_key(project: &str) -> String {
    format!("engine.failed_item.{project}")
}

/// The thread the captain's item lives on.
fn thread(project: &str) -> String {
    format!("engine-failed:{project}")
}

/// The counter of a project's current spell.
#[derive(Debug, Clone, Copy)]
struct Spell {
    count: i64,
    first: Ts,
}

fn read_spell(store: &mut Store, project: &str) -> GearResult<Option<Spell>> {
    let Some(raw) = store.read(|tx| tx.get_meta(&count_key(project)))? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    Ok(Some(Spell {
        count: value.get("count").and_then(|v| v.as_i64()).unwrap_or(0),
        first: value.get("first").and_then(|v| v.as_i64()).unwrap_or(0),
    }))
}

fn write_spell(store: &mut Store, project: &str, spell: Spell) -> GearResult<()> {
    let raw = serde_json::json!({"count": spell.count, "first": spell.first}).to_string();
    store.write(|tx| tx.set_meta(&count_key(project), &raw))
}

fn clear_spell(store: &mut Store, project: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(&count_key(project), ""))
}

/// The item's body: the error, how long the spell has lasted and how many
/// ticks failed. It is rewritten on every failed tick while the spell goes on.
fn body(error: &GearError, spell: Spell, now: Ts) -> String {
    let secs = (now - spell.first).max(0);
    format!(
        "the project tick has failed {} times in a row over {} s: {}: {}. The engine keeps ticking; this item updates while the failures go on and resolves on the first good tick.",
        spell.count, secs, error.code, error.detail
    )
}

/// Count one failed tick of `project` and open or update the captain's item
/// when the spell is long enough.
fn note(store: &mut Store, project: &str, error: &GearError, now: Ts) -> GearResult<()> {
    let spell = match read_spell(store, project)? {
        Some(spell) => Spell {
            count: spell.count + 1,
            first: spell.first,
        },
        None => Spell {
            count: 1,
            first: now,
        },
    };
    write_spell(store, project, spell)?;
    if spell.count < FAIL_TICKS || now - spell.first < FAIL_FOR_S {
        return Ok(());
    }
    let Some(captain) = store.read(|tx| tx.captain_of(project))? else {
        return Ok(());
    };
    let text = body(error, spell, now);
    let existing = store
        .read(|tx| tx.get_meta(&item_key(project)))?
        .filter(|id| !id.is_empty());
    if let Some(id) = existing {
        let item = store.read(|tx| tx.get_message(project, &id))?;
        if let Some(item) = item {
            if matches!(item.state, MsgState::Open | MsgState::Waiting) {
                if item.body != text {
                    store.write(|tx| tx.update_message_body(project, &id, &text))?;
                }
                return Ok(());
            }
        }
        // The item was resolved elsewhere: open a new one for this spell.
    }
    let message = store.write(|tx| {
        tx.insert_message(&NewMessage {
            meta: None,
            project: project.into(),
            thread: thread(project),
            task: None,
            attempt: None,
            sender: "engine".into(),
            sender_gen: 0,
            recipient: captain.id,
            kind: MsgKind::System,
            urgent: true,
            body: text,
            reply_to: None,
            needs_ack: false,
            state: MsgState::Open,
            created: now,
        })
    })?;
    store.write(|tx| tx.set_meta(&item_key(project), &message.id))
}

/// The first good tick: resolve the captain's item (if one was opened) and
/// reset the counter.
fn good(store: &mut Store, project: &str, now: Ts) -> GearResult<()> {
    let spell = read_spell(store, project)?;
    let id = store
        .read(|tx| tx.get_meta(&item_key(project)))?
        .filter(|id| !id.is_empty());
    if let Some(id) = id {
        if let Some(item) = store.read(|tx| tx.get_message(project, &id))? {
            if matches!(item.state, MsgState::Open | MsgState::Waiting) {
                let result = match spell {
                    Some(spell) => format!(
                        "the project tick is good again after {} failed ticks",
                        spell.count
                    ),
                    None => "the project tick is good again".into(),
                };
                store.write(|tx| {
                    tx.update_message_state(
                        project,
                        &id,
                        MsgState::Resolved,
                        Some(&result),
                        None,
                        None,
                        now,
                    )
                })?;
            }
        }
        store.write(|tx| tx.set_meta(&item_key(project), ""))?;
    }
    clear_spell(store, project)
}

/// Settle the failed-tick bookkeeping of one tick: every project that failed
/// is counted (and rings the captain once the spell is long enough); every
/// project that did not fail has its spell resolved and reset.
///
/// A store write that fails is logged in the engine log and the tick goes on
/// (T-607): the failure item is a report, never a reason to stop the engine.
pub fn settle(
    store: &mut Store,
    projects: &[String],
    failed: &BTreeMap<String, GearError>,
    now: Ts,
) {
    for project in projects {
        let outcome = match failed.get(project) {
            Some(error) => note(store, project, error, now),
            None => good(store, project, now),
        };
        if let Err(error) = outcome {
            crate::release::log_line(format!(
                "engine-failed {project}: {}: {}",
                error.code, error.detail
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_model::ErrorCode;
    use gear_store::testing;

    const NOW: Ts = 1_900_000_000;

    fn world() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", NOW)?;
                testing::seed_active_seat(
                    tx,
                    "p",
                    "captain",
                    gear_model::SeatRole::Captain,
                    "w1:p9",
                    "claude",
                    NOW,
                )?;
                Ok(())
            })
            .unwrap();
        store
    }

    fn error() -> GearError {
        GearError::new(ErrorCode::BadFormat, "the plan is not valid")
    }

    fn failed(error: GearError) -> BTreeMap<String, GearError> {
        let mut map = BTreeMap::new();
        map.insert("p".into(), error);
        map
    }

    fn items(store: &mut Store) -> Vec<gear_store::MessageRow> {
        store
            .read(|tx| tx.actionable_for("p", "p:captain", NOW + 10_000))
            .unwrap()
    }

    #[test]
    fn project_failed_fourteen_ticks_open_nothing_and_the_fifteenth_rings_the_captain() {
        let mut store = world();
        let projects = vec!["p".to_string()];
        // 5 s apart: the 15th tick is 70 s after the first, past the 60 s line.
        for i in 0..14 {
            settle(&mut store, &projects, &failed(error()), NOW + i * 5);
        }
        assert!(items(&mut store).is_empty(), "14 failed ticks open nothing");
        settle(&mut store, &projects, &failed(error()), NOW + 14 * 5);
        let open = items(&mut store);
        assert_eq!(open.len(), 1, "the 15th tick opens one item");
        let item = &open[0];
        assert!(item.urgent, "the item is urgent");
        assert_eq!(item.kind, MsgKind::System);
        assert_eq!(item.thread, "engine-failed:p");
        assert!(item.body.contains("BAD_FORMAT"), "{}", item.body);
        assert!(item.body.contains("the plan is not valid"), "{}", item.body);
        assert!(item.body.contains("15 times"), "{}", item.body);
    }

    #[test]
    fn project_failed_more_failures_keep_one_item_and_update_its_body() {
        let mut store = world();
        let projects = vec!["p".to_string()];
        for i in 0..15 {
            settle(&mut store, &projects, &failed(error()), NOW + i * 5);
        }
        let first = items(&mut store);
        assert_eq!(first.len(), 1);
        let id = first[0].id.clone();
        let body = first[0].body.clone();
        settle(&mut store, &projects, &failed(error()), NOW + 15 * 5);
        let open = items(&mut store);
        assert_eq!(open.len(), 1, "more failures keep one item");
        assert_eq!(open[0].id, id);
        assert_ne!(open[0].body, body, "the body says the spell goes on");
        assert!(open[0].body.contains("16 times"), "{}", open[0].body);
    }

    #[test]
    fn engine_failed_a_good_tick_resolves_the_item_and_resets_the_count() {
        let mut store = world();
        let projects = vec!["p".to_string()];
        for i in 0..15 {
            settle(&mut store, &projects, &failed(error()), NOW + i * 5);
        }
        let id = items(&mut store)[0].id.clone();
        settle(&mut store, &projects, &BTreeMap::new(), NOW + 15 * 5);
        assert!(items(&mut store).is_empty(), "the item is resolved");
        let item = store.read(|tx| tx.get_message("p", &id)).unwrap().unwrap();
        assert_eq!(item.state, MsgState::Resolved);
        assert!(
            item.result
                .as_deref()
                .is_some_and(|r| r.contains("15 failed")),
            "{:?}",
            item.result
        );
        // The count is reset: a new spell needs 15 ticks again.
        for i in 0..14 {
            settle(&mut store, &projects, &failed(error()), NOW + 100 + i * 5);
        }
        assert!(items(&mut store).is_empty(), "the count was reset");
    }
}
