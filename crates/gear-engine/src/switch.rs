//! Changing an agent's model **in place** (T-33, T-606). OpenCode is switched
//! through its session's own API (`session.switchModel`), then the status bar
//! must show the wanted model; no key is typed at all (T-531): `/model`, the
//! picker filter and the variant dialog write the variant and the recent list
//! into `~/.local/state/opencode/model.json`, the default of every OpenCode
//! session on the machine, which is not the seat's own change to make. pi and
//! Claude Code are switched by typing `/model <id>` into their own input and
//! pressing Enter, then their footer must show the model. In every case no
//! exit, no restart: the conversation stays.
//!
//! The switch is a **state machine across ticks** (review F2/F3/F6): the plan
//! (the status-bar or footer text to reach) is stored in the seat's meta
//! *before* the first request or key, the whole visible screen is re-read
//! before every step, and a slow service or render gets a tick to answer
//! instead of burning a failed attempt. An engine restart mid-switch resumes
//! the plan instead of asking the service or typing blindly.
//!
//! Never calls into a working agent (review F8): the pane's occupant and status
//! are read again immediately before any request or key; a stale `idle` from
//! this tick's observation is not trusted.
//!
//! Three protocols are driven (T-606): OpenCode's session API (the one the SOP
//! `gear --skill agent:opencode`, ops/bin/switch-model.sh, verifies), and the
//! typed `/model <id>` command of pi and Claude Code, whose footer then shows
//! the model. Codex keeps the restart: its `/model` opens a picker with no
//! argument form GEAR can drive yet. A CLI without any of these is not
//! switched here at all: the caller starts it again on the next model
//! ([`crate::restart`]).

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

/// Can GEAR change this CLI's model **in place** (T-33, T-606)? OpenCode's
/// session API (SOP `gear --skill agent:opencode`), and the typed
/// `/model <id>` command of pi and Claude Code, whose footer shows the model.
///
/// Codex is **not** switched in place: its `/model` opens a picker with no
/// argument form GEAR can drive yet, so it is closed and started again on the
/// next model ([`crate::restart`]), the same pane and worktree, the lane's
/// task delivered again in the new session.
pub fn switches_in_place(kind: &str) -> bool {
    matches!(kind, "opencode" | "pi" | "claude")
}

/// A CLI GEAR switches by typing `/model <id>` into its own input (T-606).
fn typed_command(kind: &str) -> bool {
    matches!(kind, "pi" | "claude")
}

/// The one step a plan has left (T-531, T-606): the switch was asked for, so
/// it waits for its confirmation — `session.get` and the TUI bar for OpenCode,
/// the footer showing the model for pi and Claude. The stored text stays
/// `api-confirm` for every kind: a plan written by an earlier binary with a
/// typed step (`escape`, `command`, `filter`, `variant`, `confirm`) no longer
/// parses and is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The switch was asked for; wait for its confirmation.
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

/// Switch a seat's agent in place: OpenCode through the session API alone
/// (T-531), pi and Claude by typing `/model <id>` (T-606). The status bar or
/// footer confirms it. A durable plan prevents dispatch while a TUI render
/// lags, including across an engine restart.
///
/// Nothing here falls back to keys for OpenCode: a seat its service cannot
/// switch is `Failed` and the caller flags it. A pi or Claude command that
/// does not confirm twice is `Failed` too, and the caller restarts the seat.
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
    // (3) Only the kinds GEAR drives a protocol for are switched in place: any
    // other kind fails with nothing sent, as the in-place switch always did
    // for it.
    if !switches_in_place(&profile.kind) {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    }
    // (2) A plan written by an earlier binary with a typed step was dropped by
    // `pending`; the API path starts from the beginning.
    let resumed = pending(store, seat_id)?;
    // A turn that only retries its provider (a 429, "resource exhausted")
    // never ends on its own: waiting for it to finish would wait for ever.
    // Such a turn is interrupted below and the switch goes on.
    let backing_off = obs.is_some_and(|obs| {
        obs.flags
            .contains(&gear_herdr::profile::ScreenFlag::Backoff)
    });
    if obs.is_some_and(|obs| !obs.idle) && !backing_off {
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
    // T-606: pi and Claude take the model as a typed command; their pane is
    // all that is needed, no service session.
    if typed_command(&profile.kind) {
        return switch_by_command(
            store,
            herdr,
            seat_id,
            profile,
            labels,
            model,
            resumed.is_some(),
            &binding.pane_id,
            plan,
        );
    }
    // (1) A binding with no service session cannot be switched through the
    // API, and nothing is typed for it any more.
    let Some(session) = binding.session_ref.as_deref().filter(|id| !id.is_empty()) else {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    };
    match herdr.agent_get(&binding.pane_id) {
        Ok(Some(info)) if info.agent.as_deref() == Some("opencode") => {
            if !matches!(
                info.agent_status,
                gear_herdr::AgentStatus::Idle | gear_herdr::AgentStatus::Done
            ) {
                if info.agent_status != gear_herdr::AgentStatus::Unknown {
                    let stuck = backing_off
                        || herdr
                            .read_screen(&binding.pane_id)
                            .is_ok_and(|screen| profile.backoff(&screen));
                    if stuck {
                        // The session API stops the turn; a key does not stop
                        // OpenCode's retry loop. The next tick finds the agent
                        // idle and switches it; the lane is then told to go on.
                        let _ = herdr.opencode_api(&[
                            "session.interrupt".into(),
                            "--param".into(),
                            format!("sessionID={session}"),
                        ]);
                    }
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

/// Switch a pi or Claude seat by typing `/model <id>` into its own input
/// (T-606): the command goes in as one typed line and one Enter, and the
/// footer showing the model's bar label confirms it within [`WAIT_TICKS`].
/// Two tries without a confirmation are `Failed`: the caller falls back to the
/// restart ([`crate::restart`]).
fn switch_by_command(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    seat_id: &str,
    profile: &Profile,
    labels: &ModelSwitch,
    model: &str,
    resumed: bool,
    pane: &str,
    mut plan: Pending,
) -> GearResult<SwitchOutcome> {
    // Never types into a working agent: the pane's occupant and status are
    // read again right now, as for OpenCode.
    match herdr.agent_get(pane) {
        Ok(Some(info)) if info.agent.as_deref() == Some(profile.kind.as_str()) => {
            if !matches!(
                info.agent_status,
                gear_herdr::AgentStatus::Idle | gear_herdr::AgentStatus::Done
            ) {
                return wait_for_agent(store, seat_id, &mut plan);
            }
        }
        Ok(_) => return wait_for_agent(store, seat_id, &mut plan),
        Err(_) => return Ok(SwitchOutcome::Waiting),
    }
    let Ok(screen) = herdr.read_screen(pane) else {
        return Ok(SwitchOutcome::Waiting);
    };
    if footer_confirmed(&screen, &labels.bar, profile.tail_lines) {
        clear_pending(store, seat_id)?;
        return Ok(if resumed {
            SwitchOutcome::Done
        } else {
            SwitchOutcome::Already
        });
    }
    // The first try: the command goes in once.
    if plan.tries == 0 {
        if !type_command(herdr, pane, model) {
            return wait_for_agent(store, seat_id, &mut plan);
        }
        plan.tries = 1;
        plan.waited = 0;
        set_pending(store, seat_id, &plan)?;
        return Ok(SwitchOutcome::Waiting);
    }
    // A command was typed: give the footer [`WAIT_TICKS`] ticks to show it.
    if plan.waited < WAIT_TICKS {
        plan.waited += 1;
        set_pending(store, seat_id, &plan)?;
        return Ok(SwitchOutcome::Waiting);
    }
    // The first command did not show: one more try, then `Failed`.
    if plan.tries >= 2 {
        clear_pending(store, seat_id)?;
        return Ok(SwitchOutcome::Failed);
    }
    if !type_command(herdr, pane, model) {
        return wait_for_agent(store, seat_id, &mut plan);
    }
    plan.tries = 2;
    plan.waited = 0;
    set_pending(store, seat_id, &plan)?;
    Ok(SwitchOutcome::Waiting)
}

/// Type `/model <id>` and press Enter in one go: the text first, then the key
/// after the TUI took it (L-71). `false` when the pane refused either.
fn type_command(herdr: &mut dyn HerdrAct, pane: &str, model: &str) -> bool {
    if !matches!(
        herdr.pane_type(pane, &format!("/model {model}")),
        Outcome::Ok(())
    ) {
        return false;
    }
    herdr.type_settle();
    !matches!(
        herdr.send_keys(pane, &["enter".into()]),
        Outcome::Refused { .. }
    )
}

/// The status line/footer of the screen shows `bar`: pi and Claude print the
/// model they run on there. Only the last `tail_lines` lines are read, so a
/// conversation that quotes the model is not a confirmation.
fn footer_confirmed(screen: &str, bar: &str, tail_lines: u32) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(tail_lines as usize);
    lines[start..].iter().any(|line| line.contains(bar))
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

    /// T-114, T-606: OpenCode's session API and the typed `/model <id>` of pi
    /// and Claude are driven in place. Codex's `/model` opens a picker with no
    /// argument form GEAR can drive yet, so it is restarted.
    /// T-531: a seat of any other kind is `Failed` and nothing is sent for it.
    #[test]
    fn the_kinds_gear_switches_in_place() {
        for kind in ["opencode", "pi", "claude"] {
            assert!(switches_in_place(kind), "{kind} is switched in place");
        }
        for kind in ["codex", "grok"] {
            assert!(
                !switches_in_place(kind),
                "{kind} has no protocol GEAR drives"
            );
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
        world_kind("opencode")
    }

    fn world_kind(kind: &str) -> Store {
        let mut s = Store::open_in_memory().unwrap();
        s.write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, PANE, kind, T0)
        })
        .unwrap();
        s
    }

    /// A pi or Claude seat whose footer names another model.
    fn command_world(kind: &str) -> (Store, FakeHerdr) {
        let store = world_kind(kind);
        let mut herdr = FakeHerdr::new();
        herdr.add_agent_named(PANE, kind, gear_herdr::AgentStatus::Idle, "p-heat");
        herdr.set_screen(PANE, "── footer ──\nmodel: Old Provider");
        (store, herdr)
    }

    fn command_labels() -> ModelSwitch {
        ModelSwitch {
            picker: "New Provider".into(),
            bar: "New Provider".into(),
            variant: None,
        }
    }

    /// T-606: a pi seat is switched by one typed `/model <id>` and its Enter;
    /// the footer showing the model confirms it.
    #[test]
    fn a_pi_seat_is_switched_by_one_model_command_and_its_footer() {
        let (mut store, mut herdr) = command_world("pi");
        let profile = gear_herdr::builtin("pi").unwrap();
        assert_eq!(
            switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &command_labels(),
                "provider/new",
                T0,
            )
            .unwrap(),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            typed(&herdr),
            vec!["/model provider/new".to_string(), "enter".to_string()]
        );
        herdr.set_screen(PANE, "── footer ──\nmodel: New Provider");
        assert_eq!(
            switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &command_labels(),
                "provider/new",
                T0 + 1,
            )
            .unwrap(),
            SwitchOutcome::Done
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
    }

    /// T-606: a Claude seat is switched the same way, by `/model <id>`.
    #[test]
    fn a_claude_seat_is_switched_by_one_model_command_and_its_footer() {
        let (mut store, mut herdr) = command_world("claude");
        let profile = gear_herdr::builtin("claude").unwrap();
        assert_eq!(
            switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &command_labels(),
                "provider/new",
                T0,
            )
            .unwrap(),
            SwitchOutcome::Waiting
        );
        assert_eq!(
            typed(&herdr),
            vec!["/model provider/new".to_string(), "enter".to_string()]
        );
        herdr.set_screen(PANE, "model: New Provider");
        assert_eq!(
            switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &command_labels(),
                "provider/new",
                T0 + 1,
            )
            .unwrap(),
            SwitchOutcome::Done
        );
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
    }

    /// T-606: a footer that never shows the model is `Failed` after two tries,
    /// and the plan is cleared so the caller can restart the seat.
    #[test]
    fn a_footer_that_never_shows_the_model_fails_after_two_tries() {
        let (mut store, mut herdr) = command_world("pi");
        let profile = gear_herdr::builtin("pi").unwrap();
        let mut outcomes = Vec::new();
        for tick in 0..40 {
            let outcome = switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &command_labels(),
                "provider/new",
                T0 + tick,
            )
            .unwrap();
            outcomes.push(outcome.clone());
            if outcome != SwitchOutcome::Waiting {
                break;
            }
        }
        assert_eq!(
            outcomes.last(),
            Some(&SwitchOutcome::Failed),
            "{outcomes:?}"
        );
        let commands = typed(&herdr)
            .iter()
            .filter(|text| *text == "/model provider/new")
            .count();
        assert_eq!(commands, 2, "two tries: {:?}", herdr.calls);
        assert!(pending(&mut store, "p:heat").unwrap().is_none());
    }

    /// T-606: a footer that already shows the model sends nothing.
    #[test]
    fn a_pi_footer_already_on_the_model_sends_nothing() {
        let (mut store, mut herdr) = command_world("pi");
        herdr.set_screen(PANE, "── footer ──\nmodel: New Provider");
        let profile = gear_herdr::builtin("pi").unwrap();
        assert_eq!(
            switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&idle()),
                &command_labels(),
                "provider/new",
                T0,
            )
            .unwrap(),
            SwitchOutcome::Already
        );
        assert!(typed(&herdr).is_empty(), "{:?}", herdr.calls);
    }

    /// T-606: a busy pi agent is waited for, never typed into.
    #[test]
    fn a_busy_pi_agent_is_never_typed_into() {
        let (mut store, mut herdr) = command_world("pi");
        let profile = gear_herdr::builtin("pi").unwrap();
        let mut busy = idle();
        busy.idle = false;
        busy.status = Some(gear_herdr::AgentStatus::Working);
        assert_eq!(
            switch_model(
                &mut store,
                &mut herdr,
                "p:heat",
                &profile,
                Some(&busy),
                &command_labels(),
                "provider/new",
                T0,
            )
            .unwrap(),
            SwitchOutcome::Waiting
        );
        assert!(herdr.calls.is_empty(), "{:?}", herdr.calls);
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
    /// A turn that only retries its provider never ends: a switch waiting for
    /// it interrupts it through the session API, and the next tick switches.
    #[test]
    fn a_turn_stuck_retrying_its_provider_is_interrupted_for_the_switch() {
        let (mut store, mut herdr) = api_world();
        herdr.add_agent_named(PANE, "opencode", gear_herdr::AgentStatus::Working, "p-heat");
        herdr.set_screen(
            PANE,
            "⚠ Retrying in 3s · attempt 4 · Resource exhausted. Please try again later.",
        );
        herdr
            .api_replies
            .push_back(Ok(serde_json::json!({"interrupted": true})));
        let mut busy = idle();
        busy.idle = false;
        busy.status = Some(gear_herdr::AgentStatus::Working);
        busy.flags.insert(gear_herdr::profile::ScreenFlag::Backoff);

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
        assert!(
            herdr.calls.iter().any(|call| matches!(call,
                gear_herdr::fake::Call::OpenCodeApi { args } if args.first().map(String::as_str) == Some("session.interrupt")
                    && args.iter().any(|arg| arg == "sessionID=ses_api"))),
            "{:?}",
            herdr.calls
        );
    }
}
