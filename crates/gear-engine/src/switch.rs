//! Changing an agent's model **in place** (T-33): the OpenCode session's own
//! API (`session.switchModel`), then the status bar must show the wanted
//! model. No exit, no restart: the conversation stays. No key is typed at all
//! (T-531): `/model`, the picker filter and the variant dialog write the
//! variant and the recent list into `~/.local/state/opencode/model.json`, the
//! default of every OpenCode session on the machine, which is not the seat's
//! own change to make.
//!
//! The switch is a **state machine across ticks** (review F2/F3/F6): the plan
//! (the status-bar text to reach) is stored in the seat's meta *before* the
//! first API call, the whole visible screen is re-read before every request,
//! and a slow service or render gets a tick to answer instead of burning a
//! failed attempt. An engine restart mid-switch resumes the plan instead of
//! asking the service blindly.
//!
//! Never calls into a working agent (review F8): the pane's occupant and status
//! are read again immediately before any request; a stale `idle` from this
//! tick's observation is not trusted.
//!
//! Only the OpenCode protocol is implemented: it is the one the SOP
//! (`gear --skill agent:opencode`, ops/bin/switch-model.sh) verifies, and
//! guessing another CLI's picker is worse than flagging the seat (P2). A CLI
//! without a picker GEAR drives (pi) is not switched here at all: the caller
//! starts it again on the next model ([`crate::restart`]).

use gear_app::config::ModelSwitch;
use gear_herdr::{HerdrAct, Profile};
use gear_model::{GearResult, Ts};
use gear_store::Store;

use crate::observe::SeatObs;

/// An OpenCode status bar line begins with this: `Build auto · <model>
/// <provider> · <effort>` (SOP `agent:opencode`).
const STATUS_BAR_PREFIX: &str = "Build auto";
/// The box edges OpenCode draws left of its input and status bar.
const BOX_EDGES: [char; 3] = ['┃', '│', '╹'];
/// The variant dialog's title; it is drawn in the middle of the visible screen,
/// far above the last 16 lines when the conversation is long (review F3).
const VARIANT_TITLE: &str = "Select variant";
/// How many ticks one step may wait for the screen or the service to answer
/// before it counts as a failed try. A render that never comes must not hang
/// the lane.
const WAIT_TICKS: i64 = 6;

/// What a switch attempt did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// The status bar already shows the model: nothing was sent.
    Already,
    /// The switch was confirmed by the status bar: the model is on.
    Done,
    /// A step is waiting for the screen (or the agent is working): the next
    /// tick continues the plan. Nothing is flagged.
    Waiting,
    /// The plan did not confirm twice: the caller flags the seat instead of
    /// looping.
    Failed,
}

/// Can GEAR change this CLI's model **in place** (T-33)? Only OpenCode's
/// protocol is driven today: the picker, the variant dialog and the status
/// bar (SOP `gear --skill agent:opencode`).
///
/// A CLI without a picker GEAR drives (pi) is **not** switched in place: it is
/// closed and started again on the next model ([`crate::restart`]), the same
/// pane and worktree, the lane's task delivered again in the new session.
pub fn switches_in_place(kind: &str) -> bool {
    kind == "opencode"
}

/// The one step a plan has left (T-531): the service accepted the target, so
/// the switch waits for `session.get` and the TUI bar. The stored text stays
/// `api-confirm`: a plan written by an earlier binary with a typed step
/// (`escape`, `command`, `filter`, `variant`, `confirm`) no longer parses and
/// is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Service accepted the target; wait for session.get and the TUI bar.
    ApiConfirm,
}

impl Step {
    fn as_str(self) -> &'static str {
        match self {
            Step::ApiConfirm => "api-confirm",
        }
    }

    fn parse(text: &str) -> Option<Step> {
        match text {
            "api-confirm" => Some(Step::ApiConfirm),
            _ => None,
        }
    }
}

/// An unfinished switch, durable in the seat's meta before the first key
/// (review F6): the bar text to reach, the next step, failed tries and how long
/// the current step has waited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The status-bar text that confirms the model (`[models.switch.*].bar`).
    pub to: String,
    pub step: Step,
    pub tries: i64,
    pub waited: i64,
}

/// The model and labels a pending switch plan aims at (its `to` is the bar
/// text): the `[models.switch]` entry when the config has one, else the labels
/// OpenCode's catalogue gives a chain model (T-94), in the seat's worktree. A
/// plan begun on derived labels must be continued on them too — a lane with no
/// hand-written labels was flagged on its first resumed step (M3 acceptance).
pub(crate) fn plan_target(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    config: &gear_app::config::ProjectConfig,
    seat_id: &str,
    to: &str,
) -> GearResult<Option<(String, gear_app::config::ModelSwitch)>> {
    if let Some((model, labels)) = config
        .models
        .switch
        .iter()
        .find(|(_, labels)| labels.bar == to)
    {
        return Ok(Some((model.clone(), labels.clone())));
    }
    let directory = store.read(|tx| tx.get_meta(&crate::onboard::worktree_path_key(seat_id)))?;
    let chain = config
        .models
        .lanes
        .iter()
        .chain(config.models.review.iter())
        .chain(config.models.captain.iter());
    for entry in chain {
        let model = entry.model();
        if config.models.switch.contains_key(model) {
            continue;
        }
        if let Ok(Some(labels)) =
            crate::opencode::derived_labels(herdr, model, None, directory.as_deref())
        {
            if labels.bar == to {
                return Ok(Some((model.to_string(), labels)));
            }
        }
    }
    Ok(None)
}

/// The switch plan of a seat, if one is unfinished.
pub fn pending(store: &mut Store, seat_id: &str) -> GearResult<Option<Pending>> {
    let Some(raw) = meta_get(store, &pending_key(seat_id))? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let mut parts = raw.splitn(4, '|');
    let (Some(step), Some(tries), Some(waited)) = (parts.next(), parts.next(), parts.next()) else {
        return Ok(None);
    };
    let Some(step) = Step::parse(step) else {
        // A plan stored by an earlier binary with a typed step (`escape`,
        // `command`, `filter`, `variant`, `confirm`): the keys path is gone, so
        // the plan is dropped and the API path starts from the beginning.
        clear_pending(store, seat_id)?;
        return Ok(None);
    };
    let to = parts.next().unwrap_or_default().to_string();
    if to.is_empty() {
        return Ok(None);
    }
    Ok(Some(Pending {
        to,
        step,
        tries: tries.parse().unwrap_or(0),
        waited: waited.parse().unwrap_or(0),
    }))
}

/// Forget a plan (after a confirmation, or when the seat is flagged).
pub fn clear_pending(store: &mut Store, seat_id: &str) -> GearResult<()> {
    meta_set(store, &pending_key(seat_id), "")
}

/// The status bar shows `bar`: a line that begins with the OpenCode status-bar
/// prefix and contains the text — a quote in the conversation does not count
/// (F3).
pub(crate) fn confirmed(screen: &str, bar: &str) -> bool {
    screen.lines().any(|line| {
        // The real bar is drawn inside the input box: `┃  Build auto · …`.
        let line = line.trim_start_matches(|c: char| c.is_whitespace() || BOX_EDGES.contains(&c));
        line.starts_with(STATUS_BAR_PREFIX) && line.contains(bar)
    })
}

/// What a startup model check says about going on (F5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelGate {
    /// The status bar shows the chain's model: startup may complete.
    Ready,
    /// The switch is under way: keep the startup pending, send no work.
    Waiting,
    /// The switch cannot be made: the seat is flagged with this reason.
    Failed(String),
}

/// Make sure a freshly started agent runs on `model` before any work is queued
/// (F5): switch it in place when its status bar is elsewhere. A model without
/// `[models.switch]` labels is a config fault — flagged, never a restart.
pub fn ensure_model(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &gear_store::SeatRow,
    profile: &Profile,
    config: &gear_app::config::ProjectConfig,
    model: &str,
    now: Ts,
) -> GearResult<ModelGate> {
    let labels = match labels_for(store, herdr, config, &seat.id, model)? {
        Ok(labels) => labels,
        Err(reason) => return Ok(ModelGate::Failed(reason)),
    };
    match switch_model(store, herdr, &seat.id, profile, None, &labels, model, now)? {
        SwitchOutcome::Done | SwitchOutcome::Already => Ok(ModelGate::Ready),
        SwitchOutcome::Waiting => Ok(ModelGate::Waiting),
        SwitchOutcome::Failed => Ok(ModelGate::Failed(format!(
            "the model switch to {model} did not confirm twice"
        ))),
    }
}

/// The labels that put an agent on `model` (T-94): the config's
/// `[models.switch]` entry, else OpenCode's own catalogue (`<model name>
/// <provider name>`, the text the picker and the status bar show), read in the
/// seat's worktree (F2 of the T-94 review). `Err(reason)` when the model cannot
/// be named: the caller flags the seat, never restarts it.
fn labels_for(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    config: &gear_app::config::ProjectConfig,
    seat_id: &str,
    model: &str,
) -> GearResult<Result<ModelSwitch, String>> {
    if let Some(labels) = config.models.switch.get(model) {
        return Ok(Ok(labels.clone()));
    }
    // The chain names no variant (a `[models.switch]` entry is the only place
    // one lives, and it overrides this path entirely).
    let directory = store.read(|tx| tx.get_meta(&crate::onboard::worktree_path_key(seat_id)))?;
    match crate::opencode::derived_labels(herdr, model, None, directory.as_deref()) {
        Ok(Some(labels)) => Ok(Ok(labels)),
        Ok(None) => Ok(Err(format!(
            "no [models.switch.\"{model}\"] labels (picker, bar) and {model} is not in OpenCode's model list: the agent cannot be put on the chain's model"
        ))),
        // T-307: an unreadable catalogue (a failed call, a reply that is not a
        // list, an empty list not loaded yet, a provider with no display name)
        // says nothing about the model. A Waiting gate would keep a start
        // pending with nothing to end it, so the seat is flagged with a text
        // that does not send the captain after a model id that is right.
        Err(reason) => Ok(Err(format!(
            "OpenCode's model list could not be read ({reason}), so {model} could not be checked; it is not known to be absent: name [models.switch.\"{model}\"] labels (picker, bar), or check `opencode models`"
        ))),
    }
}

/// Begin the in-place switch of a seat whose chain head changed model on its
/// own CLI (T-464): the durable plan is recorded and **no request is made
/// here**. The tick's switch step, which resumes a pending plan, makes the
/// first request in the same tick, so a config change never sends two
/// submissions in one tick. `Err(reason)` when the model has no labels
/// anywhere: the caller flags the seat and sends nothing.
pub fn begin(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat_id: &str,
    config: &gear_app::config::ProjectConfig,
    model: &str,
) -> GearResult<Result<(), String>> {
    let labels = match labels_for(store, herdr, config, seat_id, model)? {
        Ok(labels) => labels,
        Err(reason) => return Ok(Err(reason)),
    };
    set_pending(
        store,
        seat_id,
        &Pending {
            to: labels.bar,
            step: Step::ApiConfirm,
            tries: 0,
            waited: 0,
        },
    )?;
    Ok(Ok(()))
}

/// Switch a seat's OpenCode agent through the session API alone (T-531): the
/// model is set with `session.switchModel` and the status bar confirms it; no
/// key is ever typed into the pane. A durable plan prevents dispatch while a
/// TUI render lags, including across an engine restart.
///
/// Nothing here falls back to keys: a seat the service cannot switch is
/// `Failed` and the caller flags it.
pub fn switch_model(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat_id: &str,
    profile: &Profile,
    obs: Option<&SeatObs>,
    labels: &ModelSwitch,
    model: &str,
    now: Ts,
) -> GearResult<SwitchOutcome> {
    let _ = now;
    // (3) Only OpenCode's session API is driven: any other kind fails with
    // nothing sent, as the in-place switch always did for it.
    if !switches_in_place(&profile.kind) {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    }
    // (2) A plan written by an earlier binary with a typed step was dropped by
    // `pending`; the API path starts from the beginning.
    let resumed = pending(store, seat_id)?;
    if obs.is_some_and(|obs| !obs.idle) {
        return Ok(SwitchOutcome::Waiting);
    }
    let binding = store.read(|tx| {
        let Some(seat) = tx.get_seat(seat_id)? else {
            return Ok(None);
        };
        tx.get_binding(seat_id, seat.generation)
    })?;
    let Some(binding) = binding else {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    };
    // (1) A binding with no service session cannot be switched through the
    // API, and nothing is typed for it any more.
    let Some(session) = binding.session_ref.as_deref().filter(|id| !id.is_empty()) else {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    };
    // Herdr can report a launch-pending/unknown agent before the TUI is ready.
    // Record the wait now so onboarding resumes it on the following tick.
    let mut plan = resumed
        .clone()
        .filter(|p| p.to == labels.bar)
        .unwrap_or(Pending {
            to: labels.bar.clone(),
            step: Step::ApiConfirm,
            tries: 0,
            waited: 0,
        });
    set_pending(store, seat_id, &plan)?;
    match herdr.agent_get(&binding.pane_id) {
        Ok(Some(info)) if info.agent.as_deref() == Some("opencode") => {
            if !matches!(
                info.agent_status,
                gear_herdr::AgentStatus::Idle | gear_herdr::AgentStatus::Done
            ) {
                if info.agent_status != gear_herdr::AgentStatus::Unknown {
                    return Ok(SwitchOutcome::Waiting);
                }
                return wait_for_agent(store, seat_id, &mut plan);
            }
        }
        Ok(_) => return wait_for_agent(store, seat_id, &mut plan),
        Err(_) => return Ok(SwitchOutcome::Waiting),
    }
    // M4 §10.2: the screen never says that an agent is busy — Herdr's state
    // does, and it was read above (idle or done). The screen is still read
    // for the confirmation of the switch.
    let Ok(screen) = herdr.read_screen(&binding.pane_id) else {
        return Ok(SwitchOutcome::Waiting);
    };
    let directory = meta_get(store, &crate::onboard::worktree_path_key(seat_id))?;
    let target = match crate::opencode::target(
        herdr,
        model,
        labels.variant.as_deref(),
        directory.as_deref(),
    ) {
        crate::opencode::Lookup::Target(target) => target,
        // The model is not in a loaded catalogue: a config fault.
        crate::opencode::Lookup::Invalid(_) => {
            clear_pending(store, seat_id)?;
            return Ok(SwitchOutcome::Failed);
        }
        // (1) A catalogue that is not loaded yet, or no answer: wait for the
        // service, bounded like the agent gate, never a key.
        crate::opencode::Lookup::NotLoaded | crate::opencode::Lookup::Unavailable => {
            return wait_for_agent(store, seat_id, &mut plan)
        }
    };
    let get = vec!["GET".into(), format!("/api/session/{session}")];
    let value = match herdr.opencode_api(&get) {
        Ok(value) => value,
        Err(_) => return wait_for_agent(store, seat_id, &mut plan),
    };
    if !crate::opencode::matches(&value, &target) {
        if plan.tries == 0 || (plan.waited == WAIT_TICKS && plan.tries < 2) {
            // Save the intent before the request. After a timeout/crash, GET
            // determines whether another submission is needed.
            set_pending(store, seat_id, &plan)?;
            plan.tries += 1;
            set_pending(store, seat_id, &plan)?;
            let body = serde_json::json!({"model": target});
            if herdr
                .opencode_api(&[
                    "session.switchModel".into(),
                    "--param".into(),
                    format!("sessionID={session}"),
                    "-d".into(),
                    body.to_string(),
                ])
                .is_err()
            {
                return wait_for_agent(store, seat_id, &mut plan);
            }
        }
    } else {
        // The API and bar are independent confirmations; a quote in the
        // conversation and a bar showing the wrong effort do not qualify.
        let variant_ok = labels.variant.as_deref().is_none_or(|variant| {
            screen.lines().any(|line| {
                confirmed(line, &labels.bar) && line.trim_end().ends_with(&format!("· {variant}"))
            })
        });
        if confirmed(&screen, &labels.bar) && variant_ok && !screen.contains(VARIANT_TITLE) {
            clear_pending(store, seat_id)?;
            return Ok(if resumed.is_some() {
                SwitchOutcome::Done
            } else {
                SwitchOutcome::Already
            });
        }
    }
    plan.waited += 1;
    if plan.waited > WAIT_TICKS * 2 {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    }
    set_pending(store, seat_id, &plan)?;
    Ok(SwitchOutcome::Waiting)
}

fn wait_for_agent(store: &mut Store, seat: &str, plan: &mut Pending) -> GearResult<SwitchOutcome> {
    plan.waited += 1;
    if plan.waited > WAIT_TICKS * 2 {
        clear_pending(store, seat)?;
        return Ok(SwitchOutcome::Failed);
    }
    set_pending(store, seat, plan)?;
    Ok(SwitchOutcome::Waiting)
}

fn pending_key(seat_id: &str) -> String {
    format!("engine.switch.{seat_id}")
}

/// The meta key of a seat's plan, for callers that clear it inside their own
/// transaction (the quota switch finishes plan and Continue together).
pub fn pending_meta_key(seat_id: &str) -> String {
    pending_key(seat_id)
}

fn set_pending(store: &mut Store, seat_id: &str, plan: &Pending) -> GearResult<()> {
    let raw = format!(
        "{}|{}|{}|{}",
        plan.step.as_str(),
        plan.tries,
        plan.waited,
        plan.to
    );
    meta_set(store, &pending_key(seat_id), &raw)
}

fn meta_get(store: &mut Store, key: &str) -> GearResult<Option<String>> {
    store.read(|tx| tx.get_meta(key))
}

fn meta_set(store: &mut Store, key: &str, value: &str) -> GearResult<()> {
    store.write(|tx| tx.set_meta(key, value))
}

#[cfg(test)]
mod tests {

    /// L-71: the real OpenCode bar sits inside the input box's edge; it was
    /// never recognised, so every switch "failed" after the model was on.
    #[test]
    fn the_real_bar_inside_the_box_edge_is_recognised() {
        let screen = "  ┃  Ask anything… \"Fix a TODO in the codebase\"\n  ┃\n  ┃  Build auto · MiniMax-M3.1-Flash MiniMax (minimax.cn)\n  ╹▀▀▀▀\n";
        assert!(confirmed(screen, "MiniMax-M3.1-Flash MiniMax (minimax.cn)"));
        assert!(!confirmed(screen, "DeepSeek V4.1 Flash OpenCode Go"));
    }

    use super::*;
    use crate::observe::{Presence, SeatObs};
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::SeatRole;
    use gear_store::testing;
    use std::collections::BTreeSet;

    /// T-114: only OpenCode's picker is driven, so only OpenCode is switched
    /// in place. A CLI without one (pi) is started again on the next model.
    /// T-531: a seat of any other kind is `Failed` and nothing is sent for it.
    #[test]
    fn only_opencode_is_switched_in_place() {
        assert!(switches_in_place("opencode"));
        for kind in ["pi", "codex", "claude", "grok"] {
            assert!(!switches_in_place(kind), "{kind} has no picker GEAR drives");
        }
        let (mut store, mut herdr) = api_world();
        let profile = gear_herdr::builtin("codex").unwrap();
        let outcome = switch_model(
            &mut store,
            &mut herdr,
            "p:heat",
            &profile,
            None,
            &labels(),
            "opencode-go/deepseek-v4.1-flash",
            T0,
        )
        .unwrap();
        assert_eq!(outcome, SwitchOutcome::Failed);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
    }

    const T0: i64 = 1_800_000_000;
    const PANE: &str = "w1:p1";
    /// An OpenCode screen that is not on the wanted model.
    const OTHER_BAR: &str = "Build auto · MiniMax-M3.1-Flash MiniMax · high";

    fn api_world() -> (Store, FakeHerdr) {
        let mut store = world();
        store
            .write(|tx| tx.set_binding_session_ref("p:heat", 1, "ses_api"))
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "opencode", gear_herdr::AgentStatus::Idle, "p-heat");
        herdr.set_screen(PANE, OTHER_BAR);
        (store, herdr)
    }

    fn catalog() -> serde_json::Value {
        serde_json::json!({"data":[{"providerID":"opencode-go", "id":"deepseek-v4.1-flash",
            "variants":[{"id":"max"}, {"id":"high"}]}]})
    }

    /// A `GET /api/session/<id>` answer naming another model.
    fn other_session() -> serde_json::Value {
        serde_json::json!({"data":{"model":{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash"}}})
    }

    /// The `GET` answer that says the session is already on the target.
    fn switched_session(labels: &ModelSwitch) -> serde_json::Value {
        let mut model = serde_json::json!({
            "providerID":"opencode-go", "id":"deepseek-v4.1-flash"
        });
        if let Some(variant) = labels.variant.as_deref() {
            model["variant"] = variant.into();
        }
        serde_json::json!({"data":{"model": model}})
    }

    fn api_switch(store: &mut Store, herdr: &mut FakeHerdr, labels: &ModelSwitch) -> SwitchOutcome {
        switch_model(
            store,
            herdr,
            "p:heat",
            &gear_herdr::builtin("opencode").unwrap(),
            None,
            labels,
            "opencode-go/deepseek-v4.1-flash",
            T0,
        )
        .unwrap()
    }

    /// Tick the switch until it settles (a tick = one call).
    fn run(store: &mut Store, herdr: &mut FakeHerdr, at: Ts) -> SwitchOutcome {
        let profile = gear_herdr::builtin("opencode").unwrap();
        for tick in 0..20 {
            let outcome = switch_model(
                store,
                herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &labels(),
                "opencode-go/deepseek-v4.1-flash",
                at + tick,
            )
            .unwrap();
            if outcome != SwitchOutcome::Waiting {
                return outcome;
            }
        }
        SwitchOutcome::Waiting
    }

    /// Everything that reached the pane: typed text and keys alike.
    fn typed(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneType { text, .. } => Some(text.clone()),
                Call::SendKeys { keys, .. } => Some(keys.join("+")),
                _ => None,
            })
            .collect()
    }

    fn switch_requests(herdr: &FakeHerdr) -> Vec<Vec<String>> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::OpenCodeApi { args } if args[0] == "session.switchModel" => {
                    Some(args.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// T-531: no key reaches the pane, ever — not `/model`, not `/models`, not
    /// the picker filter, not an escape.
    fn assert_nothing_typed(herdr: &FakeHerdr) {
        let typed = typed(herdr);
        assert!(typed.is_empty(), "the pane got {typed:?}");
        assert!(
            !typed
                .iter()
                .any(|text| text.contains("/model") || text.contains("/models")),
            "{typed:?}"
        );
    }

    /// T-531: the session API alone switches the model and the bar confirms it
    /// after an engine restart. Not one key is typed.
    #[test]
    fn api_switch_confirms_session_and_bar_after_restart_without_keys() {
        let (mut store, mut herdr) = api_world();
        let mut labels = labels();
        labels.variant = Some("max".into());
        herdr.api_replies.extend([
            Ok(catalog()),
            Ok(other_session()),
            Ok(serde_json::json!({})),
        ]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            pending(&mut store, "p:heat").unwrap().unwrap().step,
            Step::ApiConfirm
        );
        // API alone is insufficient: the TUI must have rendered the variant.
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels))]);
        herdr.set_screen(PANE, &format!("┃ Build auto · {} · high", labels.bar));
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Waiting
        );
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels))]);
        herdr.set_screen(PANE, &format!("┃ Build auto · {} · max", labels.bar));
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Done
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
        assert_nothing_typed(&herdr);
        assert!(herdr
            .calls
            .iter()
            .all(|call| matches!(call, Call::OpenCodeApi { .. })));
        let requests = switch_requests(&herdr);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][2], "sessionID=ses_api");
        let body: serde_json::Value = serde_json::from_str(&requests[0][4]).unwrap();
        assert_eq!(body["model"]["variant"], "max");
    }

    /// T-531: a binding without a service session cannot be switched through
    /// the API, and nothing is typed for it.
    #[test]
    fn a_binding_without_a_session_ref_fails_without_keys() {
        let mut store = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "opencode", gear_herdr::AgentStatus::Idle, "p-heat");
        herdr.set_screen(PANE, OTHER_BAR);
        store
            .write(|tx| {
                tx.set_meta(
                    &pending_meta_key("p:heat"),
                    "api-confirm|0|2|DeepSeek V4.1 Flash OpenCode Go",
                )
            })
            .unwrap();

        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Failed
        );
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
        assert_nothing_typed(&herdr);
        assert!(
            pending(&mut store, "p:heat").unwrap().is_none(),
            "the plan is cleared"
        );
    }

    /// T-531: a service that never answers is waited for, bounded like the
    /// agent gate, then the seat is flagged — no key, ever.
    #[test]
    fn an_unavailable_api_waits_to_the_bound_then_fails_without_keys() {
        let (mut store, mut herdr) = api_world();
        // No scripted replies: every call answers "fake API unavailable".
        let mut outcomes = Vec::new();
        for _ in 0..20 {
            let outcome = api_switch(&mut store, &mut herdr, &labels());
            outcomes.push(outcome.clone());
            if outcome != SwitchOutcome::Waiting {
                break;
            }
        }
        let waited = outcomes
            .iter()
            .filter(|o| **o == SwitchOutcome::Waiting)
            .count();
        assert_eq!(
            outcomes.last(),
            Some(&SwitchOutcome::Failed),
            "{outcomes:?}"
        );
        assert_eq!(waited as i64, WAIT_TICKS * 2, "{outcomes:?}");
        assert_nothing_typed(&herdr);
        assert!(
            pending(&mut store, "p:heat").unwrap().is_none(),
            "the plan is cleared"
        );
    }

    /// T-531: a plan an earlier binary stored with a typed step (`filter`) is
    /// cleared and the API path runs from the beginning.
    #[test]
    fn a_typed_plan_from_an_earlier_binary_is_cleared_and_the_api_runs() {
        let (mut store, mut herdr) = api_world();
        store
            .write(|tx| {
                tx.set_meta(
                    &pending_meta_key("p:heat"),
                    "filter|0|0|DeepSeek V4.1 Flash OpenCode Go",
                )
            })
            .unwrap();
        assert!(
            pending(&mut store, "p:heat").unwrap().is_none(),
            "the typed step no longer parses"
        );
        let raw = store
            .read(|tx| tx.get_meta(&pending_meta_key("p:heat")))
            .unwrap()
            .unwrap_or_default();
        assert!(raw.is_empty(), "the stale plan is cleared: {raw:?}");

        herdr.api_replies.extend([
            Ok(catalog()),
            Ok(other_session()),
            Ok(serde_json::json!({})),
        ]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert_eq!(switch_requests(&herdr).len(), 1, "{:?}", herdr.calls);
        assert_eq!(
            pending(&mut store, "p:heat").unwrap().unwrap().step,
            Step::ApiConfirm
        );
        assert_nothing_typed(&herdr);
    }

    #[test]
    fn invalid_variant_never_reaches_api_switch_or_keyboard() {
        let (mut store, mut herdr) = api_world();
        let mut labels = labels();
        labels.variant = Some("unknown".into());
        herdr.api_replies.push_back(Ok(catalog()));
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Failed
        );
        assert_eq!(herdr.calls.len(), 1);
        assert_nothing_typed(&herdr);
    }

    /// T-531: a failed `session.switchModel` is waited for, not typed around.
    #[test]
    fn a_failed_api_switch_waits_and_never_types() {
        let (mut store, mut herdr) = api_world();
        herdr.api_replies.extend([
            Ok(catalog()),
            Ok(other_session()),
            Err("service unavailable".into()),
        ]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert_eq!(switch_requests(&herdr).len(), 1, "{:?}", herdr.calls);
        assert_nothing_typed(&herdr);
    }

    #[test]
    fn a_launch_pending_agent_keeps_a_durable_confirmation_wait() {
        let (mut store, mut herdr) = api_world();
        herdr.set_status(PANE, gear_herdr::AgentStatus::Unknown);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            pending(&mut store, "p:heat").unwrap().unwrap().step,
            Step::ApiConfirm
        );
        assert!(herdr.calls.is_empty());
        herdr.set_status(PANE, gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, &format!("┃ Build auto · {}", labels().bar));
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels()))]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Done
        );
        assert_nothing_typed(&herdr);
    }

    #[test]
    fn working_agent_gets_neither_api_calls_nor_keys() {
        let (mut store, mut herdr) = api_world();
        herdr.set_status(PANE, gear_herdr::AgentStatus::Working);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert!(herdr.calls.is_empty());
    }

    fn world() -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, "opencode", T0)
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

    /// A config with no `[models.switch]` entry for the model under test, so
    /// the labels come from OpenCode's catalogue (T-94).
    fn chain_config() -> gear_app::config::ProjectConfig {
        gear_app::config::parse(
            r#"
[project]
id = "p"
repo = "/repo"
herdr_session = "sess"
[quota]
lanes = 1
temps = 0
[captain]
pane = "unused"
kind = "claude"
profile = "claude"
"#,
        )
        .unwrap()
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
            screen: None,
            session: None,
        }
    }

    /// T-531: the switch runs on the API alone, one request per tick, and the
    /// pane never sees a key.
    #[test]
    fn the_switch_runs_on_the_api_alone() {
        let (mut store, mut herdr) = api_world();
        herdr.api_replies.extend([
            Ok(catalog()),
            Ok(other_session()),
            Ok(serde_json::json!({})),
        ]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert_eq!(switch_requests(&herdr).len(), 1, "{:?}", herdr.calls);
        herdr.set_screen(PANE, &format!("Build auto · {} · high", labels().bar));
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels()))]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Done
        );
        assert_nothing_typed(&herdr);
        assert_eq!(switch_requests(&herdr).len(), 1, "one switch request");
        assert!(
            pending(&mut store, "p:heat").unwrap().is_none(),
            "the plan is cleared"
        );
        assert!(
            herdr
                .calls
                .iter()
                .all(|call| !matches!(call, Call::AgentStart { .. })),
            "no restart"
        );
    }

    /// T-531: a bar that already shows the model is `Already`, and nothing is
    /// typed — only the catalogue and the session are read.
    #[test]
    fn an_already_correct_model_sends_nothing() {
        let (mut store, mut herdr) = api_world();
        herdr.set_screen(PANE, "Build auto · DeepSeek V4.1 Flash OpenCode Go · high");
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels()))]);

        assert_eq!(run(&mut store, &mut herdr, T0), SwitchOutcome::Already);
        assert_nothing_typed(&herdr);
        assert!(switch_requests(&herdr).is_empty());
    }

    /// F3: a conversation that quotes the model name is not a status bar, so
    /// the switch waits for the real bar instead of finishing on the quote.
    #[test]
    fn a_quoted_model_label_is_not_a_status_bar() {
        let (mut store, mut herdr) = api_world();
        herdr.set_screen(
            PANE,
            "Please switch to DeepSeek V4.1 Flash OpenCode Go when you can.\nBuild auto · MiniMax-M3.1-Flash MiniMax · high",
        );
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels()))]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        herdr.set_screen(PANE, &format!("Build auto · {} · high", labels().bar));
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels()))]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Done
        );
        assert_nothing_typed(&herdr);
    }

    /// F3: the variant dialog is drawn mid-screen; while it is open the bar
    /// does not confirm, even when it already names the model and the variant.
    #[test]
    fn an_open_variant_dialog_blocks_the_confirmation() {
        let (mut store, mut herdr) = api_world();
        let mut labels = labels();
        labels.variant = Some("high".into());
        let mut screen = String::from("  Select variant\n  Search\n  none\n  low\n  high\n");
        for line in 0..40 {
            screen.push_str(&format!("  conversation line {line}\n"));
        }
        screen.push_str(&format!("  Build auto · {} · high\n", labels.bar));
        herdr.set_screen(PANE, &screen);
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels))]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Waiting
        );
        // The dialog closed: the bar confirms.
        herdr.set_screen(PANE, &format!("  Build auto · {} · high\n", labels.bar));
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels))]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Done
        );
        assert_nothing_typed(&herdr);
    }

    /// F8: an observation that says idle is not enough — the agent may have
    /// started a turn since.
    #[test]
    fn a_stale_idle_observation_never_asks_the_service() {
        let (mut store, mut herdr) = api_world();
        herdr.set_status(PANE, gear_herdr::AgentStatus::Working);

        assert_eq!(run(&mut store, &mut herdr, T0), SwitchOutcome::Waiting);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }

    /// F8: another agent in the pane is never asked — the wait is bounded and
    /// the seat is flagged instead.
    #[test]
    fn a_changed_occupant_is_flagged_not_typed_into() {
        let (mut store, mut herdr) = api_world();
        herdr.add_agent(PANE, "codex", gear_herdr::AgentStatus::Idle);

        assert_eq!(run(&mut store, &mut herdr, T0), SwitchOutcome::Failed);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
        assert_nothing_typed(&herdr);
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
    }

    /// F3: a service answer that comes a tick late is waited for, not counted
    /// as a failure.
    #[test]
    fn a_late_service_answer_is_waited_for_not_failed() {
        let (mut store, mut herdr) = api_world();
        // The first call: no answer at all.
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            pending(&mut store, "p:heat").unwrap().unwrap().step,
            Step::ApiConfirm
        );
        assert_nothing_typed(&herdr);
        // The service answers now: the plan resumes instead of starting over.
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(switched_session(&labels()))]);
        herdr.set_screen(PANE, &format!("Build auto · {} · high", labels().bar));
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Done
        );
        assert_nothing_typed(&herdr);
    }

    /// T-307: a catalogue that cannot be read is flagged as unreadable, never
    /// as "the model is not in the list" — that text sends the captain after a
    /// model id that is right.
    #[test]
    fn an_unreadable_catalogue_is_not_reported_as_an_absent_model() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr
            .api_replies
            .push_back(Err("service unavailable".into()));

        let seat = idle().seat;
        let profile = gear_herdr::builtin("opencode").unwrap();
        let gate = ensure_model(
            &mut s,
            &mut herdr,
            &seat,
            &profile,
            &chain_config(),
            "minimax-cn/MiniMax-M3.1-Flash",
            T0,
        )
        .unwrap();
        let ModelGate::Failed(reason) = gate else {
            panic!("unreadable catalogue must fail: {gate:?}")
        };
        assert!(reason.contains("could not be read"), "{reason}");
        assert!(reason.contains("service unavailable"), "{reason}");
        assert!(
            !reason.contains("is not in OpenCode's model list"),
            "an unreadable list is not an absent model: {reason}"
        );
    }

    /// T-307: an empty catalogue is "not loaded yet"; a Waiting gate would keep
    /// the start pending with nothing to end it, so it fails with that reason.
    #[test]
    fn an_empty_catalogue_says_it_is_not_loaded_yet() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr
            .api_replies
            .push_back(Ok(serde_json::json!({"data": []})));

        let seat = idle().seat;
        let profile = gear_herdr::builtin("opencode").unwrap();
        let gate = ensure_model(
            &mut s,
            &mut herdr,
            &seat,
            &profile,
            &chain_config(),
            "minimax-cn/MiniMax-M3.1-Flash",
            T0,
        )
        .unwrap();
        let ModelGate::Failed(reason) = gate else {
            panic!("an empty catalogue must fail: {gate:?}")
        };
        assert!(
            reason.contains("the model list is not loaded yet"),
            "{reason}"
        );
        assert!(
            !reason.contains("is not in OpenCode's model list"),
            "an empty list is not an absent model: {reason}"
        );
    }

    #[test]
    fn a_working_agent_is_never_asked() {
        let (mut store, mut herdr) = api_world();
        let mut busy = idle();
        busy.idle = false;
        busy.status = Some(gear_herdr::AgentStatus::Working);

        let profile = gear_herdr::builtin("opencode").unwrap();
        let outcome = switch_model(
            &mut store,
            &mut herdr,
            "p:heat",
            &profile,
            Some(&busy),
            &labels(),
            "opencode-go/deepseek-v4.1-flash",
            T0,
        )
        .unwrap();
        assert_eq!(outcome, SwitchOutcome::Waiting);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }
}
