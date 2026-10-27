//! `gear` — the only binary (design/13, /14, /16).
//!
//! Agent commands: `gear`, `gear send`, `gear reply`, `gear plan apply`, `gear --skill`.
//! User commands: `gear run`, `gear admin …`, `gear --all`.
//!
//! Output contract: success → human text (or JSON with --json); failure →
//! `ERROR <CODE>: <detail>` on stderr with `ErrorCode::exit_code()`.
//! Every write command prints `request: <id>` to stderr **before** committing.

use clap::{Args, Parser, Subcommand};
use gear_model::{GearError, GearResult};

#[derive(Parser, Debug)]
#[command(name = "gear", about = "GEAR: keep an agent fleet working under one captain", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Expand a task id (T-n): its packet.
    #[arg(long, global = false)]
    show: Option<String>,
    /// Print the version-matched skill: (none)=SKILL.md | conventions | captain | course | decompose | review | adopt | github | agent:opencode | agent:codex | agent:claude | agent:grok | agent:pi.
    #[arg(long, num_args = 0..=1, default_missing_value = "main")]
    skill: Option<String>,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Report: progress | question | result | instruction | review.
    Send(SendCmd),
    /// Captain: handle exact message ids. Without a mode the item is
    /// resolved (the default) and its sender is answered; `--wait`, `--note`
    /// and `--reopen` are the other modes.
    Reply(ReplyCmd),
    /// Captain: plan operations.
    Plan {
        #[command(subcommand)]
        op: PlanOp,
    },
    /// User: the read-only plan board (PRD §15).
    Board {
        /// Draw the open tasks as a dependency tree.
        #[arg(long)]
        deps: bool,
    },
    /// Captain: reports about the project.
    Report {
        #[command(subcommand)]
        op: ReportOp,
    },
    /// User: run the engine loop (plugin pane entrypoint).
    Run,
    /// Captain: the friction reports (design §6).
    Friction(FrictionCmd),
    /// User: project/seat administration.
    Admin {
        #[command(subcommand)]
        op: AdminOp,
    },
    /// Captain: the seat itself.
    Captain {
        #[command(subcommand)]
        op: CaptainOp,
    },
    /// Who am I: the pane this call runs in, the seat bound to it, the store,
    /// and why a claim fails (design §10.7, row B9).
    Whoami,
}

#[derive(Subcommand, Debug)]
enum CaptainOp {
    /// Rebind the captain seat to another pane (a new generation).
    Handover {
        /// The pane the captain's agent runs in now.
        #[arg(long, value_name = "PANE")]
        pane: String,
    },
    /// Become a project's captain from the agent in this pane: no GEAR_CTX,
    /// no pane id. The project is the one named, else the one whose checkout
    /// holds the working directory, else the only registered one; with several,
    /// it lists them so you can ask the user which.
    Take {
        /// The project to captain (see the list `gear captain take` prints).
        project: Option<String>,
    },
}

#[derive(Args, Debug, Clone)]
struct SendCmd {
    #[arg(long)]
    kind: String,
    #[arg(long)]
    task: Option<String>,
    #[arg(long)]
    attempt: Option<i64>,
    #[arg(long)]
    contract: Option<i64>,
    #[arg(long)]
    about: Option<String>,
    #[arg(long)]
    need: Option<String>,
    #[arg(long)]
    to: Option<String>,
    #[arg(long = "reply-to")]
    reply_to: Option<String>,
    #[arg(long)]
    urgent: bool,
    #[arg(long)]
    candidate: Option<String>,
    /// Review only: the verdict a temp reviewer hands in (pass|fail).
    #[arg(long)]
    verdict: Option<String>,
    /// Review only: the ids of the findings that block (T-23), comma separated.
    #[arg(long)]
    blocking: Option<String>,
    /// Progress only (design §5.2): the minutes the lane says are left.
    #[arg(long, value_name = "MIN")]
    left: Option<i64>,
    /// Progress only (design §5.2): what stops the lane ("none" when nothing
    /// does).
    #[arg(long, value_name = "TEXT")]
    blocker: Option<String>,
    /// Blocked only (design §5.3): the task the lane waits for.
    #[arg(long, value_name = "T-N")]
    on: Option<String>,
    /// Result/review only (design §6.1): what cost you time that the captain
    /// could fix.
    #[arg(long, value_name = "TEXT")]
    friction: Option<String>,
    #[arg(long)]
    preempted: Option<String>,
    #[arg(long)]
    saved: Option<String>,
    #[arg(long = "request-id")]
    request_id: Option<String>,
    /// Message body; read from stdin when omitted.
    body: Option<String>,
}

#[derive(Args, Debug)]
struct ReplyCmd {
    /// Message ids to handle (M-…).
    #[arg(required = true)]
    ids: Vec<String>,
    /// Close the item; this is the default. The text is answered to its
    /// sender when that is a live seat.
    #[arg(long, conflicts_with_all = ["wait", "note"])]
    resolve: bool,
    /// Keep the item waiting until WHO can answer; the text is answered to its
    /// sender when that is a live seat.
    #[arg(long, value_name = "WHO")]
    wait: Option<String>,
    #[arg(long, requires = "wait")]
    until: Option<String>,
    /// Keep the item open with the text as the captain's record on it; sent to nobody.
    #[arg(long)]
    note: bool,
    /// Reopen an item that was resolved by mistake: it goes back to the
    /// captain's inbox. Nothing is sent to the seat that asked.
    #[arg(long, conflicts_with_all = ["resolve", "wait", "note"])]
    reopen: bool,
    #[arg(long)]
    downgrade: bool,
    #[arg(long = "request-id")]
    request_id: Option<String>,
    #[arg(long, short)]
    message: String,
}

#[derive(Subcommand, Debug)]
enum PlanOp {
    /// Apply a voyage plan file; the file is rewritten in place after a successful apply.
    Apply {
        path: Option<std::path::PathBuf>,
        #[arg(long, value_name = "V-ID")]
        keep: Option<String>,
        #[arg(long, requires = "keep")]
        next: Option<String>,
        #[arg(long = "request-id")]
        request_id: Option<String>,
    },
    /// Print `## NEW` blocks for the open issues the plan does not carry yet.
    ImportIssues {
        /// The voyage file to check against; default: the plan the store holds.
        path: Option<std::path::PathBuf>,
        /// Only issues carrying this label; repeatable (the config's own labels apply too).
        #[arg(long, value_name = "LABEL")]
        label: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ReportOp {
    /// The time accounting: per task and per voyage (estimate, dev time,
    /// review time, the wait for the captain, merge time, rework rounds and
    /// their time, actual over estimate).
    Time {
        /// Only this task (T-n).
        #[arg(long)]
        task: Option<String>,
        /// Only this voyage (milestone), e.g. M2.
        #[arg(long)]
        voyage: Option<String>,
    },
    /// The voyage report of the project (design §7.5): the voyage's lines,
    /// its `done:` sentence clause by clause, its time and tokens, the ★
    /// decisions, the friction reports and the tasks that took more than one
    /// round.
    Voyage {
        /// The voyage (milestone) to report on, e.g. M4. The project's
        /// current voyage when it is not given.
        #[arg(long)]
        voyage: Option<String>,
        /// Print every decision, not only the ★ ones.
        #[arg(long)]
        all: bool,
    },
}

/// `gear friction`: the friction reports of the project (design §6.3, §6.4).
#[derive(Args, Debug)]
struct FrictionCmd {
    /// What the captain did about a report.
    #[command(subcommand)]
    op: Option<FrictionOp>,
    /// Include the closed reports too.
    #[arg(long)]
    all: bool,
}

#[derive(Subcommand, Debug)]
enum FrictionOp {
    /// Close a report: `gear friction close F-<n> -m "<outcome>"`.
    Close {
        /// The report's id (F-<n>).
        id: String,
        /// What the captain did: fixed (what), lesson (which), not worth it
        /// (why).
        #[arg(long, short)]
        message: String,
    },
}

#[derive(Subcommand, Debug)]
enum AdminOp {
    /// Register a project, or apply a changed config to a registered one.
    Init { config: std::path::PathBuf },
    /// Print what this machine offers, for the captain's kickoff proposal.
    Kickoff,
    /// Write the GEAR skill where an agent CLI loads its skills.
    InstallSkill {
        /// The agent CLI: claude | codex | opencode | grok | pi.
        #[arg(long)]
        agent: String,
    },
    /// Stop the background engine (it starts again on the next `gear` call).
    Stop,
}

fn main() {
    // The store path is set after the parse: `--help` and `--version` answer
    // without touching the user's home (design/13: a read-only command writes
    // nothing).
    let cli = Cli::parse();
    default_store_path();
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ERROR {}: {}", e.code, e.detail);
            e.code.exit_code()
        }
    };
    std::process::exit(code);
}

/// Where GEAR keeps its store when `GEAR_DB` is not set: one place for every
/// project on this machine, `~/.local/state/gear/gear.db`. Set once, before
/// anything reads it, so this process, the engine it starts and the agents
/// the engine starts all agree.
fn default_store_path() {
    if std::env::var_os(gear_model::ctx::ENV_DB).is_some() {
        return;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let db = default_store_in(std::path::Path::new(&home));
        if let Some(dir) = db.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::env::set_var(gear_model::ctx::ENV_DB, db);
    }
}

/// The default store under `home`.
fn default_store_in(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".local/state/gear/gear.db")
}

fn run(cli: Cli) -> GearResult<()> {
    if let Some(topic) = cli.skill.as_deref() {
        // The skill is compiled into the binary: no engine, no store.
        return print_skill(topic);
    }
    if starts_engine(&cli.cmd) {
        ensure_engine();
    }
    match cli.cmd {
        None => cmd_home(cli.show.as_deref(), cli.json),
        Some(Cmd::Send(c)) => cmd_send(c, cli.json),
        Some(Cmd::Reply(c)) => cmd_reply(c, cli.json),
        Some(Cmd::Plan { op }) => match op {
            PlanOp::Apply { path, keep, next, request_id } => {
                cmd_plan(path, keep, next, request_id, cli.json)
            }
            PlanOp::ImportIssues { path, label } => {
                cmd_import_issues(path, label, cli.json)
            }
        },
        Some(Cmd::Board { deps }) => cmd_board(deps, cli.json),
        Some(Cmd::Report { op }) => match op {
            ReportOp::Time { task, voyage } => cmd_report_time(task, voyage, cli.json),
            ReportOp::Voyage { voyage, all } => cmd_report_voyage(voyage, all, cli.json),
        },
        Some(Cmd::Run) => cmd_run(),
        Some(Cmd::Friction(c)) => cmd_friction(c, cli.json),
        Some(Cmd::Admin { op: AdminOp::Init { config } }) => cmd_admin_init(&config, cli.json),
        Some(Cmd::Admin { op: AdminOp::Kickoff }) => cmd_admin_kickoff(cli.json),
        Some(Cmd::Admin { op: AdminOp::InstallSkill { agent } }) => {
            cmd_admin_install_skill(&agent, cli.json)
        }
        Some(Cmd::Admin { op: AdminOp::Stop }) => cmd_admin_stop(),
        Some(Cmd::Captain { op: CaptainOp::Handover { pane } }) => {
            cmd_captain_handover(&pane, cli.json)
        }
        Some(Cmd::Captain { op: CaptainOp::Take { project } }) => cmd_captain_take(project.as_deref(), cli.json),
        Some(Cmd::Whoami) => cmd_whoami(cli.json),
    }
}

/// Does this command start the engine when its lock is free (PRD §16)?
///
/// Most commands do: the project must keep moving. These do not:
/// - `run` *is* the engine, and `admin stop` stops it;
/// - `board` only observes — starting the engine would hide the "engine down"
///   warning it exists to show (PRD §15, the review of T-3);
/// - `report time` only observes (the time accounting of what the store
///   already knows);
/// - `admin kickoff` only reads — starting the engine would open (and migrate)
///   the store, and a kickoff runs before the project exists (the review of
///   T-10). `--skill` and `--help` never get here.
/// - `captain handover` moves a seat's binding: it must not start an engine
///   (T-104).
fn starts_engine(cmd: &Option<Cmd>) -> bool {
    !matches!(
        cmd,
        Some(Cmd::Run)
            | Some(Cmd::Admin { op: AdminOp::Stop })
            | Some(Cmd::Admin { op: AdminOp::Kickoff })
            | Some(Cmd::Admin { op: AdminOp::InstallSkill { .. } })
            | Some(Cmd::Board { .. })
            | Some(Cmd::Report { .. })
            | Some(Cmd::Whoami)
            | Some(Cmd::Plan { op: PlanOp::ImportIssues { .. } })
            | Some(Cmd::Captain { .. })
    )
}

/// Build `gear_app::Env` from GEAR_CTX / HERDR_PANE_ID / GEAR_ADMIN.
///
/// An absent `GEAR_CTX` means an anonymous caller (the user's own shell or an
/// unmanaged agent), and so does an empty one; a malformed one is an error.
fn env_from_process() -> GearResult<gear_app::Env> {
    env_from(|name| std::env::var(name).ok())
}

/// [`env_from_process`] over an injected variable lookup (testable).
fn env_from(vars: impl Fn(&str) -> Option<String>) -> GearResult<gear_app::Env> {
    // An empty GEAR_CTX is no seat: the tests blank it (.cargo/config.toml) so
    // a test run in a lane's shell never acts as that lane.
    let ctx = match vars(gear_model::ctx::ENV_CTX).filter(|raw| !raw.is_empty()) {
        None => None,
        Some(raw) => Some(gear_model::GearCtx::parse(&raw)?),
    };
    Ok(gear_app::Env {
        ctx,
        pane_id: vars("HERDR_PANE_ID"),
        admin: vars(gear_model::ctx::ENV_ADMIN).as_deref() == Some("1"),
    })
}

/// `gear admin kickoff` (PRD §8a): the inventory the captain turns into a
/// proposal. Read-only, so it needs no store, no seat and no GEAR_ADMIN: the
/// kickoff comes *before* the project is registered, and the captain's agent
/// has no GEAR_CTX yet.
fn cmd_admin_kickoff(json: bool) -> GearResult<()> {
    let inventory = gear_app::kickoff::Inventory::probe();
    if json {
        println!("{}", gear_app::kickoff::to_json(&inventory)?);
    } else {
        println!("{}", gear_app::kickoff::render(&inventory));
    }
    Ok(())
}

/// Refuse a **first** registration that is not the user's own (T-100).
///
/// An agent must not act as the user: a project that is not registered yet is
/// created only from the user's own shell — no `GEAR_CTX` of any project, and no
/// Herdr agent in the pane — whatever `GEAR_ADMIN=1` says. Re-applying the
/// config of an already-registered project from its captain's pane stays allowed
/// (T-86), and that is the only `init` an agent may run.
fn refuse_first_registration_by_an_agent(env: &gear_app::Env) -> GearResult<()> {
    if let Some(ctx) = &env.ctx {
        return Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            format!(
                "a project is registered from the user's own shell, never from an agent: this is seat {} of project {} (a config change for a registered project may be re-applied from its captain's pane)",
                ctx.seat_id(),
                ctx.project
            ),
        ));
    }
    // A shell left in a Herdr pane is still the user; a pane hosting an agent is
    // not. When the pane cannot be read, the answer is no.
    if let Some(pane) = env.pane_id.as_deref() {
        let mut herdr = open_herdr().map_err(|e| {
            GearError::new(
                gear_model::ErrorCode::Forbidden,
                format!("a project is registered from the user's own shell: cannot reach Herdr to read pane {pane} ({})", e.detail),
            )
        })?;
        use gear_herdr::HerdrRead as _;
        let occupant = herdr.agent_get(pane).map_err(|e| {
            GearError::new(
                gear_model::ErrorCode::Forbidden,
                format!("a project is registered from the user's own shell: cannot read pane {pane} ({})", e.0),
            )
        })?;
        if let Some(agent) = occupant {
            return Err(GearError::new(
                gear_model::ErrorCode::Forbidden,
                format!(
                    "a project is registered from the user's own shell, never from an agent: pane {pane} hosts {} — run `gear admin init` from a plain terminal",
                    agent.agent.as_deref().unwrap_or("an agent")
                ),
            ));
        }
    }
    Ok(())
}

/// `gear admin init <config>`: register a project — from the user's own shell
/// or from the captain's pane (T-86). `GEAR_ADMIN=1` is the user's consent and
/// is checked before anything is read; which seat may act on it is decided by
/// `gear_app::admin::init`, against the project the config names. The store is
/// `GEAR_DB`.
fn cmd_admin_init(config: &std::path::Path, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    if !env.admin {
        return Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            "`gear admin init` needs GEAR_ADMIN=1: the user's own shell, or the captain's pane",
        ));
    }
    let text = std::fs::read_to_string(config).map_err(|e| {
        GearError::new(gear_model::ErrorCode::BadFormat, format!("cannot read {}: {e}", config.display()))
    })?;
    let mut store = open_store()?;
    // A project that is not registered yet is the user's own act (T-100): an
    // agent may re-apply the config of its own project (T-86), never create
    // one. Refused before anything is written, and it rings nobody.
    if let Ok(parsed) = gear_app::config::parse(&text) {
        let registered =
            store.read(|tx| Ok(tx.get_project(&parsed.project.id)?.is_some()))?;
        if !registered {
            refuse_first_registration_by_an_agent(&env)?;
        }
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    // T-104: a context on a registered project is the captain's claim, and it
    // is proven against the caller's real pane before anything is written. The
    // first registration (above) and the user's own shell need no Herdr.
    let needs_proof = match &env.ctx {
        Some(ctx) => store.read(|tx| Ok(tx.get_project(&ctx.project)?.is_some()))?,
        None => false,
    };
    let mut herdr = if needs_proof { Some(open_herdr()?) } else { None };
    let ancestors = gear_app::identity::process_ancestors();
    let herdr_ref: Option<&mut dyn gear_herdr::HerdrRead> =
        herdr.as_mut().map(|herdr| herdr as &mut dyn gear_herdr::HerdrRead);
    let r = gear_app::admin::init_checked(
        &mut store,
        &env,
        &text,
        herdr_ref,
        &ancestors,
        home.as_deref(),
        now_ts(),
    )?;
    // This store becomes the Herdr session's GEAR target (design/14, L-54).
    if let (Ok(socket), Ok(db), Some(home)) = (
        std::env::var("HERDR_SOCKET_PATH"),
        std::env::var(gear_model::ctx::ENV_DB),
        std::env::var_os("HOME"),
    ) {
        let registry = targets_path_in(std::path::Path::new(&home));
        if let Err(e) = register_store(&registry, &socket, &canonical(&db)) {
            eprintln!("gear: could not register the store in {}: {e}", registry.display());
        }
    }
    if json {
        println!("{}", serde_json::to_string(&r).unwrap_or_default());
    } else {
        // The project is registered with this config version; a re-init
        // stores the next one (T-6).
        println!("project {} (config v{})", r.project, r.config_version);
        for seat in &r.seats {
            println!("seat {seat}");
        }
    }
    Ok(())
}

/// Open the store at GEAR_DB (agents) or the admin target (design/14).
fn open_store() -> GearResult<gear_store::Store> {
    open_store_at(|name| std::env::var(name).ok())
}

/// [`open_store`] over an injected variable lookup (testable).
/// The caller's identity (T-104): the library proves the whole claim — the
/// pane the caller really runs in is the claimed one, and the seat is bound to
/// it — and reports a refusal once to the captain.
fn observe_identity(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
) -> GearResult<Option<gear_app::identity::Fingerprint>> {
    let ancestors = gear_app::identity::process_ancestors();
    gear_app::identity::prove_seat(store, env, herdr, &ancestors, now_ts())
}

/// The caller's identity (T-126): `GEAR_CTX` when the seat's shell exports it,
/// else the seat whose **active binding** is on the pane this process runs in —
/// its own pane by ancestry, else `HERDR_PANE_ID`, else the one daemon-kind pane
/// of the session that is bound to a seat. A pane with no seat leaves the caller
/// anonymous (the user's own shell).
fn resolve_caller(
    store: &mut gear_store::Store,
    herdr: &mut dyn gear_herdr::HerdrRead,
    env: gear_app::Env,
) -> GearResult<gear_app::Env> {
    let ancestors = gear_app::identity::process_ancestors();
    gear_app::identity::resolve_env(store, &env, herdr, &ancestors)
}

fn open_store_at(vars: impl Fn(&str) -> Option<String>) -> GearResult<gear_store::Store> {
    let Some(path) = vars(gear_model::ctx::ENV_DB) else {
        // The admin target file (design/14) comes with slice 3.
        return Err(GearError::new(
            gear_model::ErrorCode::NoTarget,
            format!("{} is not set", gear_model::ctx::ENV_DB),
        ));
    };
    gear_store::Store::open(std::path::Path::new(&path))
}

/// Generate a request id and print `request: <id>` to stderr before any write.
fn announce_request_id(given: Option<String>) -> String {
    let id = given.unwrap_or_else(new_request_id);
    // Before any write: a lost receipt is replayed with this id (design/13).
    eprintln!("request: {id}");
    id
}

/// A fresh request id, `r-<unix-ms>-<pid>`.
fn new_request_id() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    request_id_at(now_ms, std::process::id())
}

/// Last millisecond handed out; see [`request_id_at`].
static LAST_REQUEST_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The request id for the given clock reading: always strictly later than the
/// previous one, so two calls in the same millisecond still differ.
fn request_id_at(now_ms: u64, pid: u32) -> String {
    use std::sync::atomic::Ordering;
    let mut last = LAST_REQUEST_MS.load(Ordering::Relaxed);
    let ms = loop {
        let candidate = if now_ms > last { now_ms } else { last + 1 };
        match LAST_REQUEST_MS.compare_exchange(last, candidate, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break candidate,
            Err(current) => last = current,
        }
    };
    format!("r-{ms}-{pid}")
}

fn cmd_home(show: Option<&str>, json: bool) -> GearResult<()> {
    if let Some(target) = show {
        let env = env_from_process()?;
        let mut store = open_store()?;
        let mut herdr = open_herdr()?;
        let env = resolve_caller(&mut store, &mut herdr, env)?;
        let out = run_show(&mut store, &env, &mut herdr, target, json)?;
        println!("{out}");
        return Ok(());
    }
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    run_home(&mut store, &env, &mut herdr, now_ts(), json, &mut std::io::stdout())
}

/// `gear --show <target>` over injected state: the task packet of a task id.
/// Every other target (`queue`, `draft`, a message id) is rejected by name.
fn run_show(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    target: &str,
    json: bool,
) -> GearResult<String> {
    if !is_task_id(target) {
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("--show {target} is not supported: only a task id (T-n) can be shown"),
        ));
    }
    let fp = observe_identity(store, env, herdr)?;
    let (text, project) = store.read(|tx| {
        // The identity of the home page: a registered seat of the project.
        let (ctx, _seat) = gear_app::identity::require_seat(tx, env, fp.as_ref(), None, None)?;
        let text = gear_app::plan::render_task_packet(tx, &ctx.project, target)?;
        Ok((text, ctx.project.clone()))
    })?;
    // The project's own standard travels with its task packets (T-85).
    let mut text = text;
    if let Some(standard) = project_standard(store, &project) {
        text.push_str("\n## Project standard (`gear/standard.md`)\n\n");
        text.push_str(&standard);
        text.push('\n');
    }
    if json {
        return serde_json::to_string(&text).map_err(json_error);
    }
    Ok(text)
}

/// The project's own standard (T-85): `gear/standard.md` in its checkout, when
/// it keeps one, so every lane reads the rules of *this* project and not only
/// GEAR's. A project without one — every project that has not adopted — is
/// unchanged.
fn project_standard(store: &mut gear_store::Store, project: &str) -> Option<String> {
    let config = gear_app::config::latest(store, project).ok()?;
    let path = std::path::Path::new(&config.project.repo)
        .join("gear")
        .join("standard.md");
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

/// A task id is `T-<n>`; every other `--show` target is rejected by name.
fn is_task_id(target: &str) -> bool {
    match target.strip_prefix("T-") {
        Some(number) => !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// The `gear` home page over injected state: build it, print it, and only
/// then record the view — a page the caller never saw is not a view
/// (design/13).
fn run_home(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    now: i64,
    json: bool,
    out: &mut dyn std::io::Write,
) -> GearResult<()> {
    let fp = observe_identity(store, env, herdr)?;
    let agents = if env.ctx.as_ref().is_some_and(|ctx| ctx.seat == "captain") {
        herdr.agents().ok()
    } else { None };
    let home = gear_app::home::build_with_agents(store, env, fp.as_ref(), now, agents.as_deref())?;
    if json {
        let doc = serde_json::json!({
            "text": &home.text,
            "view": {
                "project": &home.view.project,
                "seat": &home.view.seat,
                "generation": home.view.generation,
                "ts": home.view.ts,
                "shown_msgs": &home.view.shown_msgs,
                "shown_ops": &home.view.shown_ops,
                "shown_task": &home.view.shown_task,
                "shown_attempt": &home.view.shown_attempt,
                "shown_contract": &home.view.shown_contract,
            }
        });
        writeln!(out, "{}", serde_json::to_string(&doc).map_err(|e| {
            GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!("cannot serialize the home page: {e}"),
            )
        })?)
        .map_err(output_error)?;
    } else {
        writeln!(out, "{}", home.text).map_err(output_error)?;
    }
    out.flush().map_err(output_error)?;
    gear_app::home::record_view(store, &home.view)
}

/// A page that could not be printed is reported, and records no view.
fn output_error(e: std::io::Error) -> GearError {
    GearError::new(
        gear_model::ErrorCode::BadFormat,
        format!("cannot write the home page: {e}"),
    )
}

/// `gear board`: the plan board (PRD §15). The board itself lives in
/// `gear-board`; the CLI opens the store, names the project and hands the
/// terminal over.
fn cmd_board(deps: bool, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr().ok();
    // The caller's seat, when it has one: a captain in a pane sees its own
    // project's board without a GEAR_CTX (T-126).
    let env = match herdr.as_mut() {
        Some(herdr) => resolve_caller(&mut store, herdr, env)?,
        None => env,
    };
    let opts = gear_board::Options {
        // The caller's project; without a caller (the user's own shell) the
        // board shows every project in the store.
        project: env.ctx.as_ref().map(|ctx| ctx.project.clone()),
        // The engine's lock file sits next to the database.
        db: std::env::var_os(gear_model::ctx::ENV_DB).map(std::path::PathBuf::from),
        json,
        // The dependency tree instead of the queues of the lanes.
        view: if deps { gear_board::View::Deps } else { gear_board::View::Lanes },
        ..Default::default()
    };
    let mut out = std::io::stdout();
    gear_board::run(&mut store, &opts, &mut out,
        herdr.as_mut().map(|herdr| herdr as &mut dyn gear_herdr::HerdrRead))
}

/// `gear whoami`: who this call is (design §10.7, row B9) — the pane it
/// really runs in, the seat bound to it, the store it would write to, and why
/// a claim fails.
///
/// It only reads: no engine, no write.
fn cmd_whoami(json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    // `whoami` takes the ancestry in the signed form of the packet.
    let ancestors: Vec<i32> = gear_app::identity::process_ancestors()
        .into_iter()
        .map(|pid| pid as i32)
        .collect();
    let who = gear_app::identity::whoami(&mut store, &env, &mut herdr, &ancestors);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "pane": who.pane,
                "seat": who.seat,
                "store": who.store,
                "reason": who.reason,
            })
        );
        return Ok(());
    }
    print!("{who}");
    Ok(())
}

/// `gear friction`: the friction reports of the project (design §6.3, §6.4) —
/// what cost a lane or a reviewer time that the captain could fix, and what
/// the captain did about it.
fn cmd_friction(c: FrictionCmd, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let out = run_friction(&mut store, &env, &mut herdr, &c, now_ts())?;
    if json {
        println!("{}", serde_json::json!({ "friction": out }));
        return Ok(());
    }
    println!("{out}");
    Ok(())
}

/// `gear friction` body, over injected state.
fn run_friction(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    c: &FrictionCmd,
    now: i64,
) -> GearResult<String> {
    let project = env
        .ctx
        .as_ref()
        .map(|ctx| ctx.project.clone())
        .ok_or_else(|| {
            GearError::new(
                gear_model::ErrorCode::NotRegistered,
                "the friction reports are the captain's: run gear in the captain's pane",
            )
        })?;
    match &c.op {
        Some(FrictionOp::Close { id, message }) => {
            let fp = observe_identity(store, env, herdr)?;
            gear_app::friction::close(store, env, fp.as_ref(), &project, id, message, now)
        }
        None => gear_app::friction::list(store, &project, c.all),
    }
}

/// `gear report voyage`: the captain's landing report of the project (design
/// §7.5) — the voyage's lines, its `done:` sentence clause by clause, its
/// time and tokens, the ★ decisions, the friction reports and the tasks that
/// took more than one round.
///
/// Read-only, so it starts no engine.
fn cmd_report_voyage(voyage: Option<String>, all: bool, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let out = run_report_voyage(&mut store, &env, &mut herdr, voyage.as_deref(), all, json, now_ts())?;
    println!("{out}");
    Ok(())
}

/// `gear report voyage` body, over injected state.
fn run_report_voyage(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    voyage: Option<&str>,
    all: bool,
    json: bool,
    now: i64,
) -> GearResult<String> {
    let fp = observe_identity(store, env, herdr)?;
    // The voyage to report on: the one named, else the project's current one
    // (the voyage with open work first, then by id — the home page's order).
    let milestone = match voyage {
        Some(milestone) => milestone.to_string(),
        None => current_voyage(store, env)?,
    };
    let report = gear_app::voyage::build(store, env, fp.as_ref(), &milestone, all, now)?;
    if json {
        return serde_json::to_string(&report).map_err(json_error);
    }
    Ok(gear_app::voyage::render(&report))
}

/// The project's current voyage: the one with open work, else the first by id
/// (the home page's order).
fn current_voyage(store: &mut gear_store::Store, env: &gear_app::Env) -> GearResult<String> {
    let project = env
        .ctx
        .as_ref()
        .map(|ctx| ctx.project.clone())
        .ok_or_else(|| {
            GearError::new(
                gear_model::ErrorCode::NotRegistered,
                "no GEAR_CTX in the environment",
            )
        })?;
    store.read(|tx| {
        let open: std::collections::BTreeSet<String> = tx
            .tasks_of_project(&project)?
            .into_iter()
            .filter(|task| !task.state.is_terminal())
            .filter_map(|task| task.milestone)
            .collect();
        let mut voyages = tx.voyages_of_project(&project)?;
        voyages.sort_by_key(|voyage| (!open.contains(&voyage.milestone), voyage.milestone.clone()));
        voyages
            .into_iter()
            .next()
            .map(|voyage| voyage.milestone)
            .ok_or_else(|| {
                GearError::new(
                    gear_model::ErrorCode::NoTarget,
                    format!("the project {project} has no voyage"),
                )
            })
    })
}

/// `gear report time`: where the time went (T-38). The captain's view of the
/// project's time accounting: per task and per voyage — estimate, dev time,
/// review time, the wait for the captain, merge time, rework rounds and their
/// time, and actual over estimate.
///
/// Read-only, so it starts no engine.
fn cmd_report_time(task: Option<String>, voyage: Option<String>, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let filter = gear_app::time::Filter { task, voyage };
    let out = run_report_time(&mut store, &env, &mut herdr, &filter, json)?;
    println!("{out}");
    Ok(())
}

/// `gear report time` body, over injected state.
fn run_report_time(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    filter: &gear_app::time::Filter,
    json: bool,
) -> GearResult<String> {
    let fp = observe_identity(store, env, herdr)?;
    let report = gear_app::time::build(store, env, fp.as_ref(), filter, now_ts())?;
    if json {
        return serde_json::to_string(&report).map_err(json_error);
    }
    Ok(gear_app::time::render(&report))
}

fn cmd_send(c: SendCmd, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let body = match c.body.clone() {
        Some(body) => body,
        None => read_stdin()?,
    };
    // Announce before any write: a lost receipt is replayed with this id.
    let request_id = announce_request_id(c.request_id.clone());
    let c = SendCmd { request_id: Some(request_id), ..c };
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    // A hand-in must contain the project's current integration branch (T-19);
    // the gate is built here, from this process's worktree and config.
    let base = match parse_kind(&c.kind)? {
        gear_model::MsgKind::Result => Some(gear_app::base::gate_for(&mut store, &env)?),
        _ => None,
    };
    let out = run_send(&mut store, &env, &mut herdr, &c, body, base, now_ts(), json)?;
    println!("{out}");
    Ok(())
}

fn cmd_reply(c: ReplyCmd, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let request_id = announce_request_id(c.request_id.clone());
    let c = ReplyCmd { request_id: Some(request_id), ..c };
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let out = run_reply(&mut store, &env, &mut herdr, &c, now_ts(), json)?;
    println!("{out}");
    Ok(())
}

/// `gear admin install-skill --agent <claude|codex|opencode|grok|pi>`: write the
/// skill (SKILL.md and its references) into the directory the agent CLI loads
/// its skills from, and say where it was written.
///
/// A user command: an agent never writes into the user's home. It needs no
/// store, so it starts no engine.
/// `gear captain handover --pane <id>`: rebind the captain seat to another pane
/// (T-104). Allowed from the captain's own real pane or the user's own terminal
/// (a shell with no agent); a lane or reviewer asking is refused and the
/// captain is told which pane asked.
fn cmd_captain_handover(pane: &str, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let ancestors = gear_app::identity::process_ancestors();
    let cwd = std::env::current_dir().map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read the working directory: {e}"),
        )
    })?;
    let result =
        gear_app::captain::handover(&mut store, &env, &mut herdr, &ancestors, &cwd, pane, now_ts())?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "project": result.project,
                "captain": result.captain,
                "from": result.from,
                "to": result.to,
                "generation": result.generation,
                "kind": result.kind,
            })
        );
        return Ok(());
    }
    println!(
        "captain of {} handed over from pane {} to pane {} (generation {}, {})",
        result.project, result.from, result.to, result.generation, result.kind
    );
    Ok(())
}

fn cmd_captain_take(project: Option<&str>, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = match project.and_then(session_socket_of) {
        Some(path) => gear_herdr::SocketHerdr::connect(&path).map_err(|e| {
            GearError::new(gear_model::ErrorCode::NotRegistered, format!("cannot reach herdr at {}: {}", path.display(), e.0))
        })?,
        None => open_herdr()?,
    };
    let ancestors = gear_app::identity::process_ancestors();
    let cwd = std::env::current_dir().map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read the working directory: {e}"),
        )
    })?;
    let result =
        gear_app::captain::take(&mut store, &env, &mut herdr, &ancestors, &cwd, project, now_ts())?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "project": result.project,
                "captain": result.captain,
                "from": result.from,
                "to": result.to,
                "generation": result.generation,
                "kind": result.kind,
                "named": named_line(&result.named),
            })
        );
        return Ok(());
    }
    println!(
        "captain of {} taken by pane {} (generation {}, {}); it left {}",
        result.project,
        result.to,
        result.generation,
        result.kind,
        result.from.as_deref().unwrap_or("no pane recorded")
    );
    println!("agent name: {}", named_line(&result.named));
    println!("run `gear` here: this pane is the captain, no GEAR_CTX needed");
    Ok(())
}

/// What naming the new captain's agent did, in one line (T-134).
fn named_line(named: &gear_herdr::Ensured) -> &'static str {
    match named {
        gear_herdr::Ensured::Named => "already the captain's name",
        gear_herdr::Ensured::Renamed => "given to the agent now",
        gear_herdr::Ensured::NoAgent => "the pane hosts no agent yet",
        gear_herdr::Ensured::Taken { .. } => {
            "held by another pane: GEAR reports it and never steals it"
        }
        gear_herdr::Ensured::Failed(_) => "could not be given: the engine tries again next tick",
    }
}

fn cmd_admin_install_skill(agent: &str, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    if env.ctx.is_some() {
        return Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            "`gear admin install-skill` runs in the user's own shell, never in an agent",
        ));
    }
    let agent = gear_app::skill::SkillAgent::parse(agent)?;
    let home = std::env::var("HOME").map_err(|_| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            "HOME is not set: the skill goes into the user's home directory".to_string(),
        )
    })?;
    let home = std::path::PathBuf::from(home);
    let files = gear_app::skill::install(agent, &home)?;
    let dir = agent.skills_root(&home).join(gear_app::skill::SKILL_NAME);
    if json {
        let out = serde_json::json!({
            "agent": agent.as_str(),
            "dir": dir.to_string_lossy(),
            "files": files.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string(&out).unwrap_or_default());
        return Ok(());
    }
    println!("GEAR skill for {}: {}", agent.as_str(), dir.display());
    for file in &files {
        println!("wrote {}", file.display());
    }
    Ok(())
}

/// `gear plan apply` glue: env, store, Herdr, the announced request id, print.
fn cmd_plan(
    path: Option<std::path::PathBuf>,
    keep: Option<String>,
    next: Option<String>,
    request_id: Option<String>,
    json: bool,
) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let request_id = announce_request_id(request_id);
    let out = run_plan(
        &mut store,
        &env,
        &mut herdr,
        path.as_deref(),
        keep,
        next,
        &request_id,
        now_ts(),
        json,
    )?;
    println!("{out}");
    Ok(())
}

/// `gear plan import-issues`: the open issues a GitHub-mode project has not
/// turned into tasks yet, as `## NEW` blocks (T-81).
///
/// Read-only: it reads the project's `[github]` mode and its issues through the
/// forge, drops the ones the plan already names, and prints a block per issue
/// for the captain to paste into the voyage file, edit (lane, milestone,
/// estimate) and apply. It starts no engine: nothing is written.
fn cmd_import_issues(
    path: Option<std::path::PathBuf>,
    labels: Vec<String>,
    json: bool,
) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let blocks = run_import_issues(&mut store, &env, path.as_deref(), &labels)?;
    if json {
        println!("{}", serde_json::to_string(&blocks).map_err(json_error)?);
    } else {
        for block in &blocks {
            println!("{block}");
        }
    }
    Ok(())
}

/// `gear plan import-issues` body, over injected state.
///
/// Read-only, so it asks for the captain's seat in the store (no Herdr
/// fingerprint): the blocks are for the captain's plan, and nothing is written.
fn run_import_issues(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    path: Option<&std::path::Path>,
    labels: &[String],
) -> GearResult<Vec<String>> {
    let (project, _seat) = store.read(|tx| {
        let ctx = env.ctx.as_ref().ok_or_else(|| {
            GearError::new(
                gear_model::ErrorCode::NotRegistered,
                "no GEAR_CTX in the environment: `gear plan import-issues` runs in the captain's pane",
            )
        })?;
        let seat = tx.get_seat(&ctx.seat_id())?.ok_or_else(|| {
            GearError::new(
                gear_model::ErrorCode::NotRegistered,
                format!("seat {} is not registered", ctx.seat_id()),
            )
        })?;
        if seat.role != gear_model::SeatRole::Captain {
            return Err(GearError::new(
                gear_model::ErrorCode::Forbidden,
                format!("seat {} has role {}: `gear plan import-issues` is the captain's", seat.id, seat.role.as_str()),
            ));
        }
        Ok((ctx.project.clone(), seat))
    })?;
    let config = gear_app::config::latest(store, &project)?;
    let Some(github) = config.github.as_ref().and_then(|section| section.resolved()) else {
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!(
                "project {project} is not in GitHub mode: `[github]` with a `repo` is what its issues come from"
            ),
        ));
    };
    let mut filter = gear_forge::Filter {
        labels: github.issues_labels.clone(),
        // Every open issue, not `gh`'s default first 30: an issue older than the
        // newest page is exactly the one a long-lived plan is missing (T-81,
        // F1). One bounded call still returns it all.
        limit: u32::MAX,
    };
    filter.labels.extend(labels.iter().cloned());
    let forge = gear_forge::GitHub::new();
    use gear_forge::Forge as _;
    let issues = forge.issues(&github.repo, &filter).map_err(|why| {
        // No error code fits an unreachable forge; this one carries the detail
        // the captain needs (auth, quota, offline) and exits 1.
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read the issues of {}: {why}", github.repo),
        )
    })?;
    let planned = planned_issues(store, &project, path)?;
    Ok(issues
        .into_iter()
        .filter(|issue| !planned.contains(&issue.number))
        .map(|issue| gear_app::github::issue_block(&issue))
        .collect())
}

/// The issues the plan already names: the voyage file's own `issue: #<n>`
/// lines when a file is given, else the tasks the store holds (the applied
/// plan).
fn planned_issues(
    store: &mut gear_store::Store,
    project: &str,
    path: Option<&std::path::Path>,
) -> GearResult<std::collections::BTreeSet<u64>> {
    let mut planned = std::collections::BTreeSet::new();
    if let Some(path) = path {
        let text = std::fs::read_to_string(path).map_err(|e| {
            GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!("cannot read {}: {e}", path.display()),
            )
        })?;
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("issue:") else {
                continue;
            };
            if let Ok(number) = rest.trim().trim_start_matches('#').parse::<u64>() {
                planned.insert(number);
            }
        }
        return Ok(planned);
    }
    for task in store.read(|tx| tx.tasks_of_project(project))? {
        if let Some(issue) = task.issue.and_then(|n| u64::try_from(n).ok()) {
            planned.insert(issue);
        }
    }
    Ok(planned)
}

/// `gear plan apply` body, over injected state: read the draft, parse it and
/// apply it as the captain (design/05).
fn run_plan(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    path: Option<&std::path::Path>,
    keep: Option<String>,
    next: Option<String>,
    request_id: &str,
    now: i64,
    json: bool,
) -> GearResult<String> {
    if keep.is_some() || next.is_some() {
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            "--keep/--next (review conclusions) are not in slice 2".to_string(),
        ));
    }
    let Some(path) = path else {
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            "give the voyage file, e.g. gear plan apply gear/voyages/M1.md".to_string(),
        ));
    };
    let text = std::fs::read_to_string(path).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let draft = gear_app::draft::parse(&text)?;
    let fp = observe_identity(store, env, herdr)?;
    // T-133 (review F1): a `- release` of a **lane** must know whether the
    // lane's agent is really in its pane, and only the session can say so. The
    // agents are read here, before the write transaction (Herdr is never
    // touched inside one); a read that fails refuses the apply rather than
    // guessing. A `- release` of a **reviewer** needs no session fact (T-140):
    // its seat is idle or it is refused, and the agents say nothing about that.
    let releasing = draft.staff.iter().any(|item| {
        let gear_app::draft::StaffItem::Release { name, .. } = item else {
            return false;
        };
        let Some(project) = env.ctx.as_ref().map(|ctx| ctx.project.clone()) else {
            return false;
        };
        store
            .read(|tx| tx.get_seat(&format!("{project}:{name}")))
            .ok()
            .flatten()
            .map(|seat| seat.role == gear_model::SeatRole::Lane)
            .unwrap_or(false)
    });
    let agents = if releasing {
        Some(herdr.agents().map_err(|e| {
            GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!(
                    "cannot read the session's agents, so `- release` cannot tell whether a lane's agent is still running: {} — nothing was applied",
                    e.0
                ),
            )
        })?)
    } else {
        None
    };
    let result = gear_app::plan::apply(
        store,
        env,
        fp.as_ref(),
        agents.as_deref(),
        &draft,
        &text,
        request_id,
        now,
    )?;
    // The file is the voyage's plan: after a successful apply it is rewritten
    // in place (a temp file, then a rename) so it stays the current plan
    // (T-34): base= the applied version, each NEW block gets its id, the
    // Verdicts section goes (verdicts live in the store), and the staff
    // lines the apply used go with it (B5: an instruction that was carried
    // out must not be carried out again).
    let used_staff: Vec<usize> = draft
        .staff
        .iter()
        .map(gear_app::plan::staff_line)
        .collect();
    let rewritten =
        gear_app::draft::rewrite_applied(&text, result.version, &result.created, &used_staff);
    write_in_place(path, &rewritten).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!(
                "plan version {} applied, but the voyage file was not rewritten: {}",
                result.version, e.detail
            ),
        )
    })?;
    // After a successful apply: the captain hears about a lane that would
    // stand idle (T-58, ops/lessons.md 7a) and about the lanes his own tail
    // silences (B5: a lane that gets no warning is not a mystery). Printed
    // only — the apply itself is unchanged.
    // The warnings the captain reads: the apply's own (design §7.2: a decision
    // line that is not structured) and the lanes that would stand idle. The
    // text and the JSON carry the same list.
    let mut warnings: Vec<String> = result.warnings.clone();
    warnings.extend(
        store
            .read(|tx| gear_app::plan::idle_lanes(tx, &draft.project, now))?
            .iter()
            .map(|lane| lane.warning()),
    );
    let (tailed, until) = store.write(|tx| {
        let lanes = gear_app::plan::tailed_lanes(tx, &draft.project, now)?;
        let until = gear_app::plan::tail_until(tx, &draft.project)?;
        Ok((lanes, until))
    })?;
    if json {
        let mut value = serde_json::to_value(&result).map_err(json_error)?;
        value["warnings"] = serde_json::json!(warnings);
        value["tailed"] = serde_json::json!(tailed);
        return serde_json::to_string(&value).map_err(json_error);
    }
    let mut lines = render_apply(&result);
    // The warnings, one list for the text and the JSON.
    for warning in &warnings {
        lines.push('\n');
        lines.push_str(warning);
    }
    for lane in &tailed {
        lines.push('\n');
        let until = until
            .as_ref()
            .map(|until| format!(" {}", until.as_text()))
            .unwrap_or_default();
        lines.push_str(&format!(
            "tail: lane {lane} is silenced{until} (it may stand idle)"
        ));
    }
    Ok(lines)
}

/// Replace `path` with `text` through a temp file in the same directory, so a
/// reader never sees a half-written plan.
fn write_in_place(path: &std::path::Path, text: &str) -> GearResult<()> {
    let tmp = path.with_extension("gear-tmp");
    std::fs::write(&tmp, text).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot write {}: {e}", tmp.display()),
        )
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot replace {}: {e}", path.display()),
        )
    })
}

/// One line per item of an apply: version, created, contract changes, the
/// tasks the draft did not mention, and the verdicts settled.
fn render_apply(result: &gear_app::plan::ApplyResult) -> String {
    let mut lines = vec![format!("plan version {}", result.version)];
    for id in &result.created {
        lines.push(format!("created {id}"));
    }
    for (id, version) in &result.contract_changed {
        lines.push(format!("contract {id} v{version}"));
    }
    for id in &result.not_mentioned {
        lines.push(format!("not mentioned {id}"));
    }
    for settled in &result.settled {
        lines.push(format!("settled {settled}"));
    }
    for id in &result.cancelled {
        lines.push(format!("cancelled {id}"));
    }
    for id in &result.resolved {
        lines.push(format!("resolved {id}"));
    }
    lines.join("\n")
}

/// `gear send` body, over injected state (the command itself only wires this
/// to the process environment, the socket and the store).
fn run_send(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrAct,
    c: &SendCmd,
    body: String,
    base: Option<gear_app::base::BaseGate>,
    now: i64,
    json: bool,
) -> GearResult<String> {
    reject_unsupported(&[
        ("--need", c.need.is_some()),
        ("--preempted", c.preempted.is_some()),
        ("--saved", c.saved.is_some()),
    ])?;
    let kind = parse_kind(&c.kind)?;
    let args = gear_app::mail::SendArgs {
        kind,
        body,
        task: c.task.clone(),
        attempt: c.attempt,
        contract: c.contract,
        // A result's candidate (T-45): the commit or ref handed in.
        candidate: c.candidate.clone(),
        // A review's verdict (T-2): pass or fail.
        verdict: c
            .verdict
            .as_deref()
            .map(gear_app::review::Outcome::parse)
            .transpose()?,
        // A review's blocking findings (T-23).
        blocking: c.blocking.clone(),
        // A hand-in's gate (T-19): set by the caller for a result.
        base,
        about: c.about.clone(),
        to: c.to.clone(),
        reply_to: c.reply_to.clone(),
        urgent: c.urgent,
        // The minutes left and what stops the lane (design §5.2).
        left: c.left,
        blocker: c.blocker.clone(),
        // The task the lane waits for (design §5.3).
        on: c.on.clone(),
        // What cost time that the captain could fix (design §6.1).
        friction: c.friction.clone(),
        request_id: require_request_id(&c.request_id)?,
    };
    let fp = observe_identity(store, env, herdr)?;
    let result = gear_app::mail::send(store, env, fp.as_ref(), &args, now)?;
    // T-130: an urgent message, and every captain instruction to a lane whose
    // agent queues input (pi, claude), is typed into the pane before this call
    // returns. A replay already typed it once.
    let typed = if result.replayed {
        None
    } else {
        ring_recipient_now(store, herdr, &result, now)?
    };
    if json {
        let mut doc = serde_json::to_value(&result).map_err(json_error)?;
        // The answer to a hand-in travels with the receipt (design §10.7, row
        // B03-04/10).
        if kind == gear_model::MsgKind::Result && !result.replayed {
            if let Some(hand_in) = hand_in_reply(store, env, c.task.as_deref())? {
                doc["hand_in"] = serde_json::Value::String(hand_in);
            }
        }
        return serde_json::to_string(&doc).map_err(json_error);
    }
    {
        let mut line = format!(
            "Delivered to {} inbox {} (thread {})",
            result.recipient, result.msg_id, result.thread
        );
        if let Some(pane) = typed {
            line.push_str(&format!("; doorbell typed into {pane}"));
        }
        if result.replayed {
            line.push_str(" [replayed]");
        }
        // The answer to a hand-in (design §10.7, row B03-04/10): the work is
        // in, the next task comes by itself, and the task branch is not
        // pushed again. The lane reads the line where it handed in.
        if kind == gear_model::MsgKind::Result && !result.replayed {
            if let Some(hand_in) = hand_in_reply(store, env, c.task.as_deref())? {
                line.push('\n');
                line.push_str(&hand_in);
            }
        }
        Ok(line)
    }
}

/// The reply a hand-in gets (design §10.7, row B03-04/10): "Handed in. End
/// your turn: the next task comes by itself; do not push to <lane>/t<n>
/// again."
///
/// The branch is the task's own: the lane that owns the task, else the seat
/// that hands in (a lane's seat name is its lane name). `None` when the
/// hand-in names no task.
fn hand_in_reply(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    task: Option<&str>,
) -> GearResult<Option<String>> {
    let Some(task) = task else { return Ok(None) };
    let project = env.ctx.as_ref().map(|ctx| ctx.project.clone()).unwrap_or_default();
    let lane = store
        .read(|tx| tx.get_task(&project, task))
        .ok()
        .flatten()
        .and_then(|row| row.lane)
        .or_else(|| env.ctx.as_ref().map(|ctx| ctx.seat.clone()))
        .unwrap_or_default();
    let branch = gear_app::base::task_branch(&lane, task);
    Ok(Some(format!(
        "Handed in. End your turn: the next task comes by itself; do not push to {branch} again."
    )))
}

/// Type the doorbell of a message that must reach the recipient while it works
/// (T-130): an `--urgent` message, and every captain instruction to a lane
/// whose agent queues input (pi, claude).
///
/// The engine would ring the same seat on its next tick; doing it here puts the
/// text in the pane before `gear send` returns. The doorbell is recorded only
/// when Herdr took it, so a Herdr failure leaves the message stored and the
/// engine rings it on the next tick. Returns the pane the text went to.
fn ring_recipient_now(
    store: &mut gear_store::Store,
    herdr: &mut dyn gear_herdr::HerdrAct,
    result: &gear_app::mail::SendResult,
    now: i64,
) -> GearResult<Option<String>> {
    let target = store.read(|tx| {
        let Some(message) = tx.get_message(&result.project, &result.msg_id)? else {
            return Ok(None);
        };
        let Some(seat) = tx.get_seat(&message.recipient)? else { return Ok(None) };
        let Some(binding) = tx.get_binding(&seat.id, seat.generation)? else { return Ok(None) };
        let captain = tx.captain_of(&result.project)?.map(|row| row.id);
        if !gear_engine::doorbell::reaches_while_working(&message, &seat, &binding.kind, captain.as_deref()) {
            return Ok(None);
        }
        Ok(Some((seat.id, seat.generation, message.id, message.urgent)))
    })?;
    let Some((seat, generation, id, urgent)) = target else { return Ok(None) };
    gear_engine::ops::deliver_doorbell_now(
        store,
        herdr,
        &result.project,
        &seat,
        generation,
        &id,
        urgent,
        now,
    )
}

/// `gear reply` body, over injected state.
fn run_reply(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    c: &ReplyCmd,
    now: i64,
    json: bool,
) -> GearResult<String> {
    let (mode, wait_on, wait_until) = reply_mode(c, now)?;
    let args = gear_app::mail::ReplyArgs {
        ids: c.ids.clone(),
        text: c.message.clone(),
        mode,
        wait_on,
        wait_until,
        downgrade: c.downgrade,
        request_id: require_request_id(&c.request_id)?,
    };
    let fp = observe_identity(store, env, herdr)?;
    let result = gear_app::mail::reply(store, env, fp.as_ref(), &args, now)?;
    if json {
        return serde_json::to_string(&result).map_err(json_error);
    }
    // One line per item: every id that was handled, then the answers written.
    let mut lines: Vec<String> = result
        .handled
        .iter()
        .map(|(id, state)| format!("{id}: {state}"))
        .collect();
    for id in &result.replies {
        lines.push(format!("reply {id}"));
    }
    Ok(lines.join("\n"))
}

/// A slice-1 command rejects flags whose behaviour arrives later, naming the
/// flag instead of ignoring it.
fn reject_unsupported(flags: &[(&str, bool)]) -> GearResult<()> {
    for (flag, given) in flags {
        if *given {
            return Err(GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!("{flag} is not supported in slice 1"),
            ));
        }
    }
    Ok(())
}

/// The request id the glue announced; the command body always has one.
fn require_request_id(given: &Option<String>) -> GearResult<String> {
    given.clone().ok_or_else(|| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            "--request-id is required (the CLI announces one before writing)".to_string(),
        )
    })
}

/// `--kind` text → [`MsgKind`]. The CLI's kinds are the design/13 ones: a
/// reply is written by `gear reply`, never by `gear send`.
fn parse_kind(text: &str) -> GearResult<gear_model::MsgKind> {
    let kind: gear_model::MsgKind = text.parse()?;
    match kind {
        gear_model::MsgKind::Progress
        | gear_model::MsgKind::Question
        | gear_model::MsgKind::Result
        | gear_model::MsgKind::Instruction
        | gear_model::MsgKind::Review
        | gear_model::MsgKind::Blocked => Ok(kind),
        _ => Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!(
                "--kind {text} is not a `gear send` kind: progress, question, result, instruction, review or blocked"
            ),
        )),
    }
}

/// Reply mode and its wait fields: `--wait <who> [--until <t>]`, `--note`,
/// `--reopen`, or `--resolve` (the default; `--downgrade` alone also means a
/// note).
fn reply_mode(c: &ReplyCmd, now: i64) -> GearResult<(gear_app::mail::ReplyMode, Option<String>, Option<i64>)> {
    use gear_app::mail::ReplyMode;
    if c.reopen {
        return Ok((ReplyMode::Reopen, None, None));
    }
    if let Some(who) = &c.wait {
        let until = c.until.as_deref().ok_or_else(|| {
            GearError::new(gear_model::ErrorCode::BadFormat, "--wait needs --until <t>".to_string())
        })?;
        return Ok((ReplyMode::Wait, Some(who.clone()), Some(parse_until(until, now)?)));
    }
    if c.note || (c.downgrade && !c.resolve) {
        return Ok((ReplyMode::Note, None, None));
    }
    Ok((ReplyMode::Resolve, None, None))
}

/// A recheck time: `+<n>m` / `+<n>h` relative to `now`, or unix seconds.
fn parse_until(text: &str, now: i64) -> GearResult<i64> {
    let bad = || {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("--until {text:?}: expected +<n>m, +<n>h or unix seconds"),
        )
    };
    if let Some(rest) = text.strip_prefix('+') {
        let (number, seconds) = match (rest.strip_suffix('m'), rest.strip_suffix('h')) {
            (Some(number), _) => (number, 60),
            (None, Some(number)) => (number, 3600),
            (None, None) => return Err(bad()),
        };
        let n: i64 = number.parse().map_err(|_| bad())?;
        if n <= 0 {
            return Err(bad());
        }
        return Ok(now + n * seconds);
    }
    text.parse::<i64>().map_err(|_| bad())
}

/// A result that cannot be serialized is a data problem, not a command error.
fn json_error(e: serde_json::Error) -> GearError {
    GearError::new(
        gear_model::ErrorCode::BadFormat,
        format!("cannot serialize the result: {e}"),
    )
}

/// Body from stdin when the command line did not carry it.
fn read_stdin() -> GearResult<String> {
    let mut body = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut body).map_err(|e| {
        GearError::new(gear_model::ErrorCode::BadFormat, format!("cannot read stdin: {e}"))
    })?;
    Ok(body)
}

/// The Herdr client for this process: `HERDR_SOCKET_PATH`, else the default
/// session socket (design/04).
fn open_herdr() -> GearResult<gear_herdr::SocketHerdr> {
    // The project's own Herdr session first: an agent whose tools run in a
    // shared daemon (Codex's app-server) inherits HERDR_SOCKET_PATH from
    // wherever that daemon was started, which may be another session.
    let path = match project_session_socket().or_else(|| std::env::var_os("HERDR_SOCKET_PATH").map(std::path::PathBuf::from)) {
        Some(path) => path,
        None => {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
            home.join(".config/herdr/herdr.sock")
        }
    };
    gear_herdr::SocketHerdr::connect(&path).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::NotRegistered,
            format!("cannot reach herdr at {}: {}", path.display(), e.0),
        )
    })
}

/// The socket of the Herdr session of the project whose checkout holds the
/// working directory (`[project].herdr_session`), when that session's socket
/// exists. None outside a registered project.
fn project_session_socket() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let mut store = open_store().ok()?;
    let projects = store.read(|tx| tx.project_ids()).ok()?;
    for id in projects {
        let Ok(config) = gear_app::config::latest(&mut store, &id) else {
            continue;
        };
        let holds = config.repos().iter().any(|repo| {
            let path = std::path::PathBuf::from(&repo.path);
            let path = path.canonicalize().unwrap_or(path);
            cwd.starts_with(&path)
        }) || worktree_of_project(&cwd, &id);
        if !holds {
            continue;
        }
        let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
        let socket = home.join(".config/herdr/sessions").join(&config.project.herdr_session).join("herdr.sock");
        return socket.exists().then_some(socket);
    }
    None
}

/// The socket of `project`'s Herdr session, when it exists.
fn session_socket_of(project: &str) -> Option<std::path::PathBuf> {
    let mut store = open_store().ok()?;
    let config = gear_app::config::latest(&mut store, project).ok()?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let socket = home.join(".config/herdr/sessions").join(&config.project.herdr_session).join("herdr.sock");
    socket.exists().then_some(socket)
}

/// Whether `cwd` is inside one of the project's Herdr worktrees
/// (`<herdr worktrees>/<project>/…`), where its lanes and reviewers run.
fn worktree_of_project(cwd: &std::path::Path, project: &str) -> bool {
    cwd.components().collect::<Vec<_>>().windows(2).any(|pair| {
        pair[0].as_os_str().to_string_lossy().ends_with("herdr-worktrees") && pair[1].as_os_str() == project
    })
}

/// Unix seconds — the clock the store and the engine share.
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cmd_run() -> GearResult<()> {
    if let Err(why) = engine_target_ok() {
        return Err(GearError::new(gear_model::ErrorCode::Forbidden, why));
    }
    // The single-instance lock lives next to the database (design/08 startup).
    let db = std::env::var_os("GEAR_DB").ok_or_else(|| {
        GearError::new(gear_model::ErrorCode::NoTarget, "GEAR_DB is not set")
    })?;
    let dir = std::path::Path::new(&db)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let mut lock = take_engine_lock(&dir)?; // held for the life of the loop
    {
        // The holder's pid, for `gear admin stop`.
        use std::io::Write as _;
        let _ = lock.set_len(0);
        let _ = writeln!(lock, "{}", std::process::id());
    }
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    // The agents get the same database as `GEAR_DB` (design/03).
    let cfg = engine_cfg(&db.to_string_lossy());
    let mut clock = now_ts;
    let mut out = Stamped { inner: std::io::stdout(), at_line_start: true };
    let _ = std::io::Write::write_fmt(&mut out, format_args!("engine start pid={} db={}\n", std::process::id(), db.to_string_lossy()));
    run_engine(&mut store, &mut herdr, &cfg, &mut clock, None, &mut out)
}

/// Prefixes every engine log line with a UTC time, so `engine.log` can be
/// read back when something goes wrong.
struct Stamped<W: std::io::Write> {
    inner: W,
    at_line_start: bool,
}

impl<W: std::io::Write> std::io::Write for Stamped<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for line in buf.split_inclusive(|b| *b == b'\n') {
            if self.at_line_start {
                self.inner.write_all(format!("{} ", utc(now_ts())).as_bytes())?;
            }
            self.inner.write_all(line)?;
            self.at_line_start = line.ends_with(b"\n");
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for unix seconds (civil-from-days, no extra crate).
fn utc(ts: i64) -> String {
    let (days, secs) = (ts.div_euclid(86_400), ts.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// The engine's directory: next to `GEAR_DB`; `None` when it is not set.
fn engine_dir() -> Option<std::path::PathBuf> {
    let db = std::env::var_os(gear_model::ctx::ENV_DB)?;
    Some(std::path::Path::new(&db).parent().map(std::path::Path::to_path_buf).unwrap_or_default())
}

/// Start the engine in the background when nobody holds its lock. Every
/// `gear` call does this, so no one has to run the engine in a pane; a
/// failure here never fails the caller's command.
/// The machine's registry of GEAR targets (design/14): one line per Herdr
/// session, `<HERDR_SOCKET_PATH>\t<database path>`, written by `gear admin
/// init`. An engine runs only on the store registered for the Herdr session it
/// is connected to — a scratch or test store must never drive real panes
/// (L-54: a lane's scratch store started an engine that closed real panes and
/// removed a real worktree).
fn targets_path_in(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".config/gear/targets.tsv")
}

/// The store registered for `socket` in the registry at `path`.
fn registered_store(path: &std::path::Path, socket: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .find(|(s, _)| *s == socket)
        .map(|(_, db)| db.to_string())
}

/// Register `db` as the store of `socket` (one line per socket).
fn register_store(path: &std::path::Path, socket: &str, db: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut lines: Vec<String> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.split_once('\t').map(|(s, _)| s != socket).unwrap_or(false))
        .map(str::to_string)
        .collect();
    lines.push(format!("{socket}\t{db}"));
    std::fs::write(path, lines.join("\n") + "\n")
}

/// A path in its canonical form when it exists (symlinks, `..`).
fn canonical(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// Whether the engine may run on `GEAR_DB` here: a Herdr session is known and
/// `GEAR_DB` is the store registered for it. `Err` says why not.
fn engine_target_ok() -> Result<(), String> {
    let socket = std::env::var("HERDR_SOCKET_PATH").map_err(|_| "not inside Herdr (HERDR_SOCKET_PATH unset)".to_string())?;
    let db = std::env::var(gear_model::ctx::ENV_DB).map_err(|_| "GEAR_DB is not set".to_string())?;
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let registry = targets_path_in(std::path::Path::new(&home));
    match registered_store(&registry, &socket) {
        Some(reg) if canonical(&reg) == canonical(&db) => Ok(()),
        Some(reg) => Err(format!("{db} is not this Herdr session's GEAR store ({reg} is); an engine never runs on another store")),
        None => Err(format!("no GEAR store is registered for this Herdr session ({}): run `gear admin init`", registry.display())),
    }
}

fn ensure_engine() {
    // Only the registered store gets an engine (L-54); others are silent.
    if engine_target_ok().is_err() {
        return;
    }
    let Some(dir) = engine_dir() else { return };
    match take_engine_lock(&dir) {
        Ok(lock) => drop(lock), // free: release it for the engine we start
        Err(_) => return,       // already running (or unusable: `gear run` reports it)
    }
    match spawn_engine(&dir) {
        Ok(pid) => eprintln!("gear: engine started in the background (pid {pid}, log {})", dir.join("engine.log").display()),
        Err(e) => eprintln!("gear: could not start the engine: {e}"),
    }
}

/// `gear run` detached from the caller: own session, no terminal, output to
/// `engine.log`. Two racing starts are fine: the loser fails on the lock.
fn spawn_engine(dir: &std::path::Path) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt as _;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("engine.log"))?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("run")
        .env_remove(gear_model::ctx::ENV_CTX)
        .env_remove(gear_model::ctx::ENV_ADMIN)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe; it only detaches from the terminal.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    Ok(cmd.spawn()?.id())
}

/// `gear admin stop`: signal the engine named in its lock file.
fn cmd_admin_stop() -> GearResult<()> {
    let env = env_from_process()?;
    if env.ctx.is_some() {
        return Err(GearError::new(gear_model::ErrorCode::Forbidden, "agents do not stop the engine"));
    }
    let dir = engine_dir().ok_or_else(|| GearError::new(gear_model::ErrorCode::NoTarget, "GEAR_DB is not set"))?;
    if let Ok(lock) = take_engine_lock(&dir) {
        drop(lock);
        println!("engine not running");
        return Ok(());
    }
    let text = std::fs::read_to_string(dir.join("engine.lock")).unwrap_or_default();
    let pid: i32 = text.trim().parse().map_err(|_| {
        GearError::new(gear_model::ErrorCode::NoTarget, "engine is running but its pid is unknown")
    })?;
    // SAFETY: plain kill(2) on a pid read from our own lock file.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(GearError::new(gear_model::ErrorCode::NoTarget, format!("cannot stop pid {pid}")));
    }
    println!("engine stopped (pid {pid})");
    Ok(())
}

/// The engine's configuration: the defaults, every built-in agent profile and
/// the database path the onboarded agents get as `GEAR_DB`. A kind without a
/// built-in profile is not added, so its seats stay observed fail-closed
/// (design/04).
fn engine_cfg(db_path: &str) -> gear_engine::EngineCfg {
    let mut cfg = gear_engine::EngineCfg::default();
    cfg.db_path = db_path.to_string();
    // A passing review merges without the captain; a failing one waits for
    // the captain's judgement (the engine settles passes only).
    cfg.auto_settle = true;
    for kind in ["opencode", "claude", "codex", "grok", "pi"] {
        if let Some(profile) = gear_herdr::builtin(kind) {
            cfg.profiles.insert(kind.to_string(), profile);
        }
    }
    cfg
}

/// Take the engine's single-instance lock in `dir`; the caller keeps the file
/// handle alive, dropping it releases the lock.
fn take_engine_lock(dir: &std::path::Path) -> GearResult<std::fs::File> {
    std::fs::create_dir_all(dir).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot create {}: {e}", dir.display()),
        )
    })?;
    let path = dir.join("engine.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| {
            GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!("cannot open {}: {e}", path.display()),
            )
        })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            format!("engine already running (another process holds {})", path.display()),
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot lock {}: {e}", path.display()),
        )),
    }
}

/// The engine loop over an injected clock and a bounded number of ticks: the
/// startup recovery, then `tick` every `cfg.tick_s` seconds (design/08).
fn run_engine(
    store: &mut gear_store::Store,
    herdr: &mut dyn gear_herdr::HerdrAct,
    cfg: &gear_engine::EngineCfg,
    clock: &mut dyn FnMut() -> i64,
    ticks: Option<u64>,
    out: &mut dyn std::io::Write,
) -> GearResult<()> {
    run_engine_with(store, herdr, cfg, clock, ticks, out, &mut gear_engine::tick)
}

/// [`run_engine`] with the tick call injectable, so a test can drive the
/// "a failing tick does not stop the loop" path.
fn run_engine_with(
    store: &mut gear_store::Store,
    herdr: &mut dyn gear_herdr::HerdrAct,
    cfg: &gear_engine::EngineCfg,
    clock: &mut dyn FnMut() -> i64,
    ticks: Option<u64>,
    out: &mut dyn std::io::Write,
    tick: &mut dyn FnMut(
        &mut gear_store::Store,
        &mut dyn gear_herdr::HerdrAct,
        &gear_engine::EngineCfg,
        i64,
    ) -> GearResult<gear_engine::TickReport>,
) -> GearResult<()> {
    // Startup step 3: an op the previous process left Executing becomes
    // Unknown — nothing is re-sent.
    for (project, id) in gear_engine::ops::recover_on_start(store, clock())? {
        writeln!(out, "recovered {project} {id}: unknown").map_err(write_error)?;
    }
    let mut done: u64 = 0;
    loop {
        if ticks.is_some_and(|limit| done >= limit) {
            break;
        }
        match tick(store, herdr, cfg, clock()) {
            Ok(report) if report != gear_engine::TickReport::default() => {
                // Verbose while GEAR is in development (PRD §15): every id.
                writeln!(out, "tick: {report:?}").map_err(write_error)?;
            }
            Ok(_) => {}
            Err(e) => {
                // A failing tick is reported; the loop keeps going.
                writeln!(out, "tick failed: {}: {}", e.code, e.detail).map_err(write_error)?;
            }
        }
        out.flush().map_err(write_error)?;
        done += 1;
        if ticks.is_none() {
            let seconds = u64::try_from(cfg.tick_s).unwrap_or(0);
            std::thread::sleep(std::time::Duration::from_secs(seconds));
        }
    }
    Ok(())
}

/// An engine output line that could not be written is reported.
fn write_error(e: std::io::Error) -> GearError {
    GearError::new(
        gear_model::ErrorCode::BadFormat,
        format!("cannot write the engine output: {e}"),
    )
}

/// Skill topics and their embedded text live in `gear_app::skill` (T-30): the
/// skill is compiled into the binary, so the printed rules always match this
/// binary's version. The `--skill` help text must list exactly these names
/// (test `help_lists_every_skill_topic`).
fn print_skill(topic: &str) -> GearResult<()> {
    let Some(found) = gear_app::skill::TOPICS.iter().find(|t| t.name == topic) else {
        let available = gear_app::skill::TOPICS
            .iter()
            .map(|t| t.name)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("unknown skill topic {topic}; available: {available}"),
        ));
    };
    // The text is printed unchanged: no added newline, nothing trimmed.
    print!("{}", found.text);
    // `std::process::exit` skips stdout's buffer: flush it ourselves so the
    // last line is never lost.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_lists_every_skill_topic() {
        use clap::CommandFactory;
        let help = Cli::command().render_long_help().to_string();
        for topic in gear_app::skill::TOPICS {
            let shown = if topic.name == "main" { "(none)=SKILL.md" } else { topic.name };
            assert!(help.contains(shown), "--skill help misses topic {}", topic.name);
        }
    }

    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_model::{GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole};
    use gear_store::{testing, ContractRow, Store, TaskRow};
    use std::collections::BTreeMap;

    const T0: i64 = 1_800_000_000;
    const CAP_PANE: &str = "w1:p1";
    const LANE_PANE: &str = "w1:p2";

    /// Project `p` with an active captain (codex) and lane `heat` (opencode).
    fn world() -> (Store, FakeHerdr) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
        herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
        (store, herdr)
    }

    fn env(seat: &str, pane: &str) -> gear_app::Env {
        gear_app::Env {
            ctx: Some(GearCtx { project: "p".into(), seat: seat.into(), generation: 1 }),
            pane_id: Some(pane.into()),
            admin: false,
        }
    }

    /// The project with one more task: another lane is working on T-8, so a
    /// task that waits for it cannot start (T-58).
    fn world_with_waiting_dep() -> (Store, FakeHerdr) {
        let (mut store, herdr) = world();
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-8".into(),
                    rank: 1,
                    state: gear_model::TaskState::Running,
                    priority: "must".into(),
                    lane: Some("wire".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: T0,
                })
            })
            .unwrap();
        (store, herdr)
    }

    #[test]
    fn captain_snapshot_includes_the_observed_agent_state() {
        let (mut store, mut herdr) = world();
        herdr.set_status(LANE_PANE, AgentStatus::Working);
        let mut out = Vec::new();
        run_home(&mut store, &env("captain", CAP_PANE), &mut herdr, T0, false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("heat · agent working · no current task · 0 queued"), "{text}");
    }

    /// The world with a task the lane works on, so a hand-in goes through.
    fn world_with_task() -> (Store, FakeHerdr) {
        let (mut store, herdr) = world();
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: gear_model::TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "g".into(),
                    accept: "a".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)
            })
            .unwrap();
        (store, herdr)
    }

    fn send_cmd(kind: &str) -> SendCmd {
        SendCmd {
            kind: kind.into(),
            task: None,
            attempt: None,
            contract: None,
            about: None,
            need: None,
            to: None,
            reply_to: None,
            urgent: false,
            left: None,
            blocker: None,
            on: None,
            friction: None,
            candidate: None,
            verdict: None,
            blocking: None,
            preempted: None,
            saved: None,
            request_id: Some("r-1".into()),
            body: None,
        }
    }

    fn reply_cmd(ids: &[&str]) -> ReplyCmd {
        ReplyCmd {
            ids: ids.iter().map(|id| id.to_string()).collect(),
            resolve: false,
            wait: None,
            until: None,
            note: false,
            reopen: false,
            downgrade: false,
            request_id: Some("r-2".into()),
            message: "Use v2".into(),
        }
    }

    #[test]
    fn send_maps_kind_and_stores() {
        let (mut store, mut herdr) = world();
        let c = send_cmd("question");
        let out = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Schema v1 or v2?".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        assert!(out.contains("Delivered to p:captain"), "{out}");
        assert!(out.contains("M-1"), "{out}");
        let stored = store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
        assert_eq!(stored.kind, MsgKind::Question);
        assert_eq!(stored.body, "Schema v1 or v2?");
    }

    #[test]
    fn unknown_kind_is_bad_format() {
        let (mut store, mut herdr) = world();
        let err = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &send_cmd("nonsense"),
            "x".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
    }

    #[test]
    fn unsupported_flag_is_bad_format() {
        // Every flag whose behaviour is slice 2+ is rejected by name, never
        // silently dropped.
        let flags: [(&str, fn(&mut SendCmd)); 3] = [
            ("--need", |c| c.need = Some("a decision".into())),
            ("--preempted", |c| c.preempted = Some("T-3".into())),
            ("--saved", |c| c.saved = Some("branch".into())),
        ];
        for (flag, set) in flags {
            let (mut store, mut herdr) = world();
            let mut c = send_cmd("question");
            set(&mut c);
            let err =
                run_send(&mut store, &env("heat", LANE_PANE), &mut herdr, &c, "x".into(), None, T0 + 1, false)
                    .unwrap_err();
            assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{flag}");
            assert!(err.detail.contains(flag), "{flag}: {}", err.detail);
        }
    }

    /// Write `text` to a unique temp file and return its path.
    fn write_draft(text: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir()
            .join(format!("gear-draft-{}-{nanos}.md", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn plan_draft() -> String {
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n"
            .to_string()
    }

    #[test]
    fn plan_apply_creates_tasks() {
        let (mut store, mut herdr) = world();
        let path = write_draft(&plan_draft());

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap();

        assert!(out.contains("plan version 1"), "{out}");
        assert!(out.contains("created T-1"), "{out}");
        // The lane has a task it can start now: nothing to tell the captain.
        assert!(!out.contains("warning:"), "{out}");
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        assert_eq!(tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["T-1"]);
    }

    /// A task that passed and whose merge failed: the captain retries it with
    /// `- remerge T-1`.
    fn world_with_a_failed_merge() -> (Store, FakeHerdr) {
        let (mut store, herdr) = world();
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    project: "p".into(),
                    id: "T-1".into(),
                    rank: 1,
                    state: gear_model::TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                    issue: None,
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    version: 1,
                    goal: "g".into(),
                    accept: "a".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                gear_app::tasks::submit_result(tx, "p", "T-1", 1, 1, "abc1234", "report", T0)?;
                // The review passed: the attempt is accepted and the task done.
                tx.insert_review_verdict(&gear_store::ReviewVerdictRow {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    passed: true,
                    findings: "all good".into(),
                    by_seat: "p:review-1".into(),
                    created: T0,
                })?;
                gear_app::tasks::settle_review(tx, "p", "T-1", 1, T0)?;
                // The merge failed.
                let op = tx.insert_op(&gear_store::NewOp {
                    project: "p".into(),
                    seat: "p:heat".into(),
                    generation: 1,
                    kind: gear_model::OpKind::Merge,
                    task: Some("T-1".into()),
                    attempt: Some(1),
                    ref_msg: None,
                    plan_version: None,
                    payload: serde_json::json!({}),
                    created: T0,
                })?;
                tx.set_op_state("p", &op.id, gear_model::OpState::Executing, None, T0)?;
                tx.set_op_state("p", &op.id, gear_model::OpState::Failed, Some("conflict"), T0)
            })
            .unwrap();
        (store, herdr)
    }

    /// B5: a staff line the apply used is gone from the plan file. A
    /// `- remerge T-<n>` line that was carried out must not be carried out
    /// again — the second apply would refuse it.
    #[test]
    fn an_applied_remerge_line_is_gone_from_the_plan_file() {
        let (mut store, mut herdr) = world_with_a_failed_merge();
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Staff\n- remerge T-1\n");

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 10,
            false,
        )
        .unwrap();

        assert!(out.contains("plan version 1"), "{out}");
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains("remerge"), "the line is gone: {rewritten}");
        assert!(!rewritten.contains("# Staff"), "the empty section is gone: {rewritten}");
    }

    /// B5: the apply says which lanes the captain's own tail silences, so a
    /// lane that gets no warning is not a mystery.
    #[test]
    fn the_apply_prints_the_tailed_lanes() {
        let (mut store, mut herdr) = world();
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Decisions\n- tail: heat\n");

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 10,
            false,
        )
        .unwrap();

        assert!(
            out.contains("tail: lane heat is silenced"),
            "the tailed lane is named: {out}"
        );
    }

    /// T-140: the captain's `- release <reviewer>` line releases a reviewer
    /// seat in Standby, through the real command.
    #[test]
    fn plan_apply_releases_a_standby_reviewer() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w1:p5", "codex", T0)?;
                tx.set_seat_state("p:review-1", gear_model::SeatState::Standby, Some("review done"), T0 + 1)
            })
            .unwrap();
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Staff\n- release review-1\n");

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 2,
            false,
        )
        .unwrap();

        assert!(out.contains("plan version 1"), "{out}");
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, gear_model::SeatState::Releasing);
    }

    /// T-58: after a successful apply the captain hears about a lane that
    /// would stand idle — its only queued task waits for a dependency that is
    /// not done.
    #[test]
    fn plan_apply_warns_about_a_lane_that_would_stand_idle() {
        let (mut store, mut herdr) = world_with_waiting_dep();
        let draft = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: T-8\n";
        let path = write_draft(draft);

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap();

        assert!(out.contains("created T-1"), "{out}");
        assert!(
            out.contains("warning: lane heat has nothing to start after now (1 queued behind deps: T-1)"),
            "{out}"
        );
        // The warning comes after the summary of the apply.
        let summary = out.find("plan version 1").unwrap();
        let warning = out.find("warning:").unwrap();
        assert!(summary < warning, "the warning comes after the summary: {out}");
    }

    #[test]
    fn plan_apply_prints_the_tasks_a_rebind_is_waiting_for() {
        let (mut store, mut herdr) = world_with_task();
        store.write(|tx| {
            let mut project = tx.get_project("p")?.unwrap();
            project.config_toml = r#"
[project]
id="p"
repo="/repo"
herdr_session="sess"
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
[[repos]]
name="api"
path="api"
"#.into();
            tx.update_project(&project)
        }).unwrap();
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Staff\n- rebind heat repo=api owns=[src/, tests/]\n");
        let out = run_plan(&mut store, &env("captain", CAP_PANE), &mut herdr, Some(&path), None, None, "rebind", T0+1, false).unwrap();
        assert!(out.contains("rebind heat: waits for T-1"), "{out}");
    }

    /// The JSON output carries the same warnings as the text: the lanes that
    /// would stand idle and the apply's own (design §7.2: a decision line
    /// that is not structured).
    #[test]
    fn plan_apply_json_carries_the_warnings() {
        let (mut store, mut herdr) = world_with_waiting_dep();
        let draft = "<!-- gear-plan project=p base=0 -->\n# Decisions\n- we talked about it\n\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: T-8\n";
        let path = write_draft(draft);

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            true,
        )
        .unwrap();

        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["version"], 1);
        let warnings: Vec<String> = doc["warnings"]
            .as_array()
            .expect("the warnings")
            .iter()
            .map(|warning| warning.as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            warnings.contains(&"warning: lane heat has nothing to start after now (1 queued behind deps: T-1)".to_string()),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("is not structured; logged as a note")),
            "the apply's own warning is in the JSON too: {warnings:?}"
        );
    }

    /// T-58: a lane that is working on its last task with nothing startable
    /// behind it is told before it stands idle.
    #[test]
    fn plan_apply_warns_before_a_lane_runs_dry() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                // The lane "heat" is working on T-5; the task queued behind it
                // waits for T-7, which another lane is working on.
                tx.insert_task(&lane_task("T-5", "heat", gear_model::TaskState::Running, &[]))?;
                tx.insert_task(&lane_task("T-6", "heat", gear_model::TaskState::Ready, &["T-7"]))?;
                tx.insert_task(&lane_task("T-7", "wire", gear_model::TaskState::Running, &[]))
            })
            .unwrap();
        // The draft gives another lane the work, not this one.
        let draft = "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=wire\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n";
        let path = write_draft(draft);

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap();

        assert!(
            out.contains("warning: lane heat has nothing to start after T-5 (1 queued behind deps: T-6)"),
            "{out}"
        );
    }

    /// A task of the project, for the plan tests.
    fn lane_task(id: &str, lane: &str, state: gear_model::TaskState, deps: &[&str]) -> TaskRow {
        TaskRow {
            repo: None,
            base: None,
            gate: None,
            rework_estimate_min: None,
            project: "p".into(),
            id: id.into(),
            rank: 1,
            state,
            priority: "must".into(),
            lane: Some(lane.into()),
            milestone: None,
            required: false,
            contract_version: 1,
            deps: deps.iter().map(|dep| dep.to_string()).collect(),
            followup_of: None,
            wait_on: None,
            wait_cond: None,
            rework_note: None,
            estimate_min: None,
             issue: None,updated: T0,
        }
    }

    #[test]
    fn plan_apply_without_path_names_the_voyage_file() {
        let (mut store, mut herdr) = world();
        let err = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            None,
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(
            err.detail.contains("gear/voyages/M1.md"),
            "the error names the voyage file: {}",
            err.detail
        );
    }

    #[test]
    fn plan_apply_rewrites_the_voyage_file() {
        let (mut store, mut herdr) = world();
        let draft = format!(
            "<!-- gear-plan project=p base=0 -->\n# Goal\nthe goal\n\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n# Verdicts\n"
        );
        let path = write_draft(&draft);

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap();
        assert!(out.contains("created T-1"), "{out}");

        // The file is the current plan again: base= the applied version, the
        // NEW block carries its id, the Verdicts section is gone.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("<!-- gear-plan project=p base=1 -->"), "{text}");
        assert!(text.contains("## T-1 [must] lane=heat"), "{text}");
        assert!(!text.contains("NEW"), "{text}");
        assert!(!text.contains("# Verdicts"), "{text}");
        assert!(text.contains("# Goal\nthe goal"), "{text}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_second_apply_of_the_rewritten_file_does_nothing() {
        let (mut store, mut herdr) = world();
        let draft = format!(
            "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n# Verdicts\n"
        );
        let path = write_draft(&draft);
        run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap();

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan-2",
            T0 + 2,
            false,
        )
        .unwrap();
        assert!(out.contains("plan version 2"), "{out}");
        assert!(!out.contains("created"), "{out}");
        assert!(!out.contains("contract"), "{out}");
        assert!(!out.contains("settled"), "{out}");
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        assert_eq!(tasks.len(), 1, "{tasks:?}");
        assert_eq!(tasks[0].contract_version, 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("<!-- gear-plan project=p base=2 -->"), "{text}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_failed_apply_leaves_the_file_untouched() {
        let (mut store, mut herdr) = world();
        // T-9 does not exist: the apply fails after the file is read.
        let draft = format!(
            "<!-- gear-plan project=p base=0 -->\n# Queue\n## T-9 [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"
        );
        let path = write_draft(&draft);
        let err = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), draft, "the file is untouched");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn keep_is_not_in_slice_2() {
        let (mut store, mut herdr) = world();
        let path = write_draft(&plan_draft());
        let err = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            Some("V-1".into()),
            Some("+10m".into()),
            "r-plan",
            T0 + 1,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(err.detail.contains("not in slice 2"), "{}", err.detail);
    }

    /// T-38: `gear report time` is the captain's view of the time accounting.
    #[test]
    fn report_time_prints_the_durations_and_the_ratio() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-7".into(),
                    rank: 1,
                    state: gear_model::TaskState::Done,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: Some("M2".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(60),
                     issue: None,updated: T0,
                })?;
                tx.insert_attempt(&gear_store::AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: "T-7".into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: gear_model::AttemptState::Accepted,
                    candidate: Some("abc".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                })?;
                tx.set_attempt_time("p", "T-7", 1, gear_store::AttemptStep::Started, T0)?;
                tx.set_attempt_time(
                    "p",
                    "T-7",
                    1,
                    gear_store::AttemptStep::HandedIn,
                    T0 + 10 * 60,
                )?;
                tx.set_attempt_time(
                    "p",
                    "T-7",
                    1,
                    gear_store::AttemptStep::Verdict,
                    T0 + 15 * 60,
                )?;
                tx.set_attempt_time(
                    "p",
                    "T-7",
                    1,
                    gear_store::AttemptStep::Settled,
                    T0 + 20 * 60,
                )?;
                Ok(())
            })
            .unwrap();

        let out = run_report_time(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &gear_app::time::Filter::default(),
            false,
        )
        .unwrap();
        assert!(out.contains("Time report · project p"), "{out}");
        assert!(out.contains("T-7 · estimate 60m"), "{out}");
        assert!(out.contains("dev 10m · review 5m · wait 5m"), "{out}");
        assert!(out.contains("actual 20m"), "{out}");
        assert!(out.contains("ratio 0.33"), "{out}");
        assert!(out.contains("Voyage M2"), "{out}");

        // A lane may not read the report.
        let err = run_report_time(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &gear_app::time::Filter::default(),
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::Forbidden);
    }

    /// T-171: `gear report voyage` is the captain's landing report (design
    /// §7.5): the voyage's lines, its `done:` sentence clause by clause, its
    /// time and tokens, the ★ decisions in full (the rest counted per kind),
    /// the friction reports with their outcomes and the tasks that took more
    /// than one round.
    #[test]
    fn report_voyage_prints_the_six_parts() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                // The voyage: its own sentence, its bar and its eta.
                tx.upsert_voyage(&gear_store::VoyageRow {
                    project: "p".into(),
                    milestone: "M2".into(),
                    title: "the second station".into(),
                    done: Some("(1) v0.4.1: the board reads well; (2) v0.4.2: the user can review it".into()),
                    bar: Some("STABLE".into()),
                    eta: Some("12h".into()),
                    departure: Some(T0),
                    departure_tasks: Some(2),
                    departure_estimate_min: Some(120),
                    updated: T0,
                })?;
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-7".into(),
                    rank: 1,
                    state: gear_model::TaskState::Done,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: Some("M2".into()),
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: Some(60),
                     issue: None,updated: T0,
                })?;
                tx.insert_attempt(&gear_store::AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: "T-7".into(),
                    n: 1,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: gear_model::AttemptState::Accepted,
                    candidate: Some("abc".into()),
                    report: None,
                    safe_point: None,
                    started: T0,
                    updated: T0,
                })?;
                tx.set_attempt_time("p", "T-7", 1, gear_store::AttemptStep::Started, T0)?;
                tx.set_attempt_time("p", "T-7", 1, gear_store::AttemptStep::HandedIn, T0 + 10 * 60)?;
                tx.set_attempt_time("p", "T-7", 1, gear_store::AttemptStep::Settled, T0 + 20 * 60)?;
                // A second round: the task took more than one round.
                tx.insert_attempt(&gear_store::AttemptRow {
                    session_ref: None,
                    session_cli: None,
                    session_action: None,
                    sent_at: None,
                    project: "p".into(),
                    task: "T-7".into(),
                    n: 2,
                    seat: "p:heat".into(),
                    generation: 1,
                    contract_version: 1,
                    adopted_version: 1,
                    pending_update: None,
                    state: gear_model::AttemptState::Accepted,
                    candidate: Some("def".into()),
                    report: None,
                    safe_point: None,
                    started: T0 + 30 * 60,
                    updated: T0,
                })?;
                tx.set_attempt_time("p", "T-7", 2, gear_store::AttemptStep::Started, T0 + 30 * 60)?;
                tx.set_attempt_time("p", "T-7", 2, gear_store::AttemptStep::HandedIn, T0 + 40 * 60)?;
                tx.set_attempt_time("p", "T-7", 2, gear_store::AttemptStep::Settled, T0 + 45 * 60)?;
                // The round of the lane, with its tokens and its cost.
                let key = gear_store::RoundKey {
                    project: "p".into(),
                    task: "T-7".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "opencode".into(),
                    model: Some("gpt".into()),
                    start_at: T0,
                })?;
                tx.set_round_active(&key, 30 * 60)?;
                tx.set_round_tokens(
                    &key,
                    &gear_store::RoundTokens {
                        input: Some(1000),
                        output: Some(500),
                        cache_read: Some(10),
                        cache_write: Some(20),
                        reasoning: Some(5),
                        cost_usd: Some(0.25),
                        cost_source: "prices".into(),
                    },
                    T0 + 20 * 60,
                )?;
                // The lane waited for the quota (design §5.4): a pause.
                tx.open_pause("p", "p:heat", "quota", "minimax-cn 5h", T0)?;
                tx.close_pause("p", "p:heat", "quota", T0 + 2 * 3600)?;
                // The decisions: one ★ and two of other kinds.
                tx.insert_decision_if_new(&gear_store::NewDecision {
                    project: "p".into(),
                    key: "k1".into(),
                    voyage: "M2".into(),
                    at: "2026-10-26 10:00".into(),
                    kind: "scope".into(),
                    what: "the board reads well".into(),
                    why: Some("the user asked".into()),
                    alt: None,
                    changes: None,
                    star: true,
                    source: "plan".into(),
                    line: None,
                    plan_version: None,
                })?;
                tx.insert_decision_if_new(&gear_store::NewDecision {
                    project: "p".into(),
                    key: "k2".into(),
                    voyage: "M2".into(),
                    at: "2026-10-26 11:00".into(),
                    kind: "estimate".into(),
                    what: "the estimate holds".into(),
                    why: Some("measured".into()),
                    alt: None,
                    changes: None,
                    star: false,
                    source: "plan".into(),
                    line: None,
                    plan_version: None,
                })?;
                tx.insert_decision_if_new(&gear_store::NewDecision {
                    project: "p".into(),
                    key: "k3".into(),
                    voyage: "M2".into(),
                    at: "2026-10-26 12:00".into(),
                    kind: "estimate".into(),
                    what: "the estimate moved".into(),
                    why: Some("measured".into()),
                    alt: None,
                    changes: None,
                    star: false,
                    source: "plan".into(),
                    line: None,
                    plan_version: None,
                })?;
                // A friction report, open.
                tx.insert_friction(&gear_store::NewFriction {
                    project: "p".into(),
                    task: "T-7".into(),
                    attempt: 1,
                    role: "dev".into(),
                    seat: "p:heat".into(),
                    body: "the build took too long".into(),
                    required: true,
                })?;
                Ok(())
            })
            .unwrap();

        let out = run_report_voyage(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some("M2"),
            false,
            false,
            T0 + 2 * 3600,
        )
        .unwrap();

        // 1. The voyage's own lines.
        assert!(out.contains("Voyage M2"), "{out}");
        // 2. The done: sentence, clause by clause.
        assert!(out.contains("(1) v0.4.1: the board reads well"), "{out}");
        assert!(out.contains("(2) v0.4.2: the user can review it"), "{out}");
        // 3. Time and tokens, the work and waiting time (§5.4).
        assert!(out.contains("tokens 1000 in / 500 out"), "{out}");
        assert!(out.contains("cost $0.2500"), "{out}");
        assert!(out.contains("lane heat · work 0h30m"), "{out}");
        assert!(out.contains("quota wait 2h00m"), "{out}");
        assert!(out.contains("fleet stopped by quota"), "{out}");
        // 4. The ★ decision in full; the others counted per kind.
        assert!(out.contains("★ scope · the board reads well"), "{out}");
        assert!(out.contains("why: the user asked"), "{out}");
        assert!(out.contains("others: estimate 2"), "{out}");
        // 5. The friction report, open.
        assert!(out.contains("the build took too long"), "{out}");
        assert!(out.contains("open"), "{out}");
        // 6. The task that took more than one round.
        assert!(out.contains("Rework"), "{out}");

        // --all prints every decision, not only the ★ ones.
        let all = run_report_voyage(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some("M2"),
            true,
            false,
            T0 + 2 * 3600,
        )
        .unwrap();
        assert!(all.contains("the estimate holds"), "{all}");
        assert!(all.contains("the estimate moved"), "{all}");

        // A lane may not read the report.
        let err = run_report_voyage(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            Some("M2"),
            false,
            false,
            T0 + 2 * 3600,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::Forbidden);
    }

    #[test]
    fn send_candidate_reaches_mail() {
        let (mut store, mut herdr) = world();
        // A started attempt on T-7, so a result can be submitted.
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-7".into(),
                    rank: 1,
                    state: gear_model::TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: T0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-7", 1, T0)?;
                Ok(())
            })
            .unwrap();

        let mut c = send_cmd("result");
        c.task = Some("T-7".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc123".into());
        let out = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "done, see abc123".into(),
            None,
            T0 + 2,
            false,
        )
        .unwrap();

        assert!(out.contains("Delivered to p:captain"), "{out}");
        let attempt = store.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap();
        assert_eq!(attempt.state, gear_model::AttemptState::Submitted);
    }

    // --- T-2: a temp reviewer hands in a verdict ---

    const TEMP_PANE: &str = "w1:p3";

    /// A world with a task, a started attempt and a temp reviewer bound to it.
    fn review_world() -> (Store, FakeHerdr) {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                testing::seed_temp_seat(tx, "p", "r-1", "T-7", TEMP_PANE, "opencode", T0)?;
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: "T-7".into(),
                    rank: 1,
                    state: gear_model::TaskState::Ready,
                    priority: "must".into(),
                    lane: Some("heat".into()),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: T0,
                })?;
                gear_app::tasks::start_attempt(tx, "p", "T-7", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-7", 1, T0)?;
                Ok(())
            })
            .unwrap();
        herdr.add_agent(TEMP_PANE, "opencode", AgentStatus::Idle);
        (store, herdr)
    }

    fn review_cmd(verdict: &str) -> SendCmd {
        let mut c = send_cmd("review");
        c.task = Some("T-7".into());
        c.attempt = Some(1);
        c.verdict = Some(verdict.into());
        c
    }

    #[test]
    fn send_review_stores_the_verdict_and_reaches_the_captain() {
        let (mut store, mut herdr) = review_world();

        let out = run_send(
            &mut store,
            &env("r-1", TEMP_PANE),
            &mut herdr,
            &review_cmd("pass"),
            "P1: the header count is stale".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        assert!(out.contains("Delivered to p:captain"), "{out}");
        let record = store.read(|tx| tx.get_review_verdict("p", "T-7", 1)).unwrap().unwrap();
        assert!(record.passed);
        assert_eq!(record.findings, "P1: the header count is stale");
        assert_eq!(record.by_seat, "p:r-1");
        // A passing verdict is information; the engine handles settlement.
        assert!(store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 2)).unwrap().is_empty());
        let history = store.read(|tx| tx.thread_messages("p", "T-7:1")).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].state, MsgState::Info);
        assert!(history[0].body.contains("pass"), "{}", history[0].body);
    }

    #[test]
    fn send_review_with_a_bad_verdict_is_bad_format() {
        let (mut store, mut herdr) = review_world();
        let err = run_send(
            &mut store,
            &env("r-1", TEMP_PANE),
            &mut herdr,
            &review_cmd("maybe"),
            "ops/reviews/T-7.md".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(err.detail.contains("verdict"), "{}", err.detail);
    }

    #[test]
    fn send_review_from_a_lane_seat_is_forbidden() {
        let (mut store, mut herdr) = review_world();
        let err = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &review_cmd("pass"),
            "ops/reviews/T-7.md".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::Forbidden);
        assert!(store.read(|tx| tx.get_review_verdict("p", "T-7", 1)).unwrap().is_none());
    }

    #[test]
    fn send_json_is_the_result() {
        let (mut store, mut herdr) = world();
        let out = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &send_cmd("progress"),
            "working".into(),
            None,
            T0 + 1,
            true,
        )
        .unwrap();
        let parsed: gear_app::mail::SendResult = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.msg_id, "M-1");
        assert_eq!(parsed.recipient, "p:captain");
        assert!(!parsed.replayed);
    }

    // ------------------------------------------------------------ T-130

    /// A project whose lane `heat` runs pi and works, and whose lane `wire`
    /// runs codex: the two agent kinds the doorbell treats differently.
    fn working_world(lane_kind: &str, status: AgentStatus) -> (Store, FakeHerdr) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, lane_kind, T0)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        // Every agent carries the name of its seat (T-149: a text aims at the
        // live agent name, and Herdr refuses a prompt to an agent that lost
        // its name).
        herdr.add_agent_named(CAP_PANE, "codex", AgentStatus::Idle, "p-captain");
        herdr.add_agent_named(LANE_PANE, lane_kind, status, "p-heat");
        (store, herdr)
    }

    fn captain_instruction(to: &str, urgent: bool) -> SendCmd {
        SendCmd {
            to: Some(to.into()),
            urgent,
            ..send_cmd("instruction")
        }
    }

    fn tick_cfg() -> gear_engine::EngineCfg {
        gear_engine::EngineCfg {
            tick_s: 4,
            cooldown_s: 30,
            doorbell_check_budget: 3,
            profiles: gear_herdr::builtins(),
            ..Default::default()
        }
    }

    #[test]
    fn an_urgent_send_types_the_doorbell_before_it_returns_and_the_engine_does_not_ring_again() {
        let (mut store, mut herdr) = working_world("codex", AgentStatus::Working);
        let out = run_send(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &captain_instruction("heat", true),
            "stop and read this".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        let said = herdr.prompts_to(LANE_PANE);
        assert_eq!(said.len(), 1, "typed inside gear send: {said:?}");
        assert!(said[0].contains("M-1"), "the doorbell names the message: {said:?}");
        // T-150 (design §10.4): the lane has no open attempt, so the doorbell
        // says so and tells it to stop working on earlier things.
        assert!(said[0].contains("You have no task now"), "{said:?}");
        assert!(out.contains("doorbell typed into"), "{out}");

        // The next tick sees the message covered: no second doorbell.
        gear_engine::tick(&mut store, &mut herdr, &tick_cfg(), T0 + 40).unwrap();
        assert_eq!(herdr.prompts_to(LANE_PANE).len(), 1, "one doorbell per message");
    }

    #[test]
    fn a_captain_instruction_to_a_working_pi_lane_is_typed_by_the_send() {
        let (mut store, mut herdr) = working_world("pi", AgentStatus::Working);
        run_send(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &captain_instruction("heat", false),
            "use v2".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        let said = herdr.prompts_to(LANE_PANE);
        assert_eq!(said.len(), 1, "pi queues input while it works: {said:?}");
        assert!(said[0].contains("M-1"), "{said:?}");
    }

    #[test]
    fn a_normal_instruction_to_a_working_codex_lane_waits_for_the_engine_and_idle() {
        let (mut store, mut herdr) = working_world("codex", AgentStatus::Working);
        run_send(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &captain_instruction("heat", false),
            "use v2".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();
        assert!(herdr.prompts_to(LANE_PANE).is_empty(), "codex does not queue input");

        // Still working: the engine keeps it for idle.
        gear_engine::tick(&mut store, &mut herdr, &tick_cfg(), T0 + 40).unwrap();
        assert!(herdr.prompts_to(LANE_PANE).is_empty(), "waits for idle");

        // Idle: the engine rings it once.
        herdr.set_status(LANE_PANE, AgentStatus::Idle);
        gear_engine::tick(&mut store, &mut herdr, &tick_cfg(), T0 + 80).unwrap();
        let said = herdr.prompts_to(LANE_PANE);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("M-1"), "{said:?}");
    }

    #[test]
    fn a_failed_herdr_leaves_the_message_stored_and_the_engine_rings_it() {
        let (mut store, mut herdr) = working_world("codex", AgentStatus::Working);
        herdr.inject_next = Some(gear_herdr::fake::Inject::Refuse { code: "agent_blocked".into() });
        run_send(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &captain_instruction("heat", true),
            "stop and read this".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        assert_eq!(
            herdr.prompts_to(LANE_PANE).len(),
            1,
            "the send tried and Herdr refused: {:?}",
            herdr.prompts_to(LANE_PANE)
        );
        let stored = store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
        assert_eq!(stored.body, "stop and read this", "the message is stored all the same");
        // The claim is recorded, and it waits: design §10.1 recovers a refused
        // prompt by naming the agent of the seat again and typing it on the
        // next tick (nothing is lost).
        let ops = store
            .read(|tx| tx.ops_of_seat_kind("p:heat", OpKind::Doorbell))
            .unwrap();
        assert_eq!(ops.len(), 1, "{ops:?}");
        assert_eq!(ops[0].state, OpState::Intended, "{ops:?}");

        gear_engine::tick(&mut store, &mut herdr, &tick_cfg(), T0 + 40).unwrap();
        let said = herdr.prompts_to(LANE_PANE);
        assert_eq!(said.len(), 2, "the engine rings it on its next tick: {said:?}");
        assert!(said[1].contains("M-1"), "{said:?}");
    }

    #[test]
    fn reply_resolves_by_default() {
        let (mut store, mut herdr) = world();
        // The lane asks the captain a question.
        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &send_cmd("question"),
            "v1 or v2?".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        let out = run_reply(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &reply_cmd(&["M-1"]),
            T0 + 2,
            false,
        )
        .unwrap();

        assert!(out.contains("M-1"), "{out}");
        assert!(out.contains("resolved"), "{out}");
        let stored = store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
        assert_eq!(stored.state, MsgState::Resolved);
        // The answer went back to the lane's inbox.
        let reply = store.read(|tx| tx.get_message("p", "M-2")).unwrap().unwrap();
        assert_eq!(reply.kind, MsgKind::Reply);
        assert_eq!(reply.recipient, "p:heat");
    }

    #[test]
    fn a_reopen_reopens_a_resolved_item() {
        // §10.7 row B03-07: an item resolved by mistake goes back to the
        // captain's inbox.
        let (mut store, mut herdr) = world();
        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &send_cmd("question"),
            "v1 or v2?".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();
        run_reply(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &reply_cmd(&["M-1"]),
            T0 + 2,
            false,
        )
        .unwrap();
        assert_eq!(
            store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap().state,
            MsgState::Resolved
        );

        let mut c = reply_cmd(&["M-1"]);
        c.reopen = true;
        c.request_id = Some("r-3".into());
        let out = run_reply(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &c,
            T0 + 3,
            false,
        )
        .unwrap();

        assert!(out.contains("M-1: open"), "{out}");
        assert_eq!(
            store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap().state,
            MsgState::Open
        );
        // The item is back in the captain's inbox.
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 4)).unwrap();
        assert_eq!(inbox.len(), 1, "{inbox:?}");
    }

    #[test]
    fn the_reply_help_says_the_default_resolves() {
        // §10.7 row B03-07: the help says what the default does, and names
        // the way back.
        let mut command = <Cli as clap::CommandFactory>::command();
        let help = command
            .get_subcommands_mut()
            .find(|command| command.get_name() == "reply")
            .expect("the reply command")
            .render_long_help()
            .to_string();
        assert!(help.contains("resolved"), "the default resolves: {help}");
        assert!(help.contains("--reopen"), "the way back: {help}");
    }

    #[test]
    fn a_hand_in_answers_with_the_next_task_line() {
        // §10.7 row B03-04/10: after a hand-in the lane is told that its
        // work is in, that the next task comes by itself and that its task
        // branch is not pushed again.
        let (mut store, mut herdr) = world_with_task();
        let mut c = send_cmd("result");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc1234".into());
        let out = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Report: ops/tasks/T-1.md".into(),
            None,
            T0 + 5,
            false,
        )
        .unwrap();
        assert!(
            out.contains(
                "Handed in. End your turn: the next task comes by itself; do not push to heat/t1 again."
            ),
            "{out}"
        );
    }

    /// A lane working on T-1 (its attempt is open) and a task T-2 it can
    /// wait for.
    fn world_with_a_wait() -> (Store, FakeHerdr) {
        let (mut store, herdr) = world();
        store
            .write(|tx| {
                for (id, state, lane) in [
                    ("T-1", gear_model::TaskState::Ready, "heat"),
                    ("T-2", gear_model::TaskState::Ready, "wire"),
                ] {
                    tx.insert_task(&TaskRow {
                        project: "p".into(),
                        id: id.into(),
                        rank: 1,
                        state,
                        priority: "must".into(),
                        lane: Some(lane.into()),
                        milestone: None,
                        required: false,
                        contract_version: 1,
                        deps: vec![],
                        followup_of: None,
                        wait_on: None,
                        wait_cond: None,
                        rework_note: None,
                        estimate_min: None,
                        issue: None,
                        repo: None,
                        base: None,
                        gate: None,
                        rework_estimate_min: None,
                        updated: T0,
                    })?;
                    tx.insert_contract(&ContractRow {
                        project: "p".into(),
                        task: id.into(),
                        version: 1,
                        goal: "g".into(),
                        accept: "a".into(),
                        evidence_files: vec![],
                        evidence_due_min: None,
                        body: String::new(),
                        created: T0,
                    })?;
                }
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)
            })
            .unwrap();
        (store, herdr)
    }

    /// §5.2: a progress answer carries the minutes left and what stops the
    /// lane, in the message's meta.
    #[test]
    fn a_progress_answer_stores_its_meta() {
        let (mut store, mut herdr) = world();
        let mut c = send_cmd("progress");
        c.left = Some(30);
        c.blocker = Some("the api is late".into());

        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "the parser is done; the api is not".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        let message = store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
        let meta: serde_json::Value =
            serde_json::from_str(message.meta.as_deref().expect("the answer has a meta")).unwrap();
        assert_eq!(meta["left"], serde_json::json!(30));
        assert_eq!(meta["blocker"], serde_json::json!("the api is late"));
    }

    /// §5.3: a declared wait is written on the task; the attempt stays
    /// Started, and the captain gets the info item.
    #[test]
    fn a_blocked_message_declares_a_wait() {
        let (mut store, mut herdr) = world_with_a_wait();
        let mut c = send_cmd("blocked");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.on = Some("T-2".into());

        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "the parser waits for the api of T-2".into(),
            None,
            T0 + 5,
            false,
        )
        .unwrap();

        let task = store.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
        assert_eq!(task.wait_on.as_deref(), Some("T-2"));
        assert_eq!(task.wait_cond.as_deref(), Some("merged"));
        // The attempt goes on: a wait is not a hand-in.
        assert_eq!(
            store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state,
            gear_model::AttemptState::Started
        );
        // The captain has the info item.
        let message = store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
        assert_eq!(message.recipient, "p:captain");
        assert_eq!(message.kind, MsgKind::Blocked);
        assert_eq!(message.state, MsgState::Info);
    }

    /// §5.3: a wait for a task that is already done would never end.
    #[test]
    fn a_blocked_message_on_a_done_task_is_refused() {
        let (mut store, mut herdr) = world_with_a_wait();
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-2")?.unwrap();
                task.state = gear_model::TaskState::Done;
                tx.update_task(&task)
            })
            .unwrap();
        let mut c = send_cmd("blocked");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.on = Some("T-2".into());

        let error = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "the parser waits for the api of T-2".into(),
            None,
            T0 + 5,
            false,
        )
        .unwrap_err();

        assert_eq!(error.code, gear_model::ErrorCode::BadFormat, "{}", error.detail);
        assert!(error.detail.contains("T-2 is done"), "{}", error.detail);
        // Nothing was written: the task waits for nothing.
        let task = store.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
        assert_eq!(task.wait_on, None);
    }

    /// §5.3: the contract the lane names must be the one the task carries
    /// now: a captain who changed the contract (and ended the wait with it)
    /// must not have the lane's old command park the task on an obsolete
    /// dependency.
    #[test]
    fn a_blocked_message_with_a_stale_contract_is_refused() {
        let (mut store, mut herdr) = world_with_a_wait();
        // The captain changed the task's contract.
        store
            .write(|tx| {
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.contract_version = 2;
                tx.update_task(&task)
            })
            .unwrap();
        let mut c = send_cmd("blocked");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1); // the contract the lane still works to
        c.on = Some("T-2".into());

        let error = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "the parser waits for the api of T-2".into(),
            None,
            T0 + 5,
            false,
        )
        .unwrap_err();

        assert_eq!(error.code, gear_model::ErrorCode::BadFormat, "{}", error.detail);
        assert!(error.detail.contains("is on contract v2"), "{}", error.detail);
        // Nothing was written: no wait and no message.
        let task = store.read(|tx| tx.get_task("p", "T-1")).unwrap().unwrap();
        assert_eq!(task.wait_on, None);
        assert!(store.read(|tx| tx.get_message("p", "M-1")).unwrap().is_none());
    }

    /// The request fingerprint carries the answer's fields: a request id
    /// reused with a changed field is a conflict, not a replay of the old
    /// answer.
    #[test]
    fn a_reused_request_id_with_a_changed_field_is_a_conflict() {
        let (mut store, mut herdr) = world();
        let mut first = send_cmd("progress");
        first.left = Some(30);
        first.blocker = Some("none".into());

        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &first,
            "the parser is done".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();

        // The same request id, another answer: a conflict, not a replay.
        let mut changed = send_cmd("progress");
        changed.left = Some(60);
        changed.blocker = Some("the api is late".into());
        let error = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &changed,
            "the parser is done".into(),
            None,
            T0 + 2,
            false,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            gear_model::ErrorCode::RequestConflict,
            "{}",
            error.detail
        );

        // The same request id with the same answer: a replay.
        let out = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &first,
            "the parser is done".into(),
            None,
            T0 + 3,
            false,
        )
        .unwrap();
        assert!(out.contains("[replayed]"), "{out}");
    }

    /// §6.2: a round that passed its budget must carry a friction report.
    #[test]
    fn an_over_budget_hand_in_without_a_friction_report_is_refused() {
        let (mut store, mut herdr) = world_with_task();
        let mut c = send_cmd("result");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc1234".into());

        // Forty minutes into a thirty minute budget (the default budget of a
        // task without an estimate).
        let error = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Report: ops/reports/T-1.md".into(),
            None,
            T0 + 40 * 60,
            false,
        )
        .unwrap_err();

        assert_eq!(
            error.code,
            gear_model::ErrorCode::FrictionRequired,
            "{}",
            error.detail
        );
        assert!(
            error.detail.contains("T-1 took 40 min against 30 min"),
            "{}",
            error.detail
        );
        assert!(error.detail.contains("--friction"), "{}", error.detail);
        // Nothing was written: the attempt is not handed in.
        assert_eq!(
            store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state,
            gear_model::AttemptState::Started
        );
        assert!(store
            .read(|tx| tx.frictions_of_project("p"))
            .unwrap()
            .is_empty());
    }

    /// §6.1: the report is stored with the round's own facts.
    #[test]
    fn an_over_budget_hand_in_with_a_friction_report_is_stored() {
        let (mut store, mut herdr) = world_with_task();
        let mut c = send_cmd("result");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc1234".into());
        c.friction = Some("the api test needs a fixture that is missing".into());

        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Report: ops/reports/T-1.md".into(),
            None,
            T0 + 40 * 60,
            false,
        )
        .unwrap();

        let reports = store.read(|tx| tx.frictions_of_project("p")).unwrap();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(reports[0].task, "T-1");
        assert_eq!(reports[0].attempt, 1);
        assert_eq!(reports[0].role, "dev");
        assert_eq!(
            reports[0].body,
            "the api test needs a fixture that is missing"
        );
        assert!(reports[0].required, "the round passed its budget");
    }

    /// §6.2: the budget is compared in seconds, so a round that is one second
    /// over its budget is over.
    #[test]
    fn a_hand_in_one_second_over_the_budget_needs_a_friction_report() {
        // Exactly the 30 minute budget: no report needed.
        let (mut store, mut herdr) = world_with_task();
        let mut c = send_cmd("result");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc1234".into());
        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Report: ops/reports/T-1.md".into(),
            None,
            T0 + 1800,
            false,
        )
        .unwrap();
        assert!(store
            .read(|tx| tx.frictions_of_project("p"))
            .unwrap()
            .is_empty());

        // One second more: the report is required.
        let (mut store, mut herdr) = world_with_task();
        let error = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Report: ops/reports/T-1.md".into(),
            None,
            T0 + 1801,
            false,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            gear_model::ErrorCode::FrictionRequired,
            "{}",
            error.detail
        );

        // With the report the hand-in goes through, and the report says it
        // was required.
        let mut with_report = c.clone();
        with_report.friction = Some("the fixture was missing".into());
        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &with_report,
            "Report: ops/reports/T-1.md".into(),
            None,
            T0 + 1801,
            false,
        )
        .unwrap();
        let reports = store.read(|tx| tx.frictions_of_project("p")).unwrap();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].required, "the round passed its budget");
    }

    /// §6.2: under the budget the report is optional.
    #[test]
    fn a_hand_in_under_budget_needs_no_friction_report() {
        let (mut store, mut herdr) = world_with_task();
        let mut c = send_cmd("result");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc1234".into());

        run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "Report: ops/reports/T-1.md".into(),
            None,
            T0 + 10 * 60,
            false,
        )
        .unwrap();

        assert_eq!(
            store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap().state,
            gear_model::AttemptState::Submitted
        );
        assert!(store
            .read(|tx| tx.frictions_of_project("p"))
            .unwrap()
            .is_empty());
    }

    /// §5.1: a review is charged from the reviewer's own start — its delivery
    /// confirmation — and never from the writer's check-in.
    #[test]
    fn a_review_is_charged_from_the_reviewers_own_start() {
        let (mut store, mut herdr) = world_with_task();
        herdr.add_agent("w1:p9", "codex", AgentStatus::Idle);
        // The lane hands the candidate in.
        store
            .write(|tx| {
                gear_app::tasks::submit_result(
                    tx,
                    "p",
                    "T-1",
                    1,
                    1,
                    "abc1234",
                    "ops/reports/T-1.md",
                    T0,
                )
            })
            .unwrap();
        // The reviewer gets the packet an hour later.
        store
            .write(|tx| tx.set_attempt_sent("p", "T-1", 1, T0 + 3600))
            .unwrap();
        // A reviewer seat, bound to the task.
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w1:p9", "codex", T0)?;
                tx.set_seat_task("p:review-1", "T-1")
            })
            .unwrap();

        // The verdict five minutes after the delivery: well inside the review's
        // own budget, even though the writer checked in an hour before.
        let mut c = send_cmd("review");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.verdict = Some("pass".into());
        run_send(
            &mut store,
            &env("review-1", "w1:p9"),
            &mut herdr,
            &c,
            "all good".into(),
            None,
            T0 + 3600 + 300,
            false,
        )
        .unwrap();

        assert!(
            store
                .read(|tx| tx.frictions_of_project("p"))
                .unwrap()
                .is_empty(),
            "the reviewer was inside its own budget"
        );
    }

    /// §6.3, §6.4: the captain reads the reports and closes one.
    #[test]
    fn gear_friction_lists_the_reports_and_closes_one() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                tx.insert_friction(&gear_store::NewFriction {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                    seat: "p:heat".into(),
                    body: "the api test needs a fixture that is missing".into(),
                    required: true,
                })
            })
            .unwrap();

        let out = run_friction(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &friction_cmd(false),
            T0,
        )
        .unwrap();
        assert!(out.contains("F-1"), "{out}");
        assert!(
            out.contains("the api test needs a fixture that is missing"),
            "{out}"
        );
        assert!(out.contains("required"), "{out}");

        // The captain closes it: what the captain did about it.
        let out = run_friction(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &friction_close("F-1", "fixed: the fixture is in the repo"),
            T0 + 1,
        )
        .unwrap();
        assert!(out.contains("closed F-1"), "{out}");

        // The open list is empty now; `--all` still shows the report.
        let out = run_friction(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &friction_cmd(false),
            T0 + 2,
        )
        .unwrap();
        assert!(out.contains("no open friction reports"), "{out}");
        let out = run_friction(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &friction_cmd(true),
            T0 + 3,
        )
        .unwrap();
        assert!(out.contains("F-1"), "{out}");
        assert!(out.contains("closed: fixed"), "{out}");
    }

    /// §6.4: the outcome is one of the three kinds.
    #[test]
    fn closing_a_friction_report_needs_a_known_outcome() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                tx.insert_friction(&gear_store::NewFriction {
                    project: "p".into(),
                    task: "T-1".into(),
                    attempt: 1,
                    role: "dev".into(),
                    seat: "p:heat".into(),
                    body: "a permission it lacked".into(),
                    required: false,
                })
            })
            .unwrap();

        // The word must end at a word boundary and something must follow it.
        for outcome in ["we looked at it", "fixedwhatever", "fixed", "lesson"] {
            let error = run_friction(
                &mut store,
                &env("captain", CAP_PANE),
                &mut herdr,
                &friction_close("F-1", outcome),
                T0,
            )
            .unwrap_err();
            assert_eq!(
                error.code,
                gear_model::ErrorCode::BadFormat,
                "{outcome:?}: {}",
                error.detail
            );
            assert!(error.detail.contains("not worth it"), "{}", error.detail);
        }

        // The word and what changed: the report closes.
        let out = run_friction(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &friction_close("F-1", "fixed: the fixture is in the repo"),
            T0 + 1,
        )
        .unwrap();
        assert!(out.contains("closed F-1"), "{out}");
    }

    fn friction_cmd(all: bool) -> FrictionCmd {
        FrictionCmd { op: None, all }
    }

    fn friction_close(id: &str, message: &str) -> FrictionCmd {
        FrictionCmd {
            op: Some(FrictionOp::Close { id: id.into(), message: message.into() }),
            all: false,
        }
    }

    #[test]
    fn wait_until_relative_and_absolute() {
        assert_eq!(parse_until("+10m", 1_000).unwrap(), 1_600);
        assert_eq!(parse_until("+2h", 1_000).unwrap(), 8_200);
        assert_eq!(parse_until("1800000123", 1_000).unwrap(), 1_800_000_123);
    }

    #[test]
    fn bad_until_is_bad_format() {
        for text in ["soon", "+10x", "+", "10:30", "+0m"] {
            let err = parse_until(text, 1_000).unwrap_err();
            assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{text}");
        }
    }

    // --- T-28: environment, store, request id ---

/// A variable lookup over a fixed map (production reads the process env).
    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name: &str| map.get(name).cloned()
    }

    // --- T-32: `gear run` ---

    /// A unique directory for a lock file.
    fn lock_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("gear-lock-{}-{tag}-{nanos}", std::process::id()))
    }

    #[test]
    fn the_registry_keeps_one_store_per_herdr_session() {
        let dir = lock_dir("targets");
        let path = dir.join("targets.tsv");
        assert_eq!(registered_store(&path, "/s/orch.sock"), None);
        register_store(&path, "/s/orch.sock", "/state/a.db").unwrap();
        register_store(&path, "/s/other.sock", "/state/b.db").unwrap();
        register_store(&path, "/s/orch.sock", "/state/c.db").unwrap();
        assert_eq!(registered_store(&path, "/s/orch.sock").as_deref(), Some("/state/c.db"));
        assert_eq!(registered_store(&path, "/s/other.sock").as_deref(), Some("/state/b.db"));
    }

    #[test]
    fn utc_formats_known_instants() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_761_143_045), "2025-10-22T14:24:05Z");
    }

    #[test]
    fn second_lock_is_refused() {
        let dir = lock_dir("second");
        let first = take_engine_lock(&dir).unwrap();
        let err = take_engine_lock(&dir).unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::Forbidden);
        assert!(err.detail.contains("engine already running"), "{}", err.detail);
        drop(first);
    }

    #[test]
    fn lock_released_on_drop() {
        let dir = lock_dir("drop");
        {
            let lock = take_engine_lock(&dir).unwrap();
            assert!(take_engine_lock(&dir).is_err(), "held while the first handle lives");
            drop(lock);
        }
        // The lock went away with the handle.
        drop(take_engine_lock(&dir).unwrap());
    }

    /// A doorbell op the previous process left Executing.
    fn executing_op(store: &mut Store) -> String {
        store
            .write(|tx| {
                let id = tx
                    .insert_op(&gear_store::NewOp {
                        project: "p".into(),
                        seat: "p:captain".into(),
                        generation: 1,
                        kind: OpKind::Doorbell,
                        task: None,
                        attempt: None,
                        ref_msg: None,
                        plan_version: None,
                        payload: serde_json::json!({ "text": "t", "covers": ["M-1"] }),
                        created: T0,
                    })?
                    .id;
                tx.set_op_state("p", &id, OpState::Executing, None, T0)?;
                Ok(id)
            })
            .unwrap()
    }

    #[test]
    fn engine_cfg_carries_the_builtin_profiles() {
        let cfg = engine_cfg("/state/gear.db");
        assert_eq!(cfg.db_path, "/state/gear.db", "the agents get GEAR_DB");
        let opencode = cfg.profiles.get("opencode").expect("opencode is built in");
        assert_eq!(opencode.kind, "opencode");
        assert!(opencode.verified);
        assert_eq!(opencode.unattended_args, vec!["--auto".to_string()]);
        // A kind without a verified SOP stays out, so its seats fail closed.
        assert!(cfg.profiles.contains_key("claude"), "the captain gets a doorbell");
        // The reviewers run on Codex (T-5): its SOP is verified, so the
        // engine knows how to start and observe one.
        let codex = cfg.profiles.get("codex").expect("codex is built in");
        assert_eq!(codex.kind, "codex");
        assert!(codex.verified);
        assert_eq!(
            codex.unattended_args,
            vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-c".to_string(),
                "check_for_update_on_startup=false".to_string(),
                // T-76: the folder the agent runs in is trusted.
                "-c".to_string(),
                "projects={\"{repo_root}\"={trust_level=\"trusted\"}}".to_string(),
            ]
        );
    }

    #[test]
    fn recovers_then_ticks_n_times() {
        let (mut store, mut herdr) = world();
        let id = executing_op(&mut store);

        let clock_calls = std::cell::Cell::new(0);
        let mut clock = || {
            clock_calls.set(clock_calls.get() + 1);
            T0 + clock_calls.get()
        };
        let mut out: Vec<u8> = Vec::new();
        run_engine(
            &mut store,
            &mut herdr,
            &gear_engine::EngineCfg::default(),
            &mut clock,
            Some(2),
            &mut out,
        )
        .unwrap();

        // One clock call for the recovery, then one per tick.
        assert_eq!(clock_calls.get(), 3, "recovery once, then exactly two ticks");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(&id), "{text}");
        let op = store.read(|tx| tx.get_op("p", &id)).unwrap().unwrap();
        assert_eq!(op.state, OpState::Unknown, "recovered before the first tick");
    }

    #[test]
    fn tick_error_does_not_stop_the_loop() {
        let (mut store, mut herdr) = world();
        let clock_calls = std::cell::Cell::new(0);
        let mut clock = || {
            clock_calls.set(clock_calls.get() + 1);
            T0 + clock_calls.get()
        };
        let tick_calls = std::cell::Cell::new(0);
        let mut failing = |_: &mut Store,
                           _: &mut dyn gear_herdr::HerdrAct,
                           _: &gear_engine::EngineCfg,
                           _: i64| {
            tick_calls.set(tick_calls.get() + 1);
            Err::<gear_engine::TickReport, _>(GearError::new(
                gear_model::ErrorCode::BadFormat,
                "boom",
            ))
        };
        let mut out: Vec<u8> = Vec::new();
        run_engine_with(
            &mut store,
            &mut herdr,
            &gear_engine::EngineCfg::default(),
            &mut clock,
            Some(3),
            &mut out,
            &mut failing,
        )
        .unwrap();

        assert_eq!(tick_calls.get(), 3, "a failing tick does not stop the loop");
        assert!(String::from_utf8(out).unwrap().contains("boom"));
    }

    #[test]
    fn env_without_ctx_is_anonymous() {
        let env = env_from(vars(&[])).unwrap();
        assert_eq!(env, gear_app::Env { ctx: None, pane_id: None, admin: false });
    }

    #[test]
    fn env_parses_ctx_pane_and_admin() {
        let env = env_from(vars(&[
            ("GEAR_CTX", "harbor:heat:7"),
            ("HERDR_PANE_ID", "w1:p2"),
            ("GEAR_ADMIN", "1"),
        ]))
        .unwrap();
        assert_eq!(
            env.ctx,
            Some(gear_model::GearCtx { project: "harbor".into(), seat: "heat".into(), generation: 7 })
        );
        assert_eq!(env.pane_id.as_deref(), Some("w1:p2"));
        assert!(env.admin);
    }

    #[test]
    fn malformed_ctx_is_bad_format() {
        let err = env_from(vars(&[("GEAR_CTX", "nope")])).unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(err.detail.contains("GEAR_CTX"), "{}", err.detail);
    }

    #[test]
    fn admin_only_when_exactly_1() {
        for value in ["0", "true", "yes", "", "01", " 1"] {
            let env = env_from(vars(&[("GEAR_ADMIN", value)])).unwrap();
            assert!(!env.admin, "GEAR_ADMIN={value:?} is not the admin flag");
        }
        assert!(env_from(vars(&[("GEAR_ADMIN", "1")])).unwrap().admin);
    }

    #[test]
    fn missing_db_is_no_target() {
        let err = match open_store_at(vars(&[])) {
            Ok(_) => panic!("GEAR_DB is not set, so opening the store must fail"),
            Err(err) => err,
        };
        assert_eq!(err.code, gear_model::ErrorCode::NoTarget);
        assert!(err.detail.contains("GEAR_DB"), "{}", err.detail);
    }

    // --- T-30: the home page ---

    /// A writer that always fails, standing in for a broken stdout.
    struct FailingWriter;

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Project `p` with an active captain and lane `heat` (both in FakeHerdr).
    fn home_world() -> (Store, FakeHerdr) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, CAP_PANE, "codex", T0)?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)?;
                Ok(())
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
        herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
        (store, herdr)
    }

    fn home_env(seat: &str, pane: &str) -> gear_app::Env {
        gear_app::Env {
            ctx: Some(GearCtx { project: "p".into(), seat: seat.into(), generation: 1 }),
            pane_id: Some(pane.into()),
            admin: false,
        }
    }

    /// An unshown Reply for the lane (`messages_to_since` picks reply/instruction).
    fn unshown_reply(store: &mut Store) {
        store
            .write(|tx| {
                tx.insert_message(&gear_store::NewMessage {
                    meta: None,
                    project: "p".into(),
                    thread: "T-1:1".into(),
                    task: None,
                    attempt: None,
                    sender: "p:captain".into(),
                    sender_gen: 1,
                    recipient: "p:heat".into(),
                    kind: MsgKind::Reply,
                    urgent: false,
                    body: "Use v2".into(),
                    reply_to: None,
                    needs_ack: false,
                    state: MsgState::Info,
                    created: T0 + 1,
                })?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn home_prints_and_records_view() {
        let (mut store, mut herdr) = home_world();
        unshown_reply(&mut store);

        let mut out: Vec<u8> = Vec::new();
        run_home(&mut store, &home_env("heat", LANE_PANE), &mut herdr, T0 + 2, false, &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("seat p:heat"), "{text}");
        assert!(text.contains("Use v2"), "{text}");
        let views = store.read(|tx| tx.views_since("p:heat", 1, 0)).unwrap();
        assert_eq!(views.len(), 1, "the page was printed, so the view is recorded");
        assert_eq!(views[0].shown_msgs, vec!["M-1".to_string()]);
    }

    #[test]
    fn failed_write_records_no_view() {
        let (mut store, mut herdr) = home_world();
        unshown_reply(&mut store);

        let result =
            run_home(&mut store, &home_env("heat", LANE_PANE), &mut herdr, T0 + 2, false, &mut FailingWriter);

        assert!(result.is_err(), "a failed write is reported");
        let views = store.read(|tx| tx.views_since("p:heat", 1, 0)).unwrap();
        assert!(views.is_empty(), "a page that was not printed is not a view");
    }

    /// A task with contract v1 in project `p`.
    fn seed_task(store: &mut Store, id: &str, lane: Option<&str>) {
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
                    repo: None,
                    base: None,
                    gate: None,
                    rework_estimate_min: None,
                    project: "p".into(),
                    id: id.into(),
                    rank: 1,
                    state: gear_model::TaskState::Ready,
                    priority: "must".into(),
                    lane: lane.map(str::to_string),
                    milestone: None,
                    required: false,
                    contract_version: 1,
                    deps: vec![],
                    followup_of: None,
                    wait_on: None,
                    wait_cond: None,
                    rework_note: None,
                    estimate_min: None,
                     issue: None,updated: T0,
                })?;
                tx.insert_contract(&ContractRow {
                    project: "p".into(),
                    task: id.into(),
                    version: 1,
                    goal: "Ship the thing".into(),
                    accept: "The thing is shipped".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                })?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn show_prints_the_packet() {
        let (mut store, mut herdr) = world();
        seed_task(&mut store, "T-1", Some("heat"));

        let out = run_show(&mut store, &env("heat", LANE_PANE), &mut herdr, "T-1", false).unwrap();

        assert!(out.contains("# T-1 — Ship the thing"), "{out}");
        assert!(out.contains("Lane: heat · Contract: v1 · State: ready"), "{out}");
        assert!(out.contains("## Goal"), "{out}");
        assert!(out.contains("## Acceptance"), "{out}");
        assert!(out.contains("gear send --kind result --task T-1"), "{out}");
    }

    /// T-85: a project that keeps a standard (`gear/standard.md`) has it in
    /// every task packet; a project without one is unchanged (the test above).
    #[test]
    fn show_carries_the_projects_standard() {
        let (mut store, mut herdr) = world();
        seed_task(&mut store, "T-1", Some("heat"));
        let dir = std::env::temp_dir().join(format!("gear-standard-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("gear")).expect("the checkout");
        std::fs::write(
            dir.join("gear/standard.md"),
            "# Project standard\n\nNew code meets the standard even if its neighbours do not.\n",
        )
        .expect("the standard");
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.expect("the project");
                project.config_toml = format!(
                    "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = {:?}\n\
                     \n[quota]\nlanes = 1\ntemps = 0\n\
                     \n[captain]\npane = \"{CAP_PANE}\"\nkind = \"codex\"\nprofile = \"codex\"\n",
                    dir.to_string_lossy()
                );
                tx.update_project(&project)
            })
            .unwrap();

        let out = run_show(&mut store, &env("heat", LANE_PANE), &mut herdr, "T-1", false).unwrap();
        assert!(out.contains("# T-1 — Ship the thing"), "{out}");
        assert!(out.contains("## Project standard (`gear/standard.md`)"), "{out}");
        assert!(
            out.contains("New code meets the standard even if its neighbours do not."),
            "{out}"
        );
        // The section is an appendix: the packet's own parts come first.
        assert!(
            out.find("## Project standard").unwrap() > out.find("## Report").unwrap(),
            "{out}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn show_json_is_the_text_as_a_json_string() {
        let (mut store, mut herdr) = world();
        seed_task(&mut store, "T-1", Some("heat"));

        let out = run_show(&mut store, &env("heat", LANE_PANE), &mut herdr, "T-1", true).unwrap();

        let text: String = serde_json::from_str(&out).unwrap();
        assert!(text.contains("# T-1 — Ship the thing"), "{text}");
    }

    #[test]
    fn show_unknown_task_is_no_target() {
        let (mut store, mut herdr) = world();
        let err = run_show(&mut store, &env("heat", LANE_PANE), &mut herdr, "T-9", false).unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::NoTarget);
        assert!(err.detail.contains("T-9"), "{}", err.detail);
    }

    #[test]
    fn show_other_target_is_bad_format() {
        let (mut store, mut herdr) = world();
        for target in ["queue", "draft", "M-1"] {
            let err =
                run_show(&mut store, &env("heat", LANE_PANE), &mut herdr, target, false).unwrap_err();
            assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{target}");
            assert!(err.detail.contains(target), "{target}: {}", err.detail);
        }
    }

    #[test]
    fn given_request_id_is_kept() {
        assert_eq!(announce_request_id(Some("r-abc".into())), "r-abc");
    }

    #[test]
    fn new_request_ids_differ() {
        let a = announce_request_id(None);
        let b = announce_request_id(None);
        assert_ne!(a, b, "{a} / {b}");
        assert!(a.starts_with("r-"), "{a}");
        assert!(b.starts_with("r-"), "{b}");
    }

    /// T-133 (review F1): a release line asks the session which agents are
    /// really running — the store keeps an exited agent's binding `Active` — and
    /// a session that cannot be read refuses the apply instead of guessing.
    #[test]
    fn a_release_asks_the_session_and_refuses_an_unreadable_one() {
        fn lane_state(store: &mut Store) -> gear_model::SeatState {
            store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap().state
        }

        let (mut store, mut herdr) = world();
        let path = write_draft(
            "<!-- gear-plan project=p base=0 -->\n# Staff\n- hold heat reason=the machine is recovering\n",
        );
        run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "hold-1",
            T0 + 1,
            false,
        )
        .unwrap();
        assert_eq!(lane_state(&mut store), gear_model::SeatState::Held);

        // The agent exits; its binding stays Active. The session cannot be read.
        herdr.exit_agent(LANE_PANE);
        std::fs::write(
            &path,
            "<!-- gear-plan project=p base=1 -->\n# Staff\n- release heat\n",
        )
        .unwrap();
        herdr.agents_fail = true;
        let err = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "release-1",
            T0 + 2,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(err.detail.contains("cannot read the session's agents"), "{}", err.detail);
        assert_eq!(lane_state(&mut store), gear_model::SeatState::Held, "nothing applied, still held");

        // Read again: no agent in the binding's pane, so the lane goes back to
        // the queue and the engine onboards it again.
        herdr.agents_fail = false;
        run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "release-2",
            T0 + 3,
            false,
        )
        .unwrap();
        assert_eq!(lane_state(&mut store), gear_model::SeatState::Queued);
        std::fs::remove_file(&path).ok();
    }

}
