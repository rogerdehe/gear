//! Move a lane to its new repository after the app applies a rebind.
//! The old checkout is kept; its pane is closed only after its agent exits.
use gear_herdr::{AgentStatus, HerdrAct, Outcome};
use gear_model::{GearResult, SeatRole, SeatState, Ts};
use gear_store::{BindingStatus, SeatRow, Store, Tx};
use serde_json::{json, Value};
use crate::EngineCfg;

fn marker(seat: &str) -> String { format!("lane-rebind-applied:{seat}") }
fn key(seat: &str) -> String { format!("engine.rebind.{seat}") }
pub(crate) fn fresh_key(seat: &str) -> String { format!("engine.rebind_fresh.{seat}") }

pub(crate) fn ready_to_onboard(tx: &Tx<'_>, seat: &str) -> GearResult<bool> {
    Ok(tx.get_meta(&key(seat))?.and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| v["phase"] == "ready"))
}

/// Reconcile once before ordinary onboarding. An uncertain exit is never
/// repeated blindly; the next tick observes the pane before proceeding.
pub fn tick(store: &mut Store, herdr: &mut dyn HerdrAct, cfg: &EngineCfg, now: Ts) -> GearResult<()> {
    for project in store.read(|tx| tx.project_ids())? {
        for seat in store.read(|tx| tx.seats_of_project(&project))? {
            if seat.role != SeatRole::Lane { continue }
            let state = store.read(|tx| {
                let target = tx.get_meta(&marker(&seat.id))?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
                let phase = tx.get_meta(&key(&seat.id))?.and_then(|s| serde_json::from_str::<Value>(&s).ok());
                Ok((target, phase, tx.open_attempts_of_seat(&seat.id)?))
            })?;
            let (Some(target), phase, attempts) = state else { continue };
            if !attempts.is_empty() { continue }
            let Some(repo_name) = target["repo"].as_str() else { continue };
            let current = gear_app::config::latest(store, &project)?;
            if current.repo_of_lane(&seat.name).name != repo_name { continue }
            let phase_name = phase.as_ref().and_then(|v| v["phase"].as_str()).unwrap_or("");
            let old_generation = phase.as_ref().and_then(|v| v["generation"].as_i64()).unwrap_or(seat.generation);
            if phase_name == "ready" {
                if seat.state == SeatState::Active && seat.generation > old_generation {
                    store.write(|tx| {
                        let current = tx.get_seat(&seat.id)?;
                        let binding = tx.get_binding(&seat.id, seat.generation)?;
                        let path = tx.get_meta(&crate::onboard::worktree_path_key(&seat.id))?;
                        if current.as_ref().is_some_and(|s| s.state == SeatState::Active && s.generation == seat.generation)
                            && binding.is_some_and(|b| b.status == BindingStatus::Active)
                            && path.is_some_and(|p| !p.is_empty()) {
                            tx.set_meta(&fresh_key(&seat.id), &seat.generation.to_string())?;
                            tx.set_meta(&marker(&seat.id), "")?;
                            tx.set_meta(&key(&seat.id), "")?;
                        }
                        Ok(())
                    })?;
                }
                continue;
            }
            if !matches!(seat.state, SeatState::Active | SeatState::Standby | SeatState::Missing | SeatState::Queued) { continue }
            let binding = store.read(|tx| tx.get_binding(&seat.id, seat.generation))?;
            if seat.state == SeatState::Queued && binding.as_ref().is_none_or(|b| b.status == BindingStatus::Void) {
                prepare_onboard(store, &seat, old_generation, now)?;
                continue;
            }
            let Some(binding) = binding else { continue };
            let occupant = match herdr.agent_get(&binding.pane_id) { Ok(value) => value, Err(_) => continue };
            if let Some(agent) = occupant {
                if phase_name == "exiting" { continue; }
                if agent.agent.as_deref() != Some(binding.kind.as_str())
                    || !matches!(agent.agent_status, AgentStatus::Idle | AgentStatus::Done) { continue }
                let profiles = gear_app::config::profile_table(&cfg.profiles, &current)?;
                let profile = profiles.get(&seat.profile).filter(|p| p.kind == binding.kind)
                    .or_else(|| crate::restart::profile_for_kind(&profiles, &binding.kind).map(|(_, p)| p));
                let Some(profile) = profile else { continue };
                let target = if profile.exit_command.is_some() {
                    let Ok(names) = crate::names::read(herdr) else { continue };
                    match crate::names::resolve_target(herdr, &names, &binding.pane_id, &crate::names::desired(&seat), &binding.kind) {
                        crate::names::Target::Prompt { name } => Some(name),
                        _ => continue,
                    }
                } else { None };
                if profile.exit_command.is_none() && profile.exit_keys.is_empty() { continue }
                // Reserve the move and make the old seat unreachable before
                // sending the exit. Recheck the attempt and generation atomically.
                let reserved = store.write(|tx| {
                    let Some(current) = tx.get_seat(&seat.id)? else { return Ok(false) };
                    if current.generation != seat.generation || !tx.open_attempts_of_seat(&seat.id)?.is_empty() { return Ok(false) }
                    if matches!(current.state, SeatState::Active | SeatState::Standby) {
                        tx.set_seat_state(&seat.id, SeatState::Missing, Some("moving to the new repository"), now)?;
                    } else if current.state != SeatState::Missing { return Ok(false) }
                    tx.set_meta(&key(&seat.id), &json!({"phase":"exiting","generation":old_generation}).to_string())?;
                    Ok(true)
                })?;
                if !reserved { continue }
                let result = if let (Some(name), Some(command)) = (target.as_deref(), profile.exit_command.as_deref()) {
                    herdr.prompt(name, command)
                } else { herdr.send_keys(&binding.pane_id, &profile.exit_keys) };
                if matches!(result, Outcome::Refused { .. }) {
                    store.write(|tx| tx.set_meta(&key(&seat.id), &json!({"phase":"retry","generation":old_generation}).to_string()))?;
                }
                continue;
            }
            // The agent is gone. Closing the pane does not remove the old
            // checkout; onboarding will create/open the new repo's home branch.
            match herdr.pane_close(&binding.pane_id) {
                Outcome::Ok(()) => prepare_onboard(store, &seat, old_generation, now)?,
                Outcome::Refused { code, .. } if code == "pane_not_found" || code == "not_found" => prepare_onboard(store, &seat, old_generation, now)?,
                Outcome::Refused { .. } | Outcome::Uncertain { .. } => {}
            }
        }
    }
    Ok(())
}

fn prepare_onboard(store: &mut Store, seat: &SeatRow, generation: i64, now: Ts) -> GearResult<()> {
    store.write(|tx| {
        let Some(current) = tx.get_seat(&seat.id)? else { return Ok(()) };
        if current.generation != seat.generation || !tx.open_attempts_of_seat(&seat.id)?.is_empty() { return Ok(()) }
        if matches!(current.state, SeatState::Active | SeatState::Standby) {
            tx.set_seat_state(&seat.id, SeatState::Missing, Some("moving to the new repository"), now)?;
        } else if !matches!(current.state, SeatState::Missing | SeatState::Queued) { return Ok(()) }
        if let Some(binding) = tx.get_binding(&seat.id, current.generation)? {
            if binding.status != BindingStatus::Void { tx.set_binding_status(&seat.id, current.generation, BindingStatus::Void)?; }
        }
        tx.set_meta(&crate::onboard::worktree_path_key(&seat.id), "")?;
        tx.set_meta(&key(&seat.id), &json!({"phase":"ready","generation":generation}).to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_app::{draft, identity::Fingerprint, plan, Env};
    use gear_herdr::fake::{Call, FakeHerdr};
    use gear_model::{AttemptState, GearCtx, TaskState};
    use gear_store::{testing, ContractRow, TaskRow};
    const T: i64 = 1_800_000_000;
    const PANE: &str = "w1:p2";
    const CONFIG: &str = r#"
[project]
id="p"
herdr_session="sess"
repo="/repo"
[quota]
lanes=1
temps=0
[captain]
pane="w1:p1"
kind="codex"
profile="codex"
[[lanes]]
name="heat"
duty="work"
owns=["src/"]
kind="opencode"
profile="opencode"
repo="old"
[[repos]]
name="old"
path="old"
integration="old-dev"
gate="old-gate"
[[repos]]
name="new"
path="new"
integration="new-dev"
gate="new-gate"
"#;
    fn task(id: &str, state: TaskState, repo: &str) -> TaskRow {
        TaskRow {project:"p".into(),id:id.into(),rank:1,state,priority:"must".into(),lane:Some("heat".into()),milestone:None,required:false,contract_version:1,deps:vec![],followup_of:None,wait_on:None,wait_cond:None,rework_note:None,estimate_min:Some(10),issue:None,updated:T,repo:Some(repo.into()),base:Some("old-dev".into()),gate:Some("old-gate".into()),rework_estimate_min:None}
    }
    fn world() -> (Store, FakeHerdr, EngineCfg) {
        let mut store = Store::open_in_memory().unwrap();
        store.write(|tx| {
            let mut p=testing::seed_project(tx,"p","sess",T)?; p.config_toml=CONFIG.into(); tx.update_project(&p)?;
            testing::seed_active_seat(tx,"p","captain",SeatRole::Captain,"w1:p1","codex",T)?;
            testing::seed_active_seat(tx,"p","heat",SeatRole::Lane,PANE,"opencode",T)?;
            tx.set_binding_session_ref("p:heat",1,"old-session")?;
            tx.set_meta(&crate::onboard::worktree_path_key("p:heat"),"/old-worktree")?;
            tx.insert_task(&task("T-1",TaskState::Ready,"/repo/old"))?;
            tx.insert_contract(&ContractRow {project:"p".into(),task:"T-1".into(),version:1,goal:"old".into(),accept:"done".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:T})?;
            gear_app::tasks::start_attempt(tx,"p","T-1","p:heat",1,T)?;
            gear_app::tasks::on_delivery_confirmed(tx,"p","T-1",1,T)?;
            Ok(())
        }).unwrap();
        let mut h=FakeHerdr::new(); h.add_agent_named(PANE,"opencode",AgentStatus::Idle,"p-heat"); h.set_agent_session(PANE,"old-session");
        let mut cfg=EngineCfg::default(); cfg.profiles=gear_herdr::builtins();
        (store,h,cfg)
    }
    fn request(store:&mut Store) {
        let text="<!-- gear-plan project=p base=0 -->\n# Staff\n- rebind heat repo=new owns=[src/]\n";
        let env=Env {ctx:Some(GearCtx {project:"p".into(),seat:"captain".into(),generation:1}),pane_id:Some("w1:p1".into()),admin:false};
        let fp=Fingerprint {pane_id:"w1:p1".into(),kind:Some("codex".into()),session_ref:None};
        let result=plan::apply(store,&env,Some(&fp),None,&draft::parse(text).unwrap(),text,"rebind",T+1).unwrap();
        assert!(result.warnings.contains(&"rebind heat: waits for T-1".into()));
    }
    #[test]
    fn rebind_waits_then_onboards_new_repo_and_starts_the_next_round_fresh() {
        let (mut store,mut h,cfg)=world(); request(&mut store);
        plan::reconcile_rebinds(&mut store,"p").unwrap();
        tick(&mut store,&mut h,&cfg,T+2).unwrap();
        assert!(!h.calls.iter().any(|c| matches!(c,Call::SendKeys {..}|Call::Prompt {..}|Call::PaneClose {..})));
        store.write(|tx| {
            let mut a=tx.get_attempt("p","T-1",1)?.unwrap(); a.state=AttemptState::Accepted;
            a.session_ref=Some("old-session".into()); a.session_cli=Some("opencode".into()); tx.update_attempt(&a)?;
            let mut t=tx.get_task("p","T-1")?.unwrap(); t.state=TaskState::Done; tx.update_task(&t)
        }).unwrap();
        assert_eq!(plan::reconcile_rebinds(&mut store,"p").unwrap(),["heat"]);
        h.set_status(PANE,AgentStatus::Working);
        tick(&mut store,&mut h,&cfg,T+3).unwrap();
        assert!(!h.calls.iter().any(|c| matches!(c,Call::SendKeys {..}|Call::PaneClose {..})),"working agent must finish its turn");
        h.set_status(PANE,AgentStatus::Idle);
        tick(&mut store,&mut h,&cfg,T+4).unwrap();
        tick(&mut store,&mut h,&cfg,T+5).unwrap();
        assert_eq!(h.calls.iter().filter(|c| matches!(c,Call::SendKeys {pane,..} if pane == PANE)).count(),1);
        h.exit_agent(PANE);
        tick(&mut store,&mut h,&cfg,T+6).unwrap();
        assert!(h.calls.iter().any(|c| matches!(c,Call::PaneClose {pane} if pane == PANE)));
        assert!(!h.calls.iter().any(|c| matches!(c,Call::WorktreeRemove {..})),"the old checkout is kept");
        assert_eq!(crate::onboard::onboard_queued(&mut store,&mut h,&cfg,T+7).unwrap(),["p:heat"]);
        assert!(h.calls.iter().any(|c| matches!(c,Call::WorktreeCreate {repo,branch,base,..} if repo == "/repo/new" && branch == "home/heat" && base == "new-dev")));
        tick(&mut store,&mut h,&cfg,T+8).unwrap();
        assert_eq!(store.read(|tx| tx.get_meta(&marker("p:heat"))).unwrap().as_deref(),Some(""));
        let seat=store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap(); assert_eq!(seat.generation,2);
        assert_eq!(store.read(|tx|tx.get_binding("p:heat",1)).unwrap().unwrap().status,BindingStatus::Void);
        store.write(|tx| {
            let mut new=task("T-2",TaskState::Ready,"/repo/new"); new.base=Some("new-dev".into());
            new.followup_of=Some("T-1".into()); tx.insert_task(&new)?;
            tx.insert_contract(&ContractRow {project:"p".into(),task:"T-2".into(),version:1,goal:"new".into(),accept:"done".into(),evidence_files:vec![],evidence_due_min:None,body:String::new(),created:T})?;
            tx.insert_task(&task("T-3",TaskState::Ready,"/repo/old"))?;
            Ok(())
        }).unwrap();
        let ready=store.read(|tx|crate::dispatch::ready_tasks_of(tx,&seat)).unwrap();
        assert_eq!(ready.iter().map(|t|t.id.as_str()).collect::<Vec<_>>(),["T-2"]);
        let actions = vec![(seat.clone(), crate::dispatch::Action::Assign {task:"T-2".into()})];
        let prepared = crate::dispatch::prepare_dispatch(&mut store,&mut h,&cfg,"p",&actions,T+9).unwrap();
        assert_eq!(prepared.len(), 1);
        store.write(|tx| crate::dispatch::intend_dispatch(tx,"p",&prepared,T+10).map(|_| ())).unwrap();
        crate::ops::execute_intended(&mut store,&mut h,"p",T+10).unwrap();
        let pane = store.read(|tx|tx.get_binding("p:heat",2)).unwrap().unwrap().pane_id;
        assert!(h.prompts_to(&pane).iter().any(|text| text.contains("T-2")), "new work must reach the new repo's pane");
        assert_eq!(store.read(|tx|tx.get_attempt("p","T-2",1)).unwrap().unwrap().session_action.as_deref(),Some("fresh"));
        assert_eq!(store.read(|tx|tx.get_task("p","T-1")).unwrap().unwrap().repo.as_deref(),Some("/repo/old"));
    }
}
