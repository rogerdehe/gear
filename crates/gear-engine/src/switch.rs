//! Changing an agent's model **in place** (T-33): `/model`, the picker filter,
//! Enter — the variant dialog when it opens — then the status bar must show the
//! wanted model. No exit, no restart: the conversation stays.
//!
//! The switch is a **state machine across ticks** (review F2/F3/F6): the plan
//! (the status-bar text to reach and the next step) is stored in the seat's
//! meta *before* the first key, at most one UI submission happens per tick, and
//! the whole visible screen is re-read before every submission. A render that
//! is slow gets a tick to appear instead of burning a failed attempt, and an
//! engine restart mid-switch resumes the plan instead of retyping blindly.
//!
//! Never types into a working agent (review F8): the pane's occupant and status
//! are read again immediately before any input; a stale `idle` from this tick's
//! observation is not trusted.
//!
//! Only the OpenCode protocol is implemented: it is the one the SOP
//! (`gear --skill agent:opencode`, ops/bin/switch-model.sh) verifies, and
//! guessing another CLI's picker is worse than flagging the seat (P2). A CLI
//! without a picker GEAR drives (pi) is not switched here at all: the caller
//! starts it again on the next model ([`crate::restart`]).

use gear_app::config::ModelSwitch;
use gear_herdr::{HerdrAct, Outcome, Profile};
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
/// Marks of the model picker (SOP: a picker with "Search", "Recent", provider
/// groups). Only used to wait for it before typing the filter.
const PICKER_MARKS: [&str; 4] = ["Search", "Recent", "Select model", "No results"];
/// How many ticks one step may wait for the screen to move before it counts as
/// a failed try. A render that never comes must not hang the lane.
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

/// The keys and texts the switch sends, from the config and the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchSteps {
    /// Closes a stray dialog, a quota toast, a picker left open.
    pub escape: String,
    /// The picker command (`/model`).
    pub command: String,
    /// The default answer to a variant dialog.
    pub variant: String,
}

impl SwitchSteps {
    /// The steps of one CLI kind. Only OpenCode's protocol is verified; any
    /// other kind is not switched in place (the caller starts the agent again
    /// on the next model, or flags the seat when it cannot).
    pub fn for_kind(kind: &str, labels: &ModelSwitch) -> Option<SwitchSteps> {
        if !switches_in_place(kind) {
            return None;
        }
        Some(SwitchSteps {
            escape: "esc".to_string(),
            command: "/model".to_string(),
            variant: labels.variant.clone().unwrap_or_else(|| "high".to_string()),
        })
    }
}

/// The next UI submission of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Service accepted the target; wait for session.get and the TUI bar.
    ApiConfirm,
    /// `esc`: close whatever the last attempt (or a quota toast) left open.
    Escape,
    /// `/model`: open the picker.
    Command,
    /// The picker filter: type it and select the row (text + Enter).
    Filter,
    /// The variant dialog's answer (only for models that have variants).
    Variant,
    /// Read the status bar; the switch is done when it shows the model.
    Confirm,
}

impl Step {
    fn as_str(self) -> &'static str {
        match self {
            Step::ApiConfirm => "api-confirm",
            Step::Escape => "escape",
            Step::Command => "command",
            Step::Filter => "filter",
            Step::Variant => "variant",
            Step::Confirm => "confirm",
        }
    }

    fn parse(text: &str) -> Option<Step> {
        match text {
            "api-confirm" => Some(Step::ApiConfirm),
            "escape" => Some(Step::Escape),
            "command" => Some(Step::Command),
            "filter" => Some(Step::Filter),
            "variant" => Some(Step::Variant),
            "confirm" => Some(Step::Confirm),
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

/// Switch `seat`'s agent in place, one step per call.
///
/// `obs` is this tick's observation (used as an early gate only); `labels` are
/// the picker and status-bar texts from the config. The plan is durable, so a
/// second call continues it — including after an engine restart.
pub fn switch_in_place(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat_id: &str,
    profile: &Profile,
    obs: Option<&SeatObs>,
    labels: &ModelSwitch,
    now: Ts,
) -> GearResult<SwitchOutcome> {
    // 1. never type into a working agent (design/09). The real check is below
    //    (F8); this one keeps a busy tick from even reading the pane. `None`
    //    means "no observation this tick" (a startup check after a start).
    if let Some(obs) = obs {
        if !obs.idle {
            return Ok(SwitchOutcome::Waiting);
        }
    }
    let Some(pane) = pane_of(store, seat_id)? else {
        return Ok(SwitchOutcome::Failed);
    };
    let Some(steps) = SwitchSteps::for_kind(&profile.kind, labels) else {
        return Ok(SwitchOutcome::Failed);
    };

    // 2. a plan is written before any input (F6). A plan for another target
    //    starts over; the same target resumes where it stopped.
    let resumed = pending(store, seat_id)?;
    let mut plan = match resumed.clone() {
        Some(plan) if plan.to == labels.bar => plan,
        _ => Pending {
            to: labels.bar.clone(),
            step: Step::Escape,
            tries: 0,
            waited: 0,
        },
    };
    set_pending(store, seat_id, &plan)?;

    // 3. the pane's occupant and status again, right before any key (F8): the
    //    tick's observation may be a prompt old.
    match herdr.agent_get(&pane) {
        Ok(Some(info)) if info.agent.as_deref() == Some(profile.kind.as_str()) => {
            if !matches!(
                info.agent_status,
                gear_herdr::AgentStatus::Idle | gear_herdr::AgentStatus::Done
            ) {
                return Ok(SwitchOutcome::Waiting);
            }
        }
        // Not (yet) recognised as this kind — right after a start Herdr may
        // not have classified the agent (status unknown, no kind). Wait,
        // bounded like any other step; only a lasting mismatch fails.
        Ok(Some(_)) | Ok(None) => {
            plan.waited += 1;
            if plan.waited > WAIT_TICKS {
                return failed_try(store, seat_id, &mut plan);
            }
            set_pending(store, seat_id, &plan)?;
            return Ok(SwitchOutcome::Waiting);
        }
        Err(_) => return Ok(SwitchOutcome::Waiting),
    }

    // 4. the whole visible screen, not the tail: dialogs are drawn mid-screen
    //    (F3).
    let Ok(screen) = herdr.read_screen(&pane) else {
        return Ok(SwitchOutcome::Waiting);
    };
    let variant_open = screen.contains(VARIANT_TITLE);
    let picker_open = PICKER_MARKS.iter().any(|mark| screen.contains(mark));

    // 5. the status bar decides — but never while a variant dialog is open.
    if confirmed(&screen, &labels.bar) && !variant_open {
        clear_pending(store, seat_id)?;
        return Ok(if resumed.is_some() {
            SwitchOutcome::Done
        } else {
            SwitchOutcome::Already
        });
    }
    // M4 §10.2: the screen never says that an agent is busy — Herdr's state
    // does, and it was read above (idle or done). Nothing on the screen may
    // keep the switch from typing into an agent Herdr says is ready.

    // 6. one submission of the plan this tick. T-134: the switch types keys and
    //    text into the agent's pane, so give the agent its seat name first —
    //    after every gate, so a working agent or another occupant is not even
    //    renamed.
    if let Ok(Some(seat)) = store.read(|tx| tx.get_seat(seat_id)) {
        crate::names::ensure(herdr, &pane, &crate::names::desired(&seat));
    }
    let mut sent = false;
    match plan.step {
        Step::ApiConfirm => {
            // Only entered here when an API call failed during confirmation.
            plan.step = Step::Escape;
        }
        Step::Escape => {
            sent = keys(herdr, &pane, &steps.escape);
            if sent {
                plan.step = Step::Command;
                plan.waited = 0;
            }
        }
        Step::Command => {
            if variant_open || picker_open {
                // A dialog from a half-finished attempt: close it first.
                sent = keys(herdr, &pane, &steps.escape);
                plan.waited += 1;
            } else {
                sent = text(herdr, &pane, &steps.command);
                if sent {
                    plan.step = Step::Filter;
                    plan.waited = 0;
                }
            }
        }
        Step::Filter => {
            if variant_open {
                // The model was selected already (a render from the last try):
                // answer the dialog.
                plan.step = Step::Variant;
                plan.waited = 0;
            } else if picker_open {
                sent = text(herdr, &pane, &labels.picker);
                if sent {
                    plan.step = Step::Variant;
                    plan.waited = 0;
                }
            } else {
                // The picker has not rendered yet: give it a tick.
                plan.waited += 1;
            }
        }
        Step::Variant => {
            if variant_open {
                sent = text(herdr, &pane, &steps.variant);
            }
            // No dialog: the model has no variants; the bar is next.
            plan.step = Step::Confirm;
            plan.waited = 0;
        }
        Step::Confirm => {
            // The bar is checked at the top; waiting for it is bounded.
            plan.waited += 1;
        }
    }

    if plan.waited > WAIT_TICKS {
        return failed_try(store, seat_id, &mut plan);
    }
    if !sent && matches!(plan.step, Step::Escape | Step::Command) {
        // A refused/failed submission (or a dialog we are closing): one try
        // spent, the next tick repeats the step.
        return failed_try(store, seat_id, &mut plan);
    }
    set_pending(store, seat_id, &plan)?;
    let _ = now;
    Ok(SwitchOutcome::Waiting)
}

/// One failed try: the second one flags the seat (`Failed`). The plan goes back
/// to its first step so the retry starts from a clean dialog state.
fn failed_try(store: &mut Store, seat_id: &str, plan: &mut Pending) -> GearResult<SwitchOutcome> {
    plan.tries += 1;
    plan.step = Step::Escape;
    plan.waited = 0;
    if plan.tries >= 2 {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    }
    set_pending(store, seat_id, plan)?;
    Ok(SwitchOutcome::Waiting)
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

fn keys(herdr: &mut dyn HerdrAct, pane: &str, key: &str) -> bool {
    matches!(herdr.send_keys(pane, &[key.to_string()]), Outcome::Ok(()))
}

/// Typed text, then Enter on its own (Herdr src/app/api/panes.rs: send_input
/// wraps text in a bracketed paste when the agent enabled it, and a pasted
/// `/model` is a chat message, not a command; send_text writes raw bytes).
fn text(herdr: &mut dyn HerdrAct, pane: &str, text: &str) -> bool {
    // Typed, then Enter on its own: a pasted `/model` is not a command (L-71).
    if !matches!(herdr.pane_type(pane, text), Outcome::Ok(())) {
        return false;
    }
    // Let the TUI take the typed text before the Enter arrives. The wait
    // belongs to the terminal (T-127): a real Herdr sleeps, a fake does not.
    herdr.type_settle();
    matches!(
        herdr.send_keys(pane, &["enter".to_string()]),
        Outcome::Ok(())
    )
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
    // T-94: a model without `[models.switch]` labels takes them from
    // OpenCode's own catalogue (`<model name> <provider name>`, the text the
    // picker and the status bar show); an explicit entry still wins.
    let derived;
    let labels = match config.models.switch.get(model) {
        Some(labels) => labels,
        None => {
            // The chain names no variant (a `[models.switch]` entry is the
            // only place one lives, and it overrides this path entirely).
            // The catalogue is per directory: use the seat's own worktree (F2
            // of the T-94 review), like the start path does.
            let directory =
                store.read(|tx| tx.get_meta(&crate::onboard::worktree_path_key(&seat.id)))?;
            match crate::opencode::derived_labels(herdr, model, None, directory.as_deref()) {
                Ok(Some(labels)) => {
                    derived = labels;
                    &derived
                }
                Ok(None) => {
                    return Ok(ModelGate::Failed(format!(
                        "no [models.switch.\"{model}\"] labels (picker, bar) and {model} is not in OpenCode's model list: the agent cannot be put on the chain's model"
                    )))
                }
                // T-307: an unreadable catalogue (a failed call, a reply that
                // is not a list, an empty list not loaded yet, a provider with
                // no display name) says nothing about the model. A Waiting
                // gate would keep a start pending with nothing to end it, so
                // the seat is flagged with a text that does not send the
                // captain after a model id that is right.
                Err(reason) => {
                    return Ok(ModelGate::Failed(format!(
                        "OpenCode's model list could not be read ({reason}), so {model} could not be checked; it is not known to be absent: name [models.switch.\"{model}\"] labels (picker, bar), or check `opencode models`"
                    )))
                }
            }
        }
    };
    match switch_model(store, herdr, &seat.id, profile, None, labels, model, now)? {
        SwitchOutcome::Done | SwitchOutcome::Already => Ok(ModelGate::Ready),
        SwitchOutcome::Waiting => Ok(ModelGate::Waiting),
        SwitchOutcome::Failed => Ok(ModelGate::Failed(format!(
            "the model switch to {model} did not confirm twice"
        ))),
    }
}

/// API first for session-bound OpenCode agents. A durable API confirmation
/// plan prevents dispatch while a TUI render lags, including across restart.
/// Only a failed API call (or an old binding without a service session) enters
/// the existing typed-key protocol.
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
    let resumed = pending(store, seat_id)?;
    if profile.kind != "opencode"
        || resumed
            .as_ref()
            .is_some_and(|p| p.to == labels.bar && p.step != Step::ApiConfirm)
    {
        return switch_in_place(store, herdr, seat_id, profile, obs, labels, now);
    }
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
        return Ok(SwitchOutcome::Failed);
    };
    let Some(session) = binding.session_ref.as_deref().filter(|id| !id.is_empty()) else {
        return switch_in_place(store, herdr, seat_id, profile, obs, labels, now);
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
        // A catalogue that is not loaded yet, or no answer: the typed-key path
        // is the only thing left (it never picks another model either).
        crate::opencode::Lookup::NotLoaded | crate::opencode::Lookup::Unavailable => {
            return api_fallback(store, herdr, seat_id, profile, obs, labels, now)
        }
    };
    let get = vec!["GET".into(), format!("/api/session/{session}")];
    let value = match herdr.opencode_api(&get) {
        Ok(value) => value,
        Err(_) => return api_fallback(store, herdr, seat_id, profile, obs, labels, now),
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
                return api_fallback(store, herdr, seat_id, profile, obs, labels, now);
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

fn api_fallback(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat: &str,
    profile: &Profile,
    obs: Option<&SeatObs>,
    labels: &ModelSwitch,
    now: Ts,
) -> GearResult<SwitchOutcome> {
    clear_pending(store, seat)?;
    switch_in_place(store, herdr, seat, profile, obs, labels, now)
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

/// The pane of the seat's active binding.
fn pane_of(store: &mut Store, seat_id: &str) -> GearResult<Option<String>> {
    let seat = store.read(|tx| tx.get_seat(seat_id))?;
    let Some(seat) = seat else { return Ok(None) };
    // The binding of the seat's current generation: a start in progress has a
    // pending one, and its pane is exactly where the agent was just started.
    let binding = store.read(|tx| tx.get_binding(seat_id, seat.generation))?;
    Ok(binding.map(|binding| binding.pane_id))
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
    #[test]
    fn only_opencode_is_switched_in_place() {
        assert!(switches_in_place("opencode"));
        for kind in ["pi", "codex", "claude", "grok"] {
            assert!(!switches_in_place(kind), "{kind} has no picker GEAR drives");
            assert!(
                SwitchSteps::for_kind(kind, &labels()).is_none(),
                "{kind} has no switch steps"
            );
        }
    }

    const T0: i64 = 1_800_000_000;
    const PANE: &str = "w1:p1";
    /// An OpenCode screen that is not on the wanted model.
    const OTHER_BAR: &str = "Build auto · MiniMax-M3.1-Flash MiniMax · high";
    /// A picker (SOP: "Search", "Recent", provider groups).
    const PICKER: &str =
        "  Search\n  Recent\n  MiniMax-M3.1-Flash  MiniMax\n  DeepSeek V4.1 Flash  OpenCode Go";

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

    #[test]
    fn api_switch_confirms_session_and_bar_after_restart_without_keys() {
        let (mut store, mut herdr) = api_world();
        let mut labels = labels();
        labels.variant = Some("max".into());
        herdr.api_replies.extend([Ok(catalog()),
            Ok(serde_json::json!({"data":{"model":{"providerID":"minimax-cn", "id":"MiniMax-M3.1-Flash"}}})),
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
        let target = serde_json::json!({"data":{"model":{"providerID":"opencode-go", "id":"deepseek-v4.1-flash", "variant":"max"}}});
        // API alone is insufficient: the TUI must have rendered the variant.
        herdr
            .api_replies
            .extend([Ok(catalog()), Ok(target.clone())]);
        herdr.set_screen(PANE, &format!("┃ Build auto · {} · high", labels.bar));
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Waiting
        );
        herdr.api_replies.extend([Ok(catalog()), Ok(target)]);
        herdr.set_screen(PANE, &format!("┃ Build auto · {} · max", labels.bar));
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels),
            SwitchOutcome::Done
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
        assert!(herdr
            .calls
            .iter()
            .all(|call| matches!(call, Call::OpenCodeApi { .. })));
        let requests: Vec<_> = herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::OpenCodeApi { args } if args[0] == "session.switchModel" => Some(args),
                _ => None,
            })
            .collect();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0][2], "sessionID=ses_api");
        let body: serde_json::Value = serde_json::from_str(&requests[0][4]).unwrap();
        assert_eq!(body["model"]["variant"], "max");
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
    }

    #[test]
    fn failed_api_switch_uses_typed_fallback_only_after_failure() {
        let (mut store, mut herdr) = api_world();
        herdr.api_replies.extend([
            Ok(catalog()),
            Ok(serde_json::json!({"data":{"model":{}}})),
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
        assert!(herdr
            .calls
            .iter()
            .any(|call| matches!(call, Call::SendKeys { .. })));
        assert_eq!(
            herdr
                .calls
                .iter()
                .filter(|call| matches!(call, Call::OpenCodeApi { .. }))
                .count(),
            3
        );
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
        herdr.api_replies.extend([
            Ok(catalog()),
            Ok(serde_json::json!({"data":{"model":{
            "providerID":"opencode-go", "id":"deepseek-v4.1-flash"}}})),
        ]);
        assert_eq!(
            api_switch(&mut store, &mut herdr, &labels()),
            SwitchOutcome::Done
        );
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

    /// An OpenCode pane on another model with the scripted screens of one
    /// switch: esc clears the toast, /model opens the picker, the filter
    /// selects the row and the bar follows.
    fn switching() -> FakeHerdr {
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, "opencode", gear_herdr::AgentStatus::Idle, "p-heat");
        herdr.set_screen(PANE, "  Error: Token Plan ... (2056)\n  Session failed");
        herdr.screen_after = vec![
            ("esc".into(), "  Ask anything…".into()),
            ("/model".into(), PICKER.into()),
            (
                "DeepSeek V4.1 Flash OpenCode Go".into(),
                "Build auto · DeepSeek V4.1 Flash OpenCode Go · high".into(),
            ),
        ];
        herdr
    }

    fn texts(herdr: &FakeHerdr) -> Vec<String> {
        herdr
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::PaneType { text, .. } => Some(text.clone()),
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

    /// Tick the switch until it settles (a tick = one call).
    fn run(store: &mut Store, herdr: &mut FakeHerdr, at: Ts) -> SwitchOutcome {
        let profile = gear_herdr::builtin("opencode").unwrap();
        for tick in 0..20 {
            let outcome = switch_in_place(
                store,
                herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &labels(),
                at + tick,
            )
            .unwrap();
            if outcome != SwitchOutcome::Waiting {
                return outcome;
            }
        }
        SwitchOutcome::Waiting
    }

    #[test]
    fn the_switch_uses_the_production_sequence_one_submission_per_tick() {
        let mut s = world();
        let mut herdr = switching();

        assert_eq!(run(&mut s, &mut herdr, T0), SwitchOutcome::Done);
        // Each typed line gets its own Enter (L-71: a pasted line is not a
        // command); nothing else is pressed.
        assert_eq!(
            keys(&herdr),
            vec!["esc", "enter", "enter"],
            "{:?}",
            keys(&herdr)
        );
        assert_eq!(
            texts(&herdr),
            vec!["/model", "DeepSeek V4.1 Flash OpenCode Go"],
            "{:?}",
            texts(&herdr)
        );
        // The exact order: esc, then /model typed + Enter, then the filter
        // typed + Enter — each submission in its own tick.
        let order: Vec<String> = herdr
            .calls
            .iter()
            .map(|call| match call {
                Call::SendKeys { keys, .. } => {
                    format!("key:{}", keys.first().cloned().unwrap_or_default())
                }
                Call::PaneType { text, .. } => format!("run:{text}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            order,
            vec![
                "key:esc",
                "run:/model",
                "key:enter",
                "run:DeepSeek V4.1 Flash OpenCode Go",
                "key:enter"
            ],
            "{order:?}"
        );
        assert!(
            herdr
                .calls
                .iter()
                .all(|call| !matches!(call, Call::AgentStart { .. })),
            "no restart"
        );
        assert!(
            pending(&mut s, "p:heat").unwrap().is_none(),
            "the plan is cleared"
        );
    }

    #[test]
    fn an_already_correct_model_sends_nothing() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, "Build auto · DeepSeek V4.1 Flash OpenCode Go · high");

        assert_eq!(run(&mut s, &mut herdr, T0), SwitchOutcome::Already);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }

    /// F3: a conversation that quotes the model name is not a status bar.
    #[test]
    fn a_quoted_model_label_is_not_a_status_bar() {
        let mut s = world();
        let mut herdr = switching();
        herdr.set_screen(
            PANE,
            "Please switch to DeepSeek V4.1 Flash OpenCode Go when you can.\nBuild auto · MiniMax-M3.1-Flash MiniMax · high",
        );

        assert_eq!(run(&mut s, &mut herdr, T0), SwitchOutcome::Done);
        assert_eq!(texts(&herdr)[0], "/model", "it did not accept the quote");
    }

    /// F3: the variant dialog is drawn mid-screen; the tail window misses it.
    #[test]
    fn a_variant_dialog_above_the_tail_window_is_answered() {
        let mut s = world();
        let mut herdr = switching();
        let mut screen = String::from("  Select variant\n  Search\n  none\n  low\n  high\n");
        for line in 0..40 {
            screen.push_str(&format!("  conversation line {line}\n"));
        }
        herdr
            .screen_after
            .push(("DeepSeek V4.1 Flash OpenCode Go".into(), screen.clone()));
        // The fake matches the first scripted entry for the filter: replace it.
        herdr
            .screen_after
            .retain(|(trigger, _)| trigger != "DeepSeek V4.1 Flash OpenCode Go");
        herdr
            .screen_after
            .push(("DeepSeek V4.1 Flash OpenCode Go".into(), screen.clone()));
        // After the dialog is answered the bar confirms.
        let mut with_variant = labels();
        with_variant.variant = Some("low".into());
        herdr.screen_after.push((
            "low".into(),
            "Build auto · DeepSeek V4.1 Flash OpenCode Go · low".into(),
        ));

        let profile = gear_herdr::builtin("opencode").unwrap();
        let mut outcome = SwitchOutcome::Waiting;
        for tick in 0..20 {
            outcome = switch_in_place(
                &mut s,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &with_variant,
                T0 + tick,
            )
            .unwrap();
            if outcome != SwitchOutcome::Waiting {
                break;
            }
        }
        assert_eq!(outcome, SwitchOutcome::Done);
        let sent = texts(&herdr);
        assert!(
            sent.contains(&"low".to_string()),
            "the variant is answered: {sent:?}"
        );
    }

    /// F8: an observation that says idle is not enough — the agent may have
    /// started a turn since.
    #[test]
    fn a_stale_idle_observation_never_types_into_a_working_agent() {
        let mut s = world();
        let mut herdr = switching();
        herdr.set_status(PANE, gear_herdr::AgentStatus::Working);

        assert_eq!(run(&mut s, &mut herdr, T0), SwitchOutcome::Waiting);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }

    /// F8: another agent in the pane is never typed into.
    #[test]
    fn a_changed_occupant_is_flagged_not_typed_into() {
        let mut s = world();
        let mut herdr = switching();
        herdr.add_agent(PANE, "codex", gear_herdr::AgentStatus::Idle);

        assert_eq!(run(&mut s, &mut herdr, T0), SwitchOutcome::Failed);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }

    /// F3: a render that comes a tick late is waited for, not counted as a
    /// failure.
    #[test]
    fn a_late_picker_is_waited_for_not_failed() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, OTHER_BAR);
        // /model renders nothing the first time; the tick after shows it.
        herdr.screen_after = vec![
            ("esc".into(), OTHER_BAR.into()),
            (
                "DeepSeek V4.1 Flash OpenCode Go".into(),
                "Build auto · DeepSeek V4.1 Flash OpenCode Go · high".into(),
            ),
        ];

        let profile = gear_herdr::builtin("opencode").unwrap();
        // The escape step, then /model (no picker yet).
        switch_in_place(
            &mut s,
            &mut herdr,
            "p:heat",
            &profile,
            Some(&idle()),
            &labels(),
            T0,
        )
        .unwrap();
        switch_in_place(
            &mut s,
            &mut herdr,
            "p:heat",
            &profile,
            Some(&idle()),
            &labels(),
            T0 + 1,
        )
        .unwrap();
        assert!(texts(&herdr).contains(&"/model".to_string()));
        // The picker appears only now: the switch waits (no failure) and then
        // types the filter.
        herdr.set_screen(PANE, PICKER);
        let outcome = switch_in_place(
            &mut s,
            &mut herdr,
            "p:heat",
            &profile,
            Some(&idle()),
            &labels(),
            T0 + 2,
        )
        .unwrap();
        assert_eq!(outcome, SwitchOutcome::Waiting);
        assert!(texts(&herdr).contains(&"DeepSeek V4.1 Flash OpenCode Go".to_string()));
        let outcome = switch_in_place(
            &mut s,
            &mut herdr,
            "p:heat",
            &profile,
            Some(&idle()),
            &labels(),
            T0 + 3,
        )
        .unwrap();
        assert_eq!(outcome, SwitchOutcome::Done);
    }

    #[test]
    fn two_failed_tries_flag_the_seat() {
        let mut s = world();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(PANE, "opencode", gear_herdr::AgentStatus::Idle);
        herdr.set_screen(PANE, OTHER_BAR);

        assert_eq!(run(&mut s, &mut herdr, T0), SwitchOutcome::Failed);
        assert!(pending(&mut s, "p:heat").unwrap().is_none());
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
    fn a_working_agent_is_never_typed_into() {
        let mut s = world();
        let mut herdr = switching();
        let mut busy = idle();
        busy.idle = false;
        busy.status = Some(gear_herdr::AgentStatus::Working);

        let profile = gear_herdr::builtin("opencode").unwrap();
        let outcome = switch_in_place(
            &mut s,
            &mut herdr,
            "p:heat",
            &profile,
            Some(&busy),
            &labels(),
            T0,
        )
        .unwrap();
        assert_eq!(outcome, SwitchOutcome::Waiting);
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
    }
}
