//! Opt-in T-44 smoke check. Requires HERDR_ENV=1 and a disposable git repo
//! with a `dev` branch. Never registers with the user's GEAR database.
//! Run: env -u GEAR_DB cargo run -p gear-engine --example opencode_live -- /tmp/scratch-repo
use gear_herdr::{AgentInfo, HerdrAct, HerdrRead, ObserveError, Outcome, SocketHerdr, Worktree};
use gear_model::{SeatRole, SeatState};
use gear_store::{ProjectRow, SeatRow, Store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(std::env::var("HERDR_ENV").as_deref(), Ok("1"));
    let repo = std::env::args()
        .nth(1)
        .expect("disposable git repo required");
    let config_text = format!(
        r#"
[project]
id = "t44live"
herdr_session = "orch"
repo = {repo:?}
[quota]
lanes = 1
temps = 0
[captain]
pane = "unused"
kind = "claude"
profile = "claude"
[[lanes]]
name = "api"
duty = "smoke check"
owns = []
kind = "opencode"
profile = "opencode"
[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash", "opencode-go/deepseek-v4.1-flash"]
[models.switch."minimax-cn/MiniMax-M3.1-Flash"]
picker = "MiniMax-M3.1-Flash"
bar = "MiniMax-M3.1-Flash MiniMax (minimax.cn)"
[models.switch."opencode-go/deepseek-v4.1-flash"]
picker = "DeepSeek V4.1 Flash OpenCode Go"
bar = "DeepSeek V4.1 Flash OpenCode Go"
variant = "max"
"#
    );
    let mut store = Store::open_in_memory().unwrap();
    store
        .write(|tx| {
            tx.insert_project(&ProjectRow {
                id: "t44live".into(),
                herdr_session: "orch".into(),
                config_toml: config_text.clone(),
                config_version: 1,
                quota_lanes: 1,
                quota_temps: 0,
                quota_includes_captain: false,
                plan_version: 0,
                created: 1_800_000_000,
            })?;
            tx.insert_seat(&SeatRow {
                id: "t44live:api".into(),
                project: "t44live".into(),
                role: SeatRole::Lane,
                name: "api".into(),
                profile: "opencode".into(),
                generation: 0,
                state: SeatState::Queued,
                state_since: 1_800_000_000,
                state_reason: None,
                task_id: None,
            })
        })
        .unwrap();
    let herdr =
        SocketHerdr::connect(std::path::Path::new(&std::env::var("HERDR_SOCKET_PATH")?)).unwrap();
    let cfg = gear_engine::EngineCfg {
        profiles: gear_herdr::builtins(),
        db_path: format!("{repo}/scratch.db"),
        ..Default::default()
    };
    let mut guarded = NoKeys(herdr);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&mut store, &mut guarded, &cfg, &config_text)
    }))
    .unwrap_or_else(|_| Err("live check attempted terminal keys".into()));
    let mut herdr = guarded.0;
    // Own scratch agent only; exit once, then let Herdr remove its worktree.
    let binding = store
        .read(|tx| {
            let seat = tx.get_seat("t44live:api")?.unwrap();
            tx.get_binding(&seat.id, seat.generation)
        })
        .unwrap();
    if let Some(binding) = binding {
        println!("cleanup pane {}", binding.pane_id);
        if result.is_err() {
            println!("failure screen: {:?}", herdr.read_screen(&binding.pane_id));
        }
        if herdr.agent_get(&binding.pane_id).ok().flatten().is_some() {
            let _ = herdr.send_keys(&binding.pane_id, &["ctrl+c".into()]);
            for _ in 0..30 {
                if herdr.agent_get(&binding.pane_id).ok().flatten().is_none() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
        let workspace = binding.pane_id.split(':').next().unwrap();
        println!("cleanup worktree: {:?}", herdr.worktree_remove(workspace));
    }
    result.map_err(Into::into)
}

fn run(
    store: &mut Store,
    herdr: &mut dyn HerdrAct,
    cfg: &gear_engine::EngineCfg,
    config: &str,
) -> Result<(), String> {
    for tick in 0..20 {
        gear_engine::onboard::onboard_queued(store, herdr, cfg, 1_800_000_001 + tick)
            .map_err(|e| e.detail)?;
        let seat = store
            .read(|tx| tx.get_seat("t44live:api"))
            .unwrap()
            .unwrap();
        if let Some(binding) = store
            .read(|tx| tx.get_binding(&seat.id, seat.generation))
            .unwrap()
        {
            println!(
                "TICK {tick}: {:?} {:?} {:?}",
                seat.state,
                gear_engine::switch::pending(store, &seat.id),
                herdr.agent_get(&binding.pane_id)
            );
        }
        if seat.state == SeatState::NeedsCheck {
            return Err(format!("start: {:?}", seat.state_reason));
        }
        if seat.state == SeatState::Active {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let seat = store
        .read(|tx| tx.get_seat("t44live:api"))
        .unwrap()
        .unwrap();
    if seat.state != SeatState::Active {
        return Err("start did not confirm".into());
    }
    let binding = store
        .read(|tx| tx.get_binding(&seat.id, seat.generation))
        .unwrap()
        .unwrap();
    let id = binding
        .session_ref
        .as_deref()
        .ok_or("start did not record API session")?;
    println!("START {} session={id}", binding.pane_id);
    evidence(herdr, &binding.pane_id, id)?;
    let config = gear_app::config::parse(config).map_err(|e| e.detail)?;
    for tick in 0..20 {
        match gear_engine::switch::ensure_model(
            store,
            herdr,
            &seat,
            &cfg.profiles["opencode"],
            &config,
            "opencode-go/deepseek-v4.1-flash",
            1_800_000_040 + tick,
        )
        .map_err(|e| e.detail)?
        {
            gear_engine::switch::ModelGate::Ready => {
                println!("SWITCH same-pane={} same-session={id}", binding.pane_id);
                evidence(herdr, &binding.pane_id, id)?;
                return Ok(());
            }
            gear_engine::switch::ModelGate::Failed(reason) => return Err(reason),
            _ => std::thread::sleep(std::time::Duration::from_secs(1)),
        }
    }
    Err("switch did not confirm".into())
}

fn evidence(herdr: &mut dyn HerdrAct, pane: &str, id: &str) -> Result<(), String> {
    let screen = herdr.read_screen(pane).map_err(|e| e.0)?;
    for line in screen.lines().filter(|line| line.contains("Build auto")) {
        println!("BAR: {}", line.trim());
    }
    println!(
        "session.get: {}",
        herdr.opencode_api(&["GET".into(), format!("/api/session/{id}")])?
    );
    Ok(())
}

/// Any picker fallback fails this live check. Teardown uses the inner door.
struct NoKeys(SocketHerdr);
impl HerdrRead for NoKeys {
    fn agents(&mut self) -> Result<Vec<AgentInfo>, ObserveError> {
        self.0.agents()
    }
    fn agent_get(&mut self, pane: &str) -> Result<Option<AgentInfo>, ObserveError> {
        self.0.agent_get(pane)
    }
    fn read_tail(&mut self, pane: &str, lines: u32) -> Result<String, ObserveError> {
        self.0.read_tail(pane, lines)
    }
    fn read_screen(&mut self, pane: &str) -> Result<String, ObserveError> {
        self.0.read_screen(pane)
    }
    fn pane_process_info(
        &mut self,
        pane: &str,
    ) -> Result<gear_herdr::PaneProcessInfo, ObserveError> {
        self.0.pane_process_info(pane)
    }
    fn panes(&mut self) -> Result<Vec<String>, ObserveError> {
        self.0.panes()
    }
    fn agent_names(
        &mut self,
    ) -> Result<std::collections::BTreeMap<String, Option<String>>, ObserveError> {
        self.0.agent_names()
    }
}
impl HerdrAct for NoKeys {
    fn opencode_api(&mut self, args: &[String]) -> Result<serde_json::Value, String> {
        println!("API {}", args.join(" "));
        let result = self.0.opencode_api(args);
        if let Err(error) = &result {
            println!("API ERROR: {error}");
        }
        result
    }
    fn prompt(&mut self, _: &str, _: &str) -> Outcome<()> {
        panic!("unexpected prompt")
    }
    fn agent_rename(&mut self, pane: &str, name: &str) -> Outcome<()> {
        self.0.agent_rename(pane, name)
    }
    fn pane_type(&mut self, _: &str, _: &str) -> Outcome<()> {
        panic!("unexpected typed keys")
    }
    /// This driver refuses typed keys, so it never settles after them.
    fn type_settle(&mut self) {}
    fn send_keys(&mut self, _: &str, _: &[String]) -> Outcome<()> {
        panic!("unexpected keys")
    }
    fn pane_run(&mut self, pane: &str, command: &str) -> Outcome<()> {
        self.0.pane_run(pane, command)
    }
    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane: &str,
        args: &[String],
    ) -> Outcome<AgentInfo> {
        println!("START REQUEST pane={pane} args={args:?}");
        self.0.agent_start(name, kind, pane, args)
    }
    fn pane_close(&mut self, pane: &str) -> Outcome<()> {
        self.0.pane_close(pane)
    }
    fn set_title(&mut self, pane: &str, title: &str) -> Outcome<()> {
        self.0.set_title(pane, title)
    }
    fn notify(&mut self, title: &str, body: &str) -> Outcome<()> {
        self.0.notify(title, body)
    }
    fn worktree_create(
        &mut self,
        repo: &str,
        branch: &str,
        base: &str,
        label: &str,
    ) -> Outcome<Worktree> {
        self.0.worktree_create(repo, branch, base, label)
    }
    fn worktree_open(&mut self, repo: &str, branch: &str, label: &str) -> Outcome<Worktree> {
        self.0.worktree_open(repo, branch, label)
    }
    /// This driver never runs a board, so it never names a tab; the call is
    /// forwarded like the others.
    fn tab_rename(&mut self, tab_id: &str, label: &str) -> Outcome<()> {
        self.0.tab_rename(tab_id, label)
    }
    fn worktree_open_path(&mut self, repo: &str, path: &str, label: &str) -> Outcome<Worktree> {
        self.0.worktree_open_path(repo, path, label)
    }
    fn worktree_remove(&mut self, workspace: &str) -> Outcome<()> {
        self.0.worktree_remove(workspace)
    }
}
