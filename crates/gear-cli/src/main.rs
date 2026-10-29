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
        /// The project to show. Without it the caller's own project is shown,
        /// and with no caller every project in the store.
        #[arg(long, value_name = "ID")]
        project: Option<String>,
    },
    /// Captain: the queue of the project (design §8.3) — what every lane
    /// does now, why an idle lane is idle, and what it has left.
    Queue {
        /// The queue as JSON.
        #[arg(long)]
        json: bool,
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
    /// Result of a direction check, as text (not a task hand-in).
    #[arg(long, value_name = "TEXT", conflicts_with = "body")]
    direction: Option<String>,
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
    /// Captain only: the instruction is confirmed by an ack, and the item
    /// comes back if it is not (design §10.7, row B7).
    #[arg(long)]
    ack: bool,
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
        /// The user asked for this goal, bar or eta change and confirmed it.
        #[arg(long)]
        confirmed: bool,
    },
    /// Print one task's block as it would stand in the plan file (design §8.3).
    Show {
        /// The task to show (T-n).
        task: String,
    },
    /// End the parks whose time has come (T-190): a lane the plan parked
    /// until a time that has passed works again.
    Check {},
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
    Init {
        config: std::path::PathBuf,
        /// Validate the config and change nothing: no store, no Herdr, no
        /// engine (T-228).
        #[arg(long)]
        check: bool,
        /// With `--check`: ask each CLI for its models and warn about a
        /// configured model id its CLI does not list (T-315). Asking runs the
        /// CLIs, so it never happens unless asked for.
        #[arg(long, requires = "check")]
        models: bool,
        /// The user asked for this staffing change and confirmed it
        /// (T-401): a mid-voyage change to the staffing of a departed voyage
        /// applies only on their confirmation, and GEAR stores no words of
        /// theirs.
        #[arg(long)]
        confirmed: bool,
    },
    /// Print what this machine offers, for the captain's kickoff proposal.
    Kickoff,
    /// Write the GEAR skill where an agent CLI loads its skills.
    InstallSkill {
        /// The agent CLI: claude | codex | opencode | grok | pi.
        #[arg(long)]
        agent: String,
    },
    /// Stop the background engine (it starts again on the next `gear` call).
    Stop {
        /// Keep it stopped: while `engine.hold` exists no `gear` call starts
        /// the engine until `gear admin start` (T-394).
        #[arg(long)]
        hold: bool,
    },
    /// Lift a hold (`admin stop --hold`) and start the background engine.
    Start,
    /// Every store on this machine and the projects in it (T-322): the shared
    /// store, one per Herdr session, and the registered ones. Read-only.
    Projects,
}

fn main() {
    // The store path is set after the parse: `--help` and `--version` answer
    // without touching the user's home (design/13: a read-only command writes
    // nothing).
    let cli = Cli::parse();
    // T-228 (review F1): `admin init --check` validates a config and changes
    // nothing, and that includes the directories of the default store.
    if !writes_nothing(&cli.cmd) {
        default_store_path();
    }
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ERROR {}: {}", e.code, e.detail);
            e.code.exit_code()
        }
    };
    std::process::exit(code);
}

/// True for the commands that write nothing at all, not even the directory of
/// the default store: the read-only validation `gear admin init --check` and
/// `gear admin projects`, which only reads the stores already on this machine
/// (T-322).
fn writes_nothing(cmd: &Option<Cmd>) -> bool {
    matches!(
        cmd,
        Some(Cmd::Admin { op: AdminOp::Init { check: true, .. } })
            | Some(Cmd::Admin { op: AdminOp::Projects })
    )
}

/// Where GEAR keeps its store when `GEAR_DB` is not set: one store per Herdr
/// session, since an engine drives one session and pane ids are only unique
/// inside one. The store registered for the caller's session comes first
/// (`~/.config/gear/targets.tsv`); else a named session `<name>` gets
/// `~/.local/state/gear/<name>/gear.db`, and no session or the default one
/// `~/.local/state/gear/gear.db`. Set once, before anything reads it, so this
/// process, the engine it starts and the agents the engine starts all agree.
fn default_store_path() {
    if std::env::var_os(gear_model::ctx::ENV_DB).is_some() {
        return;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        let socket = std::env::var("HERDR_SOCKET_PATH").ok();
        let db = default_store_in(&home, socket.as_deref());
        if let Some(dir) = db.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::env::set_var(gear_model::ctx::ENV_DB, db);
    }
}

/// The default store under `home` for the Herdr session at `socket`.
fn default_store_in(home: &std::path::Path, socket: Option<&str>) -> std::path::PathBuf {
    let shared = home.join(".local/state/gear/gear.db");
    let Some(socket) = socket else { return shared };
    if let Some(db) = registered_store(&targets_path_in(home), socket) {
        return std::path::PathBuf::from(db);
    }
    match session_name_of(home, socket) {
        Some(name) => home.join(".local/state/gear").join(name).join("gear.db"),
        None => shared,
    }
}

/// The name of a named Herdr session from its socket
/// (`~/.config/herdr/sessions/<name>/herdr.sock`); None for the default one.
fn session_name_of(home: &std::path::Path, socket: &str) -> Option<String> {
    let rest = std::path::Path::new(socket).strip_prefix(home.join(".config/herdr/sessions")).ok()?;
    let mut parts = rest.components();
    let name = parts.next()?.as_os_str().to_str()?.to_string();
    (parts.as_path() == std::path::Path::new("herdr.sock")).then_some(name)
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
            PlanOp::Apply { path, keep, next, request_id, confirmed } => {
                cmd_plan(path, keep, next, request_id, confirmed, cli.json)
            }
            PlanOp::Show { task } => cmd_plan_show(&task),
            PlanOp::Check {} => cmd_plan_check(cli.json),
            PlanOp::ImportIssues { path, label } => {
                cmd_import_issues(path, label, cli.json)
            }
        },
        Some(Cmd::Board { deps, project }) => cmd_board(deps, project.as_deref(), cli.json),
        Some(Cmd::Queue { json }) => cmd_queue(json),
        Some(Cmd::Report { op }) => match op {
            ReportOp::Time { task, voyage } => cmd_report_time(task, voyage, cli.json),
            ReportOp::Voyage { voyage, all } => cmd_report_voyage(voyage, all, cli.json),
        },
        Some(Cmd::Run) => cmd_run(),
        Some(Cmd::Friction(c)) => cmd_friction(c, cli.json),
        Some(Cmd::Admin { op: AdminOp::Init { config, check, models, confirmed } }) => {
            cmd_admin_init(&config, check, models, confirmed, cli.json)
        }
        Some(Cmd::Admin { op: AdminOp::Kickoff }) => cmd_admin_kickoff(cli.json),
        Some(Cmd::Admin { op: AdminOp::InstallSkill { agent } }) => {
            cmd_admin_install_skill(&agent, cli.json)
        }
        Some(Cmd::Admin { op: AdminOp::Stop { hold } }) => cmd_admin_stop(hold),
        Some(Cmd::Admin { op: AdminOp::Start }) => cmd_admin_start(),
        Some(Cmd::Admin { op: AdminOp::Projects }) => cmd_admin_projects(cli.json),
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
/// - `run` *is* the engine, and `admin stop` stops it, `admin start` starts
///   it (T-394);
/// - `board` only observes — starting the engine would hide the "engine down"
///   warning it exists to show (PRD §15, the review of T-3);
/// - `report time` only observes (the time accounting of what the store
///   already knows);
/// - `admin kickoff` only reads — starting the engine would open (and migrate)
///   the store, and a kickoff runs before the project exists (the review of
///   T-10). `--skill` and `--help` never get here.
/// - `captain handover` moves a seat's binding: it must not start an engine
///   (T-104);
/// - `admin init --check` only validates a config: it reads the store's other
///   projects for the branch check but writes nothing, so there is nothing to
///   keep moving (T-228, T-256);
/// - `admin projects` only reads the stores on this machine (T-322): an engine
///   would open one of them and write it.
fn starts_engine(cmd: &Option<Cmd>) -> bool {
    match cmd {
        None => true,
        Some(Cmd::Admin { op: AdminOp::Init { check, .. } }) => !check,
        Some(Cmd::Run)
        | Some(Cmd::Admin { op: AdminOp::Stop { .. } })
        | Some(Cmd::Admin { op: AdminOp::Start })
        | Some(Cmd::Admin { op: AdminOp::Kickoff })
        | Some(Cmd::Admin { op: AdminOp::Projects })
        | Some(Cmd::Admin { op: AdminOp::InstallSkill { .. } })
        | Some(Cmd::Board { .. })
        | Some(Cmd::Report { .. })
        | Some(Cmd::Whoami)
        | Some(Cmd::Plan { op: PlanOp::ImportIssues { .. } })
        | Some(Cmd::Captain { .. }) => false,
        Some(_) => true,
    }
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

/// `gear admin projects`: every store on this machine and the projects in it
/// (T-322).
///
/// Since v0.4.5 each Herdr session has its own store ([`default_store_in`]),
/// and nothing listed them together. This does, and nothing else: the stores
/// are the shared one, every `~/.local/state/gear/<session>/gear.db` and every
/// store the registry names, existing files only, each once (by canonical
/// path), in path order. Each is opened read-only, so nothing is written and
/// no engine starts.
fn cmd_admin_projects(json: bool) -> GearResult<()> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let stores: Vec<StoreProjects> = stores_in(home.as_deref())
        .into_iter()
        .map(|path| {
            let store = path.to_string_lossy().into_owned();
            match projects_in_store(&path) {
                Ok(projects) => StoreProjects { store, projects, error: None },
                Err(detail) => StoreProjects { store, projects: Vec::new(), error: Some(detail) },
            }
        })
        .collect();
    if json {
        println!("{}", stores_json(&stores)?);
    } else {
        let out = render_stores(&stores);
        if !out.is_empty() {
            println!("{out}");
        }
    }
    Ok(())
}

/// One store as `gear admin projects` reports it: what it holds, or why it
/// could not be read.
struct StoreProjects {
    store: String,
    projects: Vec<ProjectSummary>,
    error: Option<String>,
}

/// What `gear admin projects` says about one project.
struct ProjectSummary {
    id: String,
    session: String,
    config_version: i64,
    /// The voyage the project is in; empty when it has none.
    voyage: String,
}

impl ProjectSummary {
    /// The voyage as printed: the id, or `-` when the project is in none.
    fn voyage_or_dash(&self) -> &str {
        if self.voyage.is_empty() { "-" } else { &self.voyage }
    }
}

/// The stores of this machine, in path order: the shared store, every
/// `~/.local/state/gear/<session>/gear.db` and every store the registry names
/// ([`targets_path_in`]). Existing files only, each once: deduplicated by
/// canonical path, which is also what is printed.
fn stores_in(home: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let Some(home) = home else { return Vec::new() };
    let state = home.join(".local/state/gear");
    let mut named = vec![state.join("gear.db")];
    if let Ok(entries) = std::fs::read_dir(&state) {
        for entry in entries.flatten() {
            named.push(entry.path().join("gear.db"));
        }
    }
    if let Ok(text) = std::fs::read_to_string(targets_path_in(home)) {
        for line in text.lines() {
            if let Some((_, db)) = line.split_once('\t') {
                named.push(std::path::PathBuf::from(db));
            }
        }
    }
    let mut paths: std::collections::BTreeSet<std::path::PathBuf> = std::collections::BTreeSet::new();
    for path in named {
        // Canonicalize already tells whether the file exists, and dedups a
        // store reached twice (a session directory and a registry line).
        if let Ok(real) = std::fs::canonicalize(&path) {
            if real.is_file() {
                paths.insert(real);
            }
        }
    }
    paths.into_iter().collect()
}

/// The projects of the store at `path`, read-only; `Err` is why it was not
/// readable (`gear admin projects` prints it and goes on).
fn projects_in_store(path: &std::path::Path) -> Result<Vec<ProjectSummary>, String> {
    let mut store = gear_store::Store::open_read_only(path).map_err(|e| e.detail)?;
    store
        .read(|tx| {
            let mut projects = Vec::new();
            for id in tx.project_ids()? {
                let Some(row) = tx.get_project(&id)? else { continue };
                projects.push(ProjectSummary {
                    session: row.herdr_session,
                    config_version: row.config_version,
                    voyage: gear_app::plan::current_voyage(tx, &id)?,
                    id: row.id,
                });
            }
            Ok(projects)
        })
        .map_err(|e| e.detail)
}

/// [`StoreProjects`] as lines: the store's path, then one line per project, or
/// `(no projects)`, or `cannot read: <detail>`.
fn render_stores(stores: &[StoreProjects]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for store in stores {
        lines.push(store.store.clone());
        if let Some(detail) = &store.error {
            lines.push(format!("  cannot read: {detail}"));
            continue;
        }
        if store.projects.is_empty() {
            lines.push("  (no projects)".to_string());
            continue;
        }
        for project in &store.projects {
            lines.push(format!(
                "  {} · session {} · config v{} · voyage {}",
                project.id,
                project.session,
                project.config_version,
                project.voyage_or_dash()
            ));
        }
    }
    lines.join("\n")
}

/// [`StoreProjects`] as JSON: `[{"store", "projects": [{"id", "session",
/// "config_version", "voyage"}], "error"}]`.
fn stores_json(stores: &[StoreProjects]) -> GearResult<String> {
    let doc: Vec<serde_json::Value> = stores
        .iter()
        .map(|store| {
            let projects: Vec<serde_json::Value> = store
                .projects
                .iter()
                .map(|project| {
                    serde_json::json!({
                        "id": project.id,
                        "session": project.session,
                        "config_version": project.config_version,
                        "voyage": project.voyage_or_dash(),
                    })
                })
                .collect();
            serde_json::json!({
                "store": store.store,
                "projects": projects,
                "error": store.error,
            })
        })
        .collect();
    serde_json::to_string(&doc).map_err(json_error)
}

/// Refuse a **first** registration from another project's seat (T-100).
///
/// The captain registers its project (user, M4 landing: the whole flow is the
/// captain's; `GEAR_ADMIN=1` is the user's consent, given at kickoff). A seat
/// of a project — a lane or reviewer with `GEAR_CTX` — never creates a new one.
fn refuse_first_registration_by_an_agent(env: &gear_app::Env) -> GearResult<()> {
    if let Some(ctx) = &env.ctx {
        return Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            format!(
                "a seat of a project never registers another: this is seat {} of project {} (a config change for a registered project may be re-applied from its captain's pane)",
                ctx.seat_id(),
                ctx.project
            ),
        ));
    }
    Ok(())
}

/// `gear admin init <config>`: register a project — from the user's own shell
/// or from the captain's pane (T-86). `GEAR_ADMIN=1` is the user's consent and
/// is checked before anything is read; which seat may act on it is decided by
/// `gear_app::admin::init`, against the project the config names. The store is
/// `GEAR_DB`.
///
/// With `--check` (T-228) it only validates the config: `GEAR_ADMIN` is not
/// needed, no store is written, nothing starts, and the command says what it
/// read (`config ok: project <id>, <n> lanes, <m> reviewers`). Its
/// cross-project branch check (T-256) reads an existing store, never creates
/// one, and says so when there is none.
///
/// Both forms go on to name the seats the config starts and the model each
/// one runs (T-261), so a captain sees a wrong model here rather than once the
/// fleet runs.
///
/// With `--models` (T-315) it also asks each seat's CLI for the models it
/// lists and warns, on stderr, about a configured model id that is not among
/// them — a warning, not a refusal, and never without `--models`, since asking
/// runs the CLIs.
///
/// With `--confirmed` (T-401) the user's confirmation of a staffing change to
/// a departed voyage is carried; without it such a change is refused
/// (T-392), in the check as well, and GEAR stores no words of theirs.
fn cmd_admin_init(
    config: &std::path::Path,
    check_only: bool,
    with_models: bool,
    confirmed: bool,
    json: bool,
) -> GearResult<()> {
    // T-261: the lane list — `(name, kind, model)` — with the model being the
    // lane's first `models` entry, else the role chain's first, else "-".
    let lane_list = |parsed: &gear_app::config::ProjectConfig| -> Vec<(String, String, String)> {
        parsed
            .lanes
            .iter()
            .map(|lane| {
                let model = lane
                    .models
                    .first()
                    .or_else(|| parsed.models.lanes.first())
                    .map(|entry| entry.model().to_string())
                    .unwrap_or_else(|| "-".to_string());
                (lane.name.clone(), lane.kind.clone(), model)
            })
            .collect()
    };
    // The model a reviewer runs: the first `[models] review` entry, else "-".
    let review_model = |parsed: &gear_app::config::ProjectConfig| -> String {
        parsed
            .models
            .review
            .first()
            .map(|entry| entry.model().to_string())
            .unwrap_or_else(|| "-".to_string())
    };
    let lane_json = |lanes: &[(String, String, String)]| -> Vec<serde_json::Value> {
        lanes
            .iter()
            .map(|(name, kind, model)| {
                serde_json::json!({ "name": name, "kind": kind, "model": model })
            })
            .collect()
    };
    // The same facts as lines: one per lane, then the reviewers.
    let seat_lines = |parsed: &gear_app::config::ProjectConfig| -> Vec<String> {
        let mut lines: Vec<String> = parsed
            .lanes
            .iter()
            .zip(lane_list(parsed))
            .map(|(lane, (name, kind, model))| {
                format!("  lane {name}: {kind} {model} · owns {}", lane.owns.join(", "))
            })
            .collect();
        lines.push(match &parsed.temps {
            Some(temps) => format!(
                "  reviewers: {} × {} {}",
                parsed.quota.temps,
                temps.kind,
                review_model(parsed)
            ),
            None => "  reviewers: none".to_string(),
        });
        lines
    };
    // T-269: one store per Herdr session (v0.4.5). A config that names
    // another session would register into this session's store, where its
    // engine never reaches the panes — so it is refused before `--check`
    // prints "config ok" and before a real init writes anything. Nothing is
    // checked without a socket, for the default session, or when the caller
    // names a store of its own with `GEAR_DB`.
    let refuse_a_foreign_session =
        |parsed: &gear_app::config::ProjectConfig| -> GearResult<()> {
            let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
                return Ok(());
            };
            let Ok(socket) = std::env::var("HERDR_SOCKET_PATH") else {
                return Ok(());
            };
            let Some(name) = session_name_of(&home, &socket) else {
                return Ok(());
            };
            // `default_store_path` fills `GEAR_DB` in with this session's own
            // default store: a caller that named another store itself is
            // checked by nothing, since that store need not be a session's.
            if let Some(db) = std::env::var_os(gear_model::ctx::ENV_DB) {
                if std::path::PathBuf::from(db) != default_store_in(&home, Some(&socket)) {
                    return Ok(());
                }
            }
            let configured = &parsed.project.herdr_session;
            if *configured == name {
                return Ok(());
            }
            Err(GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!(
                    "project {} names herdr_session {configured}, but this shell runs in session {name}: its store and engine are this session's; run init from a pane of session {configured}, or fix herdr_session",
                    parsed.project.id
                ),
            ))
        };
    // T-316: a repository under the system temp directory is a fixture (M4.2
    // #35: a leftover fixture project with empty tokens appeared in the
    // user's board, its captain seat bound to the real captain's pane). A
    // fixture registers only into a store the caller names itself with
    // `GEAR_DB`, never into this session's store, where the user would see
    // it. Like T-269, nothing is refused when the caller names its own store.
    let refuse_a_fixture_into_this_store =
        |parsed: &gear_app::config::ProjectConfig| -> GearResult<()> {
            let temp = std::env::temp_dir().to_string_lossy().into_owned();
            let temp = canonical(&temp);
            let repo = canonical(&parsed.project.repo);
            if !std::path::Path::new(&repo).starts_with(&temp) {
                return Ok(());
            }
            let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
                return Ok(());
            };
            let socket = std::env::var("HERDR_SOCKET_PATH").ok();
            // `default_store_path` fills `GEAR_DB` in with this session's own
            // default store: a `GEAR_DB` that differs was named by the caller.
            let db = match std::env::var_os(gear_model::ctx::ENV_DB) {
                Some(db) => {
                    let db = std::path::PathBuf::from(db);
                    if db != default_store_in(&home, socket.as_deref()) {
                        return Ok(());
                    }
                    db
                }
                None => default_store_in(&home, socket.as_deref()),
            };
            Err(GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!(
                    "project {} lives under the temp directory ({repo}): a fixture registers only into a store named with GEAR_DB, never into this session's store {}",
                    parsed.project.id,
                    db.display()
                ),
            ))
        };
    // T-228: `--check` validates the config and changes nothing — no
    // GEAR_ADMIN, no store, no Herdr, no engine.
    if check_only {
        let text = read_config(config)?;
        let parsed = gear_app::admin::check(&text, None)?;
        refuse_a_foreign_session(&parsed)?;
        // T-256: another project of this store on the same repository is a
        // branch clash, reported before anything is registered. The store is
        // only read, and only when it exists already: a check creates nothing.
        // T-388: the same read gives the config this store holds for this very
        // project, which is what a re-init would change. A store with no
        // project of this id is a first registration: nothing to compare.
        let changes: Option<(i64, Vec<String>)> = match other_projects()? {
            Some(others) => {
                let pairs: Vec<(String, String)> = others
                    .iter()
                    .map(|(id, text, _)| (id.clone(), text.clone()))
                    .collect();
                if let Some(problem) = gear_app::admin::branch_clash(&parsed, &pairs) {
                    return Err(GearError::new(gear_model::ErrorCode::BadFormat, problem));
                }
                others
                    .iter()
                    .find(|(id, _, _)| *id == parsed.project.id)
                    .map(|(_, registered, version)| (*version, config_changes(registered, &text)))
            }
            None => {
                eprintln!("cross-project check skipped: no store");
                None
            }
        };
        // T-304: the lane quota counts the lanes that can run — the seats,
        // retired ones excluded — so it is checked after the branch check,
        // against the retired lanes of this project's seats.
        if let Some(problem) =
            gear_app::admin::lane_quota_problem(&parsed, &retired_lanes_of(&parsed.project.id)?)
        {
            return Err(GearError::new(gear_model::ErrorCode::BadFormat, problem));
        }
        // T-392: a departed voyage's staffing changes only on the user's
        // confirmation (T-401). The check reports the same refusal a real
        // init would write, and writes nothing itself.
        if !confirmed {
            if let Some(problem) = staffing_refusal_of(&parsed)? {
                return Err(problem);
            }
        }
        let lanes = parsed.lanes.len();
        let reviewers = parsed.quota.temps;
        // T-315: only when asked for — this runs the CLIs.
        let model_warnings: Vec<String> = if with_models {
            gear_app::kickoff::model_warnings(&parsed, &mut |cli: &str| {
                gear_app::kickoff::models_of(cli)
            })
        } else {
            Vec::new()
        };
        if json {
            let lane_list = lane_list(&parsed);
            let changes = changes
                .as_ref()
                .map(|(_, lines)| lines.clone())
                .unwrap_or_default();
            println!(
                "{}",
                serde_json::json!({
                    "ok": true,
                    "project": parsed.project.id,
                    "lanes": lanes,
                    "reviewers": reviewers,
                    "lane_list": lane_json(&lane_list),
                    "review_model": review_model(&parsed),
                    "model_warnings": model_warnings,
                    "changes": changes,
                })
            );
        } else {
            println!(
                "config ok: project {}, {lanes} lanes, {reviewers} reviewers",
                parsed.project.id
            );
            // T-388: right after it, what the re-init would change (or that
            // it changes nothing), so a captain sees the store it writes.
            if let Some((version, lines)) = &changes {
                if lines.is_empty() {
                    println!("no change against the registered config v{version}");
                } else {
                    println!("changes against the registered config v{version}:");
                    for line in lines {
                        println!("{line}");
                    }
                }
            }
            for line in seat_lines(&parsed) {
                println!("{line}");
            }
            for line in &model_warnings {
                eprintln!("warning: {line}");
            }
        }
        return Ok(());
    }
    let env = env_from_process()?;
    if !env.admin {
        return Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            "`gear admin init` needs GEAR_ADMIN=1: the user's own shell, or the captain's pane",
        ));
    }
    let text = read_config(config)?;
    // T-269: a config for another session never lands in this one's store.
    if let Ok(parsed) = gear_app::config::parse(&text) {
        refuse_a_foreign_session(&parsed)?;
        // T-316: nor does a fixture, unless the caller named its store.
        refuse_a_fixture_into_this_store(&parsed)?;
    }
    let mut store = open_store()?;
    // A project that is not registered yet is created by its captain, never by
    // another project's seat (T-100); a seat may re-apply its own project's
    // config (T-86). Refused before anything is written, and it rings nobody.
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
        confirmed,
        now_ts(),
    )?;
    // T-321: remember which file this config was applied from, so the home
    // page can tell a config file that drifted from the config the project
    // runs on. Canonical, so the meta names the same file every run.
    let path = canonical(&config.to_string_lossy());
    store.write(|tx| tx.set_meta(&gear_app::admin::config_path_key(&r.project), &path))?;
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
    // The config parsed already (init validated it): say which seats it
    // starts, and on which model (T-261).
    let parsed = gear_app::config::parse(&text)?;
    let lanes = lane_list(&parsed);
    if json {
        let mut value = serde_json::to_value(&r).unwrap_or_default();
        if let Some(object) = value.as_object_mut() {
            object.insert("lane_list".to_string(), serde_json::json!(lane_json(&lanes)));
            object.insert("review_model".to_string(), serde_json::json!(review_model(&parsed)));
        }
        println!("{value}");
    } else {
        // The project is registered with this config version; a re-init
        // stores the next one (T-6).
        println!("project {} (config v{})", r.project, r.config_version);
        // T-373: a changed `conventions.seat_branch` leaves worktrees behind;
        // name them and the command that takes each one away.
        for line in &r.stale_worktrees {
            println!("{line}");
        }
        for seat in &r.seats {
            println!("seat {seat}");
        }
        for line in seat_lines(&parsed) {
            println!("{line}");
        }
    }
    Ok(())
}

/// The lines of a `--check` change list (T-388): the registered config text
/// against the config file's own text, as a line diff — `- ` for a line only
/// in the registered one, `+ ` for a line only in the file, in file order.
/// The LCS is a plain table, small next to a config file, so no dependency.
fn config_changes(registered: &str, file: &str) -> Vec<String> {
    let old: Vec<&str> = registered.lines().collect();
    let new: Vec<&str> = file.lines().collect();
    // `same[i][j]`: the length of the longest common subsequence of the tails
    // `old[i..]` and `new[j..]`.
    let mut same = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            same[i][j] = if old[i] == new[j] {
                same[i + 1][j + 1] + 1
            } else {
                same[i + 1][j].max(same[i][j + 1])
            };
        }
    }
    let mut lines = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            i += 1;
            j += 1;
        } else if same[i + 1][j] >= same[i][j + 1] {
            // A tie takes the registered line away first, as a diff does.
            lines.push(format!("- {}", old[i]));
            i += 1;
        } else {
            lines.push(format!("+ {}", new[j]));
            j += 1;
        }
    }
    for line in &old[i..] {
        lines.push(format!("- {line}"));
    }
    for line in &new[j..] {
        lines.push(format!("+ {line}"));
    }
    lines
}

/// Read the config a command was given, as `BadFormat` when it cannot be read.
fn read_config(config: &std::path::Path) -> GearResult<String> {
    std::fs::read_to_string(config).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read {}: {e}", config.display()),
        )
    })
}

/// The other projects of the store this `gear` call would use — `(id,
/// config_toml, config_version)` — for the branch check of `admin init
/// --check` (T-256) and its change list (T-388), or `None` when there is no
/// store to read: none at the path, or no `HOME` to find the default one
/// under. The path is `GEAR_DB` when the caller names
/// one, else the default store of this session — resolved with
/// [`default_store_in`], never with [`default_store_path`], which creates the
/// directory. Only an existing store is opened, and read-only: a check creates
/// no file, applies no schema and migrates nothing (T-228).
fn other_projects() -> GearResult<Option<Vec<(String, String, i64)>>> {
    let path = match std::env::var_os(gear_model::ctx::ENV_DB) {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let Some(home) = std::env::var_os("HOME") else {
                return Ok(None);
            };
            let socket = std::env::var("HERDR_SOCKET_PATH").ok();
            default_store_in(&std::path::PathBuf::from(home), socket.as_deref())
        }
    };
    if !path.exists() {
        return Ok(None);
    }
    let mut store = gear_store::Store::open_read_only(&path)?;
    store
        .read(|tx| {
            let mut others = Vec::new();
            for id in tx.project_ids()? {
                if let Some(row) = tx.get_project(&id)? {
                    others.push((row.id, row.config_toml, row.config_version));
                }
            }
            Ok(others)
        })
        .map(Some)
}

/// The staffing refusal `admin init --check` reports for a registered project
/// whose departed voyage is under way and whose config changes `quota`,
/// `lanes`, `temps` or `models` (T-392), or `None` when nothing is locked. It
/// reads the same store `other_projects` does, with the same path rule,
/// read-only.
fn staffing_refusal_of(parsed: &gear_app::config::ProjectConfig) -> GearResult<Option<GearError>> {
    let path = match std::env::var_os(gear_model::ctx::ENV_DB) {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let Some(home) = std::env::var_os("HOME") else {
                return Ok(None);
            };
            let socket = std::env::var("HERDR_SOCKET_PATH").ok();
            default_store_in(&std::path::PathBuf::from(home), socket.as_deref())
        }
    };
    if !path.exists() {
        return Ok(None);
    }
    let mut store = gear_store::Store::open_read_only(&path)?;
    store.read(|tx| {
        let Some(row) = tx.get_project(&parsed.project.id)? else {
            return Ok(None);
        };
        let Ok(stored) = gear_app::config::parse(&row.config_toml) else {
            return Ok(None);
        };
        let changes = gear_app::admin::staffing_changes(&stored, parsed);
        if !gear_app::admin::staffing_changed(&stored, parsed) {
            return Ok(None);
        }
        let Some(voyage) = gear_app::admin::staffing_voyage(tx, &parsed.project.id)? else {
            return Ok(None);
        };
        Ok(Some(GearError::new(
            gear_model::ErrorCode::StaffLocked,
            gear_app::admin::staffing_refusal(&voyage, &changes),
        )))
    })
}

/// The lane names this project's seats hold as `Retired` (T-304), for the lane
/// quota check of `admin init --check`. It reads the same store `other_projects`
/// does, with the same path rule, read-only, and is empty when there is no
/// store to read (a check creates nothing).
fn retired_lanes_of(project: &str) -> GearResult<Vec<String>> {
    let path = match std::env::var_os(gear_model::ctx::ENV_DB) {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let Some(home) = std::env::var_os("HOME") else {
                return Ok(Vec::new());
            };
            let socket = std::env::var("HERDR_SOCKET_PATH").ok();
            default_store_in(&std::path::PathBuf::from(home), socket.as_deref())
        }
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut store = gear_store::Store::open_read_only(&path)?;
    store.read(|tx| {
        let mut names = Vec::new();
        for seat in tx.seats_of_project(project)? {
            if seat.role == gear_model::SeatRole::Lane
                && seat.state == gear_model::SeatState::Retired
            {
                names.push(seat.name);
            }
        }
        Ok(names)
    })
}

/// Open the store at GEAR_DB (agents) or the admin target (design/14).
fn open_store() -> GearResult<gear_store::Store> {
    open_store_at(|name| std::env::var(name).ok())
}

/// [`open_store`] over an injected variable lookup (testable).
/// The caller's identity (T-104): the library proves the whole claim — the
/// pane the caller really runs in is the claimed one, and the seat is bound to
/// it — and reports a refusal once to the captain. A caller whose tools run in
/// a daemon (T-328) also proves it is the seat's own session.
fn observe_identity(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
) -> GearResult<Option<gear_app::identity::Fingerprint>> {
    let ancestors = gear_app::identity::process_ancestors();
    let daemon = gear_app::identity::DaemonSessions::from_vars(|name| std::env::var(name).ok());
    gear_app::identity::prove_seat(store, env, herdr, &ancestors, &daemon, now_ts())
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
/// `gear queue`: the captain's plain view of the work (design §8.3) — every
/// lane, what it does now, why an idle lane is idle, and what it has left.
///
/// Read-only, so it starts no engine.
fn cmd_queue(json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let out = run_queue(&mut store, &env, json, now_ts())?;
    println!("{out}");
    Ok(())
}

/// `gear queue` body, over injected state.
fn run_queue(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    json: bool,
    now: i64,
) -> GearResult<String> {
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
    let queue = gear_app::queue::build(store, &project, now)?;
    if json {
        return serde_json::to_string(&queue).map_err(json_error);
    }
    Ok(gear_app::queue::render(&queue))
}

/// `gear plan show T-n`: one task's block as it would stand in the plan file
/// (design §8.3).
///
/// Read-only, so it starts no engine.
fn cmd_plan_show(task: &str) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
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
    let out = store.read(|tx| gear_app::plan::show(tx, &project, task))?;
    println!("{out}");
    Ok(())
}

/// `gear board [--deps] [--project <id>]`: the read-only plan board.
///
/// `--project` names the project to show and wins over the caller's own: a
/// pane bound to no seat (the user's own board tab) has no caller, so without
/// the flag the board shows every project in the store (T-249).
fn cmd_board(deps: bool, project: Option<&str>, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr().ok();
    // A project that is not registered is refused before the board starts: a
    // typo must not draw an empty or, worse, a wrong board.
    if let Some(id) = project {
        let found = store.read(|tx| tx.get_project(id))?;
        if found.is_none() {
            let registered = store.read(|tx| tx.project_ids())?;
            return Err(GearError::new(
                gear_model::ErrorCode::NoTarget,
                format!(
                    "no project {id} in this store; registered: {}",
                    registered.join(", ")
                ),
            ));
        }
    }
    // The caller's seat, when it has one: a captain in a pane sees its own
    // project's board without a GEAR_CTX (T-126).
    let env = match herdr.as_mut() {
        Some(herdr) => resolve_caller(&mut store, herdr, env)?,
        None => env,
    };
    let opts = gear_board::Options {
        // The project named on the command line, else the caller's; without
        // either (the user's own shell) the board shows every project in the
        // store.
        project: project
            .map(str::to_string)
            .or_else(|| env.ctx.as_ref().map(|ctx| ctx.project.clone())),
        // The engine's lock file sits next to the database.
        db: std::env::var_os(gear_model::ctx::ENV_DB).map(std::path::PathBuf::from),
        json,
        // The dependency tree instead of the queues of the lanes.
        view: if deps { gear_board::View::Deps } else { gear_board::View::Lanes },
        ..Default::default()
    };
    let mut out = std::io::stdout();
    draw_board(&mut store, &opts, herdr.as_mut(), herdr_tab_id(), &mut out)
}

/// The Herdr tab this call runs in (`HERDR_TAB_ID`), when Herdr set one. The
/// user's shell, a terminal outside Herdr, has none.
fn herdr_tab_id() -> Option<String> {
    std::env::var("HERDR_TAB_ID").ok().filter(|id| !id.is_empty())
}

/// The label the board gives its Herdr tab (T-278): `<project> board`, or
/// `GEAR board` when the board shows every project.
fn board_tab_label(project: Option<&str>) -> String {
    match project {
        Some(project) => format!("{project} board"),
        None => "GEAR board".into(),
    }
}

/// Name the caller's Herdr tab after the board, then draw the board (T-278):
/// a captain opens the board in a new tab, and an unnamed tab shows a number in
/// the tab bar. A refused or uncertain rename is ignored — the board still
/// starts. `--json` is machine output and renames nothing.
fn draw_board<H: gear_herdr::HerdrAct>(
    store: &mut gear_store::Store,
    opts: &gear_board::Options,
    herdr: Option<&mut H>,
    tab_id: Option<String>,
    out: &mut dyn std::io::Write,
) -> GearResult<()> {
    let mut herdr = herdr;
    if !opts.json {
        if let (Some(herdr), Some(tab_id)) = (herdr.as_mut(), tab_id.as_deref()) {
            let label = board_tab_label(opts.project.as_deref());
            let _ = herdr.tab_rename(tab_id, &label);
        }
    }
    gear_board::run(store, opts, out, herdr.map(|h| h as &mut dyn gear_herdr::HerdrRead))
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

/// The counted usage of the open rounds of a project (T-207).
///
/// A round that is still running has no counted tokens in the store yet: the
/// report counts it as it reads (design §4.3) and passes the count to the
/// report as an override. Nothing is written — a closed round keeps its
/// stored numbers, and the store is left as it was.
fn open_round_usage(
    store: &mut gear_store::Store,
    cfg: &gear_engine::EngineCfg,
    project: &str,
    now: i64,
) -> gear_app::time::UsageOverrides {
    let mut overrides = gear_app::time::UsageOverrides::default();
    let rounds = match store.read(|tx| tx.rounds_of_project(project)) {
        Ok(rounds) => rounds,
        Err(error) => {
            eprintln!("gear: cannot read the rounds of {project}: {error}");
            return overrides;
        }
    };
    for round in rounds.iter().filter(|round| round.end_at.is_none()) {
        match gear_engine::rounds::count_round(store, cfg, round, now) {
            Ok(counted) => {
                // A log that cannot be read leaves the round unknown: the
                // store's own numbers stand (a round that was never counted
                // has none, and stays a question mark).
                if counted.input.is_some() {
                    overrides.set(&round.task, round.attempt, &round.role, (&counted).into());
                }
            }
            // A round that cannot be counted stays unknown: the report shows
            // a question mark, never a zero.
            Err(error) => eprintln!(
                "gear: cannot count the open round {} attempt {} {}: {error}",
                round.task, round.attempt, round.role
            ),
        }
    }
    overrides
}

/// The counted usage of the open rounds of the caller's project (T-207).
fn caller_open_round_usage(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    now: i64,
) -> gear_app::time::UsageOverrides {
    let Some(project) = env.ctx.as_ref().map(|ctx| ctx.project.clone()) else {
        return gear_app::time::UsageOverrides::default();
    };
    open_round_usage(store, &report_cfg(), &project, now)
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
    let overrides = caller_open_round_usage(store, env, now);
    let report = gear_app::voyage::build_with(
        store,
        env,
        fp.as_ref(),
        &milestone,
        all,
        now,
        &overrides,
    )?;
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
    // The voyage the landing is about: the one that departed last (the
    // plan's own rule, `gear_app::plan::current_voyage`), so a finished
    // voyage never hands the captain an older one.
    let milestone = store.read(|tx| gear_app::plan::current_voyage(tx, &project))?;
    if milestone.is_empty() {
        return Err(GearError::new(
            gear_model::ErrorCode::NoTarget,
            format!("the project {project} has no voyage"),
        ));
    }
    Ok(milestone)
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
    let out = run_report_time(&mut store, &env, &mut herdr, &filter, json, now_ts())?;
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
    now: i64,
) -> GearResult<String> {
    let fp = observe_identity(store, env, herdr)?;
    let overrides = caller_open_round_usage(store, env, now);
    let report = gear_app::time::build_with(store, env, fp.as_ref(), filter, now, &overrides)?;
    if json {
        return serde_json::to_string(&report).map_err(json_error);
    }
    Ok(gear_app::time::render(&report))
}

fn cmd_send(c: SendCmd, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let body = match c.direction.clone().or_else(|| c.body.clone()) {
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
        gear_model::MsgKind::Result if c.direction.is_none() => Some(gear_app::base::gate_for(&mut store, &env)?),
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
    confirmed: bool,
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
        confirmed,
    )?;
    println!("{out}");
    Ok(())
}

/// `gear plan check`: end the parks whose time has come (T-190).
///
/// A lane the captain parked until a time that has passed works again: its
/// agent continues (Active) or the engine onboards it again (Queued). A lane
/// parked on quota is left to the quota path.
///
/// It prints one line per lane that was let go.
///
/// The caller is the captain, proved as for `gear plan apply` (T-216, F2):
/// the caller's seat is resolved (its pane, or its `GEAR_CTX`) and its
/// identity is checked against the store inside the write transaction, so a
/// stale or unknown captain cannot end a park.
fn cmd_plan_check(json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let env = resolve_caller(&mut store, &mut herdr, env)?;
    let out = run_plan_check(&mut store, &env, &mut herdr)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({ "ended": out })).map_err(json_error)?
        );
    } else {
        for line in &out {
            println!("{line}");
        }
    }
    Ok(())
}

/// `gear plan check` body, over injected state.
///
/// The caller's identity is observed first (as `gear plan apply` does) and
/// checked inside the write transaction: only the captain, at the generation
/// the store holds, may end a park (T-216, F2). A caller that cannot be
/// proved is refused, and nothing is written.
///
/// The session's agents say whether a lane's agent is still in its pane.
/// Without them nothing is guessed: a park stays (fail-closed, T-190) and the
/// command says so.
fn run_plan_check(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
) -> GearResult<Vec<String>> {
    let fp = observe_identity(store, env, herdr)?;
    let agents = herdr.agents().ok();
    let ended = store.write(|tx| {
        let (ctx, _seat) = gear_app::identity::require_seat(
            tx,
            env,
            fp.as_ref(),
            Some(&[gear_model::SeatRole::Captain]),
            Some(&[gear_model::SeatState::Active]),
        )?;
        gear_app::plan::end_parked_lanes(tx, &ctx.project, agents.as_deref(), now_ts())
    })?;
    if ended.is_empty() && agents.is_none() {
        return Ok(vec![
            "the session's agents could not be read: no park was ended".to_string(),
        ]);
    }
    Ok(ended)
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
///
/// T-410: `--confirmed` marks the draft as a change to a departed voyage's
/// goal, bar or eta that the user asked for and confirmed.
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
    confirmed: bool,
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
            "give the voyage file, e.g. gear plan apply gear/voyages/M1/plan.md".to_string(),
        ));
    };
    let text = std::fs::read_to_string(path).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let mut draft = gear_app::draft::parse(&text)?;
    draft.confirmed = confirmed;
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
    // T-216 (F4): the agents are read for **every** apply, not only for a
    // release: an ordinary apply ends the parks whose time has come, and that
    // needs the same fact (is the lane's agent still in its pane?). A read
    // that fails refuses a release (the release cannot be carried out
    // unchecked) and leaves the automatic expiry fail-closed: a park whose
    // lane has a live binding stays, a park whose lane has none still ends.
    let agents = match herdr.agents() {
        Ok(agents) => Some(agents),
        Err(e) => {
            if releasing {
                return Err(GearError::new(
                    gear_model::ErrorCode::BadFormat,
                    format!(
                        "cannot read the session's agents, so `- release` cannot tell whether a lane's agent is still running: {} — nothing was applied",
                        e.0
                    ),
                ));
            }
            None
        }
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
    // stand idle (T-58, gear/voyages/M2.1/lessons.md 7a) and about the lanes his own tail
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
    // T-299: the captain's plan commit follows the project's own form. The
    // message is the project's `plan_message`, filled with the draft's project
    // and the id of its last milestone block; `{what}` is left for the captain
    // to say. A draft without a milestone block has no plan commit named.
    let plan_commit = match draft.milestones.last() {
        Some((header, _)) => {
            let (milestone, _) = gear_app::draft::milestone_parts(header);
            let conventions =
                gear_app::config::conventions_or_default(store, &draft.project)?;
            Some(gear_model::commit::fill(
                &conventions.plan_message,
                &[("project", draft.project.as_str()), ("milestone", milestone)],
            ))
        }
        None => None,
    };
    if json {
        let mut value = serde_json::to_value(&result).map_err(json_error)?;
        value["warnings"] = serde_json::json!(warnings);
        value["tailed"] = serde_json::json!(tailed);
        // The JSON carries the message itself (a script uses it as the commit
        // message); the text output wraps it in a sentence.
        if let Some(plan_commit) = &plan_commit {
            value["commit"] = serde_json::json!(plan_commit);
        }
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
    // The plan commit comes last: it is the captain's next step after an apply.
    if let Some(plan_commit) = &plan_commit {
        lines.push('\n');
        lines.push_str(&format!("commit the plan as: {plan_commit}"));
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

/// The commit a `--candidate` names (T-421): a value that is already a sha (7 to
/// 40 hex digits) is sent as it is; any other value is a ref, resolved with
/// `git rev-parse --verify <value>^{commit}` in the caller's working directory.
/// A value that names no commit here is refused, never sent: the store keeps the
/// commit reviewed, so the commit merged is the one that was read.
fn candidate_commit(value: &str) -> GearResult<String> {
    let sha = (7..=40).contains(&value.len()) && value.chars().all(|c| c.is_ascii_hexdigit());
    if sha {
        return Ok(value.to_string());
    }
    let resolved = std::process::Command::new("git")
        .args(["rev-parse", "--verify", &format!("{value}^{{commit}}")])
        .output();
    match resolved {
        Ok(out) if out.status.success() => {
            let full = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if full.is_empty() {
                return Err(candidate_not_a_commit(value));
            }
            Ok(full)
        }
        _ => Err(candidate_not_a_commit(value)),
    }
}

fn candidate_not_a_commit(value: &str) -> GearError {
    GearError::new(
        gear_model::ErrorCode::BadFormat,
        format!(
            "--candidate {value} is not a commit here: hand in the commit itself, e.g. \
             --candidate $(git rev-parse HEAD)"
        ),
    )
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
    // A hand-in names a commit, never a branch (T-421): a ref is resolved here,
    // in the caller's working directory, to the commit it points at now, so the
    // store never keeps a name whose commit can move.
    let candidate = c.candidate.as_deref().map(candidate_commit).transpose()?;
    let args = gear_app::mail::SendArgs {
        kind,
        body,
        task: c.task.clone(),
        attempt: c.attempt,
        contract: c.contract,
        // A result's candidate (T-45): the commit handed in.
        candidate,
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
    let result = if c.ack {
        gear_app::mail::send_with_ack(store, env, fp.as_ref(), &args, now)?
    } else if c.direction.is_some() {
        gear_app::mail::send_direction(store, env, fp.as_ref(), &args, now)?
    } else { gear_app::mail::send(store, env, fp.as_ref(), &args, now)? };
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
        if kind == gear_model::MsgKind::Result && c.direction.is_none() && !result.replayed {
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
        if kind == gear_model::MsgKind::Result && c.direction.is_none() && !result.replayed {
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
    // The branch is the project's own template (T-252); a project whose config
    // cannot be read keeps the name GEAR uses.
    let conventions =
        gear_app::config::conventions_or_default(store, &project).unwrap_or_default();
    let branch = gear_app::base::task_branch(&conventions, &project, &lane, task);
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
        | gear_model::MsgKind::Blocked
        | gear_model::MsgKind::Ack => Ok(kind),
        _ => Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!(
                "--kind {text} is not a `gear send` kind: progress, question, result, instruction, review, blocked or ack"
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
    install_stop_handler();
    let mut clock = now_ts;
    let mut out = Stamped { inner: std::io::stdout(), at_line_start: true };
    let line = engine_start_line(
        std::process::id(),
        std::path::Path::new(&db),
        env!("CARGO_PKG_VERSION"),
        env!("GEAR_BUILD_SHA"),
    );
    let _ = std::io::Write::write_fmt(&mut out, format_args!("{line}"));
    run_engine(&mut store, &mut herdr, &cfg, &mut clock, None, &mut out)
}

/// The engine's start line, with the build it runs, so the log says when a
/// replaced binary is being run by an old engine (T-398). `build` is the git
/// sha `build.rs` read at build time ("unknown" without git).
fn engine_start_line(pid: u32, db: &std::path::Path, version: &str, build: &str) -> String {
    format!("engine start pid={pid} db={} version={version} build={build}\n", db.display())
}

/// Set by the SIGTERM handler: `gear admin stop` asks the loop to leave
/// between ticks, never in the middle of one (T-265).
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The SIGTERM handler itself: async-signal-safe, it only stores the flag.
/// The loop then finishes the tick it is in — a merge gate may be running —
/// and leaves, so `gear admin stop` never cuts a merge in half.
extern "C" fn on_sigterm(_signal: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Let SIGTERM ask the engine loop to stop between ticks.
fn install_stop_handler() {
    // SAFETY: `on_sigterm` only stores into an atomic, which is
    // async-signal-safe; `libc::signal` only installs it.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            on_sigterm as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t,
        );
    }
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
    let Some(dir) = engine_dir() else { return };
    // A stop the user asked for stays stopped (T-394): no gear call starts a
    // held engine, and each call says why it is not running.
    if hold_path(&dir).exists() {
        eprintln!("{HELD_NOTICE}");
        return;
    }
    // Only the registered store gets an engine (L-54); others are silent.
    if engine_target_ok().is_err() {
        return;
    }
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

/// `engine.hold`: the stop the user asked for (T-394). While it exists no
/// `gear` call starts the engine; `gear admin start` removes it.
fn hold_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("engine.hold")
}

/// What every `gear` call says while the engine is held (T-394).
const HELD_NOTICE: &str = "gear: the engine is held (gear admin stop --hold); `gear admin start` resumes it";

/// What `gear admin stop --hold` says once the hold is written (T-394).
const HELD_WRITTEN: &str = "engine held: no gear call starts it until `gear admin start`";

/// Hold the engine: write `engine.hold` with the time and who held it.
fn write_hold(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(hold_path(dir), format!("{} held by gear admin stop --hold\n", utc(now_ts())))
}

/// The stop's last step with `--hold`: write the hold and say so (T-394).
fn hold_engine(dir: &std::path::Path) -> GearResult<()> {
    write_hold(dir).map_err(|e| {
        GearError::new(gear_model::ErrorCode::BadFormat, format!("cannot write {}: {e}", hold_path(dir).display()))
    })?;
    println!("{HELD_WRITTEN}");
    Ok(())
}

/// `gear admin stop`: signal the engine named in its lock file. With `hold`
/// it writes `engine.hold` as well, so the stop stays (T-394).
fn cmd_admin_stop(hold: bool) -> GearResult<()> {
    let env = env_from_process()?;
    if env.ctx.is_some() {
        return Err(GearError::new(gear_model::ErrorCode::Forbidden, "agents do not stop the engine"));
    }
    let dir = engine_dir().ok_or_else(|| GearError::new(gear_model::ErrorCode::NoTarget, "GEAR_DB is not set"))?;
    if let Ok(lock) = take_engine_lock(&dir) {
        drop(lock);
        println!("engine not running");
        if hold {
            hold_engine(&dir)?;
        }
        return Ok(());
    }
    // One engine and one store serve every project of a Herdr session, and each
    // project cannot restart at its own point while it does: a captain that
    // stops the engine says which projects that pauses (T-298). Read-only — the
    // stop writes nothing to the store — and a store that cannot be read prints
    // nothing more; the stop goes on either way.
    if let Some(db) = std::env::var_os(gear_model::ctx::ENV_DB) {
        if let Some(projects) = projects_in(std::path::Path::new(&db)) {
            println!("{}", stop_notice(&projects));
        }
    }
    let text = std::fs::read_to_string(dir.join("engine.lock")).unwrap_or_default();
    let pid: i32 = text.trim().parse().map_err(|_| {
        GearError::new(gear_model::ErrorCode::NoTarget, "engine is running but its pid is unknown")
    })?;
    // SAFETY: plain kill(2) on a pid read from our own lock file.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(GearError::new(gear_model::ErrorCode::NoTarget, format!("cannot stop pid {pid}")));
    }
    // The engine leaves its loop only between ticks (see `install_stop_handler`),
    // so wait for the lock it holds: a merge gate may still be running, and the
    // merge must not be left Executing (T-265).
    let mut held: Option<std::fs::File> = None;
    let mut announced = false;
    let mut try_lock = || match take_engine_lock(&dir) {
        Ok(lock) => {
            held = Some(lock);
            true
        }
        Err(_) => {
            if !announced {
                println!("waiting for the engine to finish its tick (a merge gate may be running)");
                announced = true;
            }
            false
        }
    };
    let mut sleep = || std::thread::sleep(std::time::Duration::from_secs(1));
    if wait_for_lock(&mut try_lock, &mut sleep, STOP_TRIES) {
        // Released here: the engine is gone, and this process must not keep
        // the next `gear` call from starting one.
        drop(held);
        println!("engine stopped (pid {pid})");
        if hold {
            hold_engine(&dir)?;
        }
        return Ok(());
    }
    Err(GearError::new(
        gear_model::ErrorCode::Forbidden,
        format!("the engine (pid {pid}) is still running after 20 min"),
    ))
}

/// `gear admin start`: lift the hold `gear admin stop --hold` wrote and start
/// the engine as `ensure_engine` does, printing its pid (T-394).
fn cmd_admin_start() -> GearResult<()> {
    let env = env_from_process()?;
    if env.ctx.is_some() {
        return Err(GearError::new(gear_model::ErrorCode::Forbidden, "agents do not start the engine"));
    }
    let dir = engine_dir().ok_or_else(|| GearError::new(gear_model::ErrorCode::NoTarget, "GEAR_DB is not set"))?;
    // The hold goes whichever way the start goes: the user asked for it gone.
    let hold = hold_path(&dir);
    if let Err(e) = std::fs::remove_file(&hold) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(GearError::new(gear_model::ErrorCode::BadFormat, format!("cannot remove {}: {e}", hold.display())));
        }
    }
    // Only the registered store gets an engine (L-54), as in `ensure_engine`.
    if engine_target_ok().is_err() {
        return Ok(());
    }
    match take_engine_lock(&dir) {
        Ok(lock) => {
            drop(lock); // free: release it for the engine we start
            match spawn_engine(&dir) {
                Ok(pid) => println!("engine started (pid {pid})"),
                Err(e) => {
                    return Err(GearError::new(gear_model::ErrorCode::NoTarget, format!("could not start the engine: {e}")))
                }
            }
        }
        Err(_) => println!("engine already running"),
    }
    Ok(())
}

/// What `gear admin stop` says before it signals the engine (T-298): the
/// projects this store holds, because they all pause until it starts again.
fn stop_notice(projects: &[String]) -> String {
    format!(
        "this engine runs {} project(s) of this store: {}; each pauses until the engine starts \
         again (the next gear call starts it)",
        projects.len(),
        projects.join(", ")
    )
}

/// Ids of the projects in the store at `db`, read-only; `None` when the store
/// cannot be read (T-298: the stop then says nothing more and goes on).
fn projects_in(db: &std::path::Path) -> Option<Vec<String>> {
    let mut store = gear_store::Store::open_read_only(db).ok()?;
    store.read(|tx| tx.project_ids()).ok()
}

/// How often `gear admin stop` looks at the engine lock: once a second for 20
/// minutes, the longest a merge gate may take before the user is told.
const STOP_TRIES: u32 = 20 * 60;

/// Try `try_lock` up to `tries` times, calling `sleep` between tries; true as
/// soon as it succeeds. Both are injected so a test never sleeps on the wall
/// clock.
fn wait_for_lock(try_lock: &mut dyn FnMut() -> bool, sleep: &mut dyn FnMut(), tries: u32) -> bool {
    for try_index in 0..tries {
        if try_lock() {
            return true;
        }
        // No sleep after the last try: the wait is over.
        if try_index + 1 < tries {
            sleep();
        }
    }
    false
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
    cfg.profiles = builtin_profiles();
    cfg
}

/// Every built-in agent profile, by name.
fn builtin_profiles() -> std::collections::BTreeMap<String, gear_herdr::Profile> {
    let mut profiles = std::collections::BTreeMap::new();
    for kind in ["opencode", "claude", "codex", "grok", "pi"] {
        if let Some(profile) = gear_herdr::builtin(kind) {
            profiles.insert(kind.to_string(), profile);
        }
    }
    profiles
}

/// The configuration the reports count their rounds with: the built-in agent
/// profiles (each kind's log) and nothing else.
fn report_cfg() -> gear_engine::EngineCfg {
    let mut cfg = gear_engine::EngineCfg::default();
    cfg.profiles = builtin_profiles();
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
    run_engine_with(store, herdr, cfg, clock, ticks, out, &|| STOP.load(std::sync::atomic::Ordering::SeqCst), &mut gear_engine::tick, &mut BinaryWatch::current())
}

/// The message the engine logs and puts in the captains' inbox when the binary
/// on disk is no longer the one it runs (T-398).
const BINARY_CHANGED: &str =
    "engine: the binary on disk changed since this engine started; restart it to run the new build";

/// The engine's own binary on disk: a rebuild replaces it and the running
/// engine keeps the old one, silently, for as long as it lives (T-398;
/// 2026-10-29: an hour on a replaced pre-fix binary).
struct BinaryWatch {
    path: std::path::PathBuf,
    /// The modification time read when the engine started.
    mtime_at_start: Option<std::time::SystemTime>,
    /// Set once the change is told: once per engine run.
    announced: bool,
}

impl BinaryWatch {
    /// Watch `path` as it is now.
    fn at_start(path: std::path::PathBuf) -> Self {
        Self { mtime_at_start: modified(&path), path, announced: false }
    }

    /// The engine's own binary, `std::env::current_exe()`.
    fn current() -> Self {
        Self::at_start(std::env::current_exe().unwrap_or_default())
    }

    /// Whether the file on disk is a different build than the one this engine
    /// started from — true once per engine run.
    fn changed_once(&mut self) -> bool {
        if self.announced {
            return false;
        }
        let Some(now) = modified(&self.path) else {
            return false; // the binary is gone or unreadable: no evidence
        };
        if Some(now) == self.mtime_at_start {
            return false;
        }
        self.announced = true;
        true
    }
}

/// The modification time of `path`, when it can be read.
fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// [`run_engine`] with the tick call injectable, so a test can drive the
/// "a failing tick does not stop the loop" path — and `stop`, which the loop
/// asks before each tick and once per second of the sleep between ticks — and
/// `watch`, which says when the binary on disk is a newer build (T-398).
fn run_engine_with(
    store: &mut gear_store::Store,
    herdr: &mut dyn gear_herdr::HerdrAct,
    cfg: &gear_engine::EngineCfg,
    clock: &mut dyn FnMut() -> i64,
    ticks: Option<u64>,
    out: &mut dyn std::io::Write,
    stop: &dyn Fn() -> bool,
    tick: &mut dyn FnMut(
        &mut gear_store::Store,
        &mut dyn gear_herdr::HerdrAct,
        &gear_engine::EngineCfg,
        i64,
    ) -> GearResult<gear_engine::TickReport>,
    watch: &mut BinaryWatch,
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
        // `gear admin stop` only asks the loop to leave between ticks, so the
        // tick that is running — and a merge gate in it — always finishes
        // (T-265).
        if stop() {
            break;
        }
        let now = clock();
        // T-398: a binary replaced on disk is told once, before the tick — a
        // failing announcement never stops the loop.
        if watch.changed_once() {
            if let Err(error) = announce_binary_changed(store, out, now) {
                writeln!(out, "binary watch failed: {}: {}", error.code, error.detail)
                    .map_err(write_error)?;
            }
        }
        match tick(store, herdr, cfg, now) {
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
        // The release lines the tick recorded (T-398): the engine log is
        // written here, where its writer is.
        for line in gear_engine::release::take_release_log() {
            writeln!(out, "{line}").map_err(write_error)?;
        }
        out.flush().map_err(write_error)?;
        done += 1;
        if ticks.is_none() {
            // One second at a time, so a stop that arrives during the sleep is
            // noticed within a second instead of after the whole tick.
            let seconds = u64::try_from(cfg.tick_s).unwrap_or(0);
            let mut slept = 0;
            while slept < seconds && !stop() {
                std::thread::sleep(std::time::Duration::from_secs(1));
                slept += 1;
            }
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

/// The binary on disk is not the one this engine runs: say it in the engine
/// log and put one item in every project's captain inbox (T-398). Once per
/// engine run — the caller's watch remembers.
fn announce_binary_changed(
    store: &mut gear_store::Store,
    out: &mut dyn std::io::Write,
    now: i64,
) -> GearResult<()> {
    writeln!(out, "{BINARY_CHANGED}").map_err(write_error)?;
    for project in store.read(|tx| tx.project_ids())? {
        store.write(|tx| {
            // A project without a captain has nobody to tell.
            let Some(captain) = tx.captain_of(&project)? else {
                return Ok(());
            };
            tx.insert_message(&gear_store::NewMessage {
                meta: None,
                project: project.clone(),
                thread: "engine:binary-changed".to_string(),
                task: None,
                attempt: None,
                sender: "engine".to_string(),
                sender_gen: 0,
                recipient: captain.id.clone(),
                kind: gear_model::MsgKind::System,
                urgent: false,
                body: BINARY_CHANGED.to_string(),
                reply_to: None,
                needs_ack: false,
                state: gear_model::MsgState::Open,
                created: now,
            })?;
            Ok(())
        })?;
    }
    Ok(())
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
    /// A fresh scratch directory under the temp directory, unique to this test
    /// and this name, removed when the test is done (T-412): a passing run
    /// leaves nothing behind. The same name in one test is the same directory.
    pub(crate) fn scratch(name: &str) -> std::path::PathBuf {
        use std::cell::{Cell, RefCell};
        use std::sync::atomic::{AtomicU64, Ordering};
        static THREADS: AtomicU64 = AtomicU64::new(0);

        /// The directories one test made, removed when that test is done.
        struct Made {
            /// The number of this thread, given at its first scratch directory.
            thread: Cell<Option<u64>>,
            /// The directories it made, by name.
            dirs: RefCell<Vec<(String, std::path::PathBuf)>>,
        }

        impl Drop for Made {
            fn drop(&mut self) {
                for (_, dir) in self.dirs.borrow().iter() {
                    std::fs::remove_dir_all(dir).ok();
                }
            }
        }

        thread_local! {
            static MADE: Made = const { Made { thread: Cell::new(None), dirs: RefCell::new(Vec::new()) } };
        }

        MADE.with(|made| {
            if let Some((_, dir)) = made.dirs.borrow().iter().find(|(named, _)| named == name) {
                return dir.clone();
            }
            let thread = made.thread.get().unwrap_or_else(|| {
                let thread = THREADS.fetch_add(1, Ordering::Relaxed);
                made.thread.set(Some(thread));
                thread
            });
            let dir = std::env::temp_dir().join(format!("{name}-{}-{thread}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            made.dirs.borrow_mut().push((name.to_string(), dir.clone()));
            dir
        })
    }
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
    use gear_store::{testing, BindingRow, ContractRow, SeatRow, Store, TaskRow};
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

    /// A stdout that refuses the first write: the board never draws (T-278).
    struct NoDraw;

    impl std::io::Write for NoDraw {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the board does not draw here"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// T-278: a board opened in a Herdr tab names that tab (`HERDR_TAB_ID`)
    /// before it draws, `--json` renames nothing, and a draw that fails still
    /// leaves the tab named — the rename comes first.
    #[test]
    fn the_board_names_its_herdr_tab_before_it_draws() {
        std::env::set_var("HERDR_TAB_ID", "w1:t2");
        let tab = herdr_tab_id();

        // The board of project `p` labels its tab `p board`.
        let (mut store, mut herdr) = world();
        let opts = gear_board::Options { project: Some("p".into()), ..Default::default() };
        let mut out = Vec::new();
        draw_board(&mut store, &opts, Some(&mut herdr), tab.clone(), &mut out).unwrap();
        assert!(!out.is_empty(), "the board drew nothing");
        assert_eq!(herdr.tab_renames_to("p board"), vec!["w1:t2"]);

        // A stdout that refuses the draw proves the rename came before it.
        let (mut store, mut herdr) = world();
        let drawn = draw_board(&mut store, &opts, Some(&mut herdr), tab.clone(), &mut NoDraw);
        assert!(drawn.is_err(), "the draw was expected to fail");
        assert_eq!(herdr.tab_renames_to("p board"), vec!["w1:t2"]);

        // A board that shows every project says so.
        let (mut store, mut herdr) = world();
        let mut out = Vec::new();
        draw_board(
            &mut store,
            &gear_board::Options::default(),
            Some(&mut herdr),
            tab.clone(),
            &mut out,
        )
        .unwrap();
        assert_eq!(herdr.tab_renames_to("GEAR board"), vec!["w1:t2"]);

        // `--json` is machine output: it renames nothing.
        let (mut store, mut herdr) = world();
        let json = gear_board::Options { project: Some("p".into()), json: true, ..Default::default() };
        let mut out = Vec::new();
        draw_board(&mut store, &json, Some(&mut herdr), tab, &mut out).unwrap();
        assert!(herdr.calls.is_empty(), "the JSON board renamed a tab: {:?}", herdr.calls);

        std::env::remove_var("HERDR_TAB_ID");
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
                    mechanical: None,
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
            direction: None,
            verdict: None,
            blocking: None,
            ack: false,
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
    fn ack_flag_sends_the_instruction_with_the_ack_line() {
        // `--ack` wires to `send_with_ack` (T-320): the lane gets the line to
        // acknowledge with, and the captain gets the item that waits.
        let (mut store, mut herdr) = world();
        let mut c = send_cmd("instruction");
        c.to = Some("heat".into());
        c.ack = true;
        let out = run_send(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &c,
            "check the queue".into(),
            None,
            T0 + 1,
            false,
        )
        .unwrap();
        assert!(out.contains("Delivered to p:heat"), "{out}");

        let sent = store.read(|tx| tx.get_message("p", "M-1")).unwrap().unwrap();
        assert!(sent.needs_ack);
        assert!(
            sent.body.ends_with(
                "Acknowledge it: gear send --kind ack --reply-to M-1 \"<what you will do>\""
            ),
            "{}",
            sent.body
        );
        let item = store.read(|tx| tx.get_message("p", "M-2")).unwrap().unwrap();
        assert_eq!(item.thread, "ack:M-1");
        assert_eq!(item.state, gear_model::MsgState::Waiting);
        assert_eq!(item.wait_until, Some(T0 + 1 + 600));
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
        let path = scratch("gear-draft").join("draft.md");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn plan_draft() -> String {
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n"
            .to_string()
    }

    /// A draft with a milestone block: the plan commit is named for it (T-299).
    fn plan_draft_with_milestone() -> String {
        "<!-- gear-plan project=p base=0 -->\n# Milestone M2: two\ndone: both ship\nbar: fast\neta: 1h\n\n# Queue\n## NEW [must] lane=heat milestone=M2\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n"
            .to_string()
    }

    /// Give the project `p` a `[conventions]` in its stored config.
    fn set_conventions(store: &mut Store, lines: &str) {
        store
            .write(|tx| {
                let mut project = tx.get_project("p")?.expect("the project");
                project.config_toml = format!(
                    "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/repo\"\n\n[quota]\nlanes = 1\ntemps = 0\n\n[captain]\npane = \"{CAP_PANE}\"\nkind = \"codex\"\nprofile = \"codex\"\n\n[conventions]\n{lines}\n"
                );
                tx.update_project(&project)
            })
            .unwrap();
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

    // T-410: `gear plan apply --confirmed` parses, and the flag reaches the
    // draft the apply reads: without it a departed voyage's eta change is
    // refused, with it the change applies.
    #[test]
    fn plan_apply_takes_the_confirmed_flag() {
        let cli =
            Cli::try_parse_from(["gear", "plan", "apply", "--confirmed", "plan.md"]).unwrap();
        match cli.cmd {
            Some(Cmd::Plan { op: PlanOp::Apply { path, confirmed, .. } }) => {
                assert!(confirmed);
                assert_eq!(path.as_deref(), Some(std::path::Path::new("plan.md")));
            }
            other => panic!("not a plan apply: {other:?}"),
        }
        let cli = Cli::try_parse_from(["gear", "plan", "apply", "plan.md"]).unwrap();
        match cli.cmd {
            Some(Cmd::Plan { op: PlanOp::Apply { confirmed, .. } }) => assert!(!confirmed),
            other => panic!("not a plan apply: {other:?}"),
        }

        // The flag is set on the draft before the apply: M2 departs with the
        // first block, so its eta moves only with the confirmation.
        let (mut store, mut herdr) = world();
        let path = write_draft(&plan_draft_with_milestone());
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
            false,
        )
        .unwrap();
        let changed = plan_draft_with_milestone().replace("eta: 1h", "eta: 2h").replace(
            "base=0",
            "base=1",
        );
        let path = write_draft(&changed);
        let err = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-changed",
            T0 + 2,
            false,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(err.detail.contains("milestone M2: eta changed after departure"), "{}", err.detail);
        run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "r-confirmed",
            T0 + 3,
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            store.read(|tx| tx.get_voyage("p", "M2")).unwrap().unwrap().eta.as_deref(),
            Some("2h")
        );
    }

    // T-299: after an apply the captain is told how to commit the plan, in the
    // project's own form: the `plan_message` template, filled with the project
    // and the id of the draft's last milestone block; `{what}` is his to say.
    #[test]
    fn plan_apply_names_the_plan_commit() {
        let (mut store, mut herdr) = world();
        let path = write_draft(&plan_draft_with_milestone());

        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "plan-commit",
            T0 + 1,
            false,
            false,
        )
        .unwrap();
        assert!(out.ends_with("commit the plan as: M2: {what}"), "{out}");

        // A project with its own form (ticket key) gets that one.
        let (mut store, mut herdr) = world();
        set_conventions(&mut store, "plan_message = \"PROJ-1 {milestone} {what}\"");
        let path = write_draft(&plan_draft_with_milestone());
        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "plan-commit",
            T0 + 1,
            false,
            false,
        )
        .unwrap();
        assert!(out.ends_with("commit the plan as: PROJ-1 M2 {what}"), "{out}");

        // The JSON carries the message itself, not the sentence.
        let (mut store, mut herdr) = world();
        let path = write_draft(&plan_draft_with_milestone());
        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "plan-commit",
            T0 + 1,
            true,
            false,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["commit"], serde_json::json!("M2: {what}"));

        // A staff-only plan has no milestone block: no commit line.
        let (mut store, mut herdr) = world();
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Staff\n- hold heat reason=test\n");
        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "plan-commit",
            T0 + 1,
            true,
            false,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(value.get("commit").is_none(), "{out}");
        let (mut store, mut herdr) = world();
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Staff\n- hold heat reason=test\n");
        let out = run_plan(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some(&path),
            None,
            None,
            "plan-commit-2",
            T0 + 2,
            false,
            false,
        )
        .unwrap();
        assert!(!out.contains("commit the plan as:"), "{out}");
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
                    mechanical: None,
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
            false,
        )
        .unwrap();

        assert!(out.contains("plan version 1"), "{out}");
        let seat = store.read(|tx| tx.get_seat("p:review-1")).unwrap().unwrap();
        assert_eq!(seat.state, gear_model::SeatState::Releasing);
    }

    /// T-216 (F2): the captain seat was replaced (the store holds generation
    /// 2); a caller that still carries generation 1 is stale and may not end
    /// a park.
    fn world_with_a_replaced_captain() -> (Store, FakeHerdr) {
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", T0)?;
                tx.insert_seat(&SeatRow {
                    id: "p:captain".into(),
                    project: "p".into(),
                    role: SeatRole::Captain,
                    name: "captain".into(),
                    profile: "codex".into(),
                    generation: 2,
                    state: gear_model::SeatState::Active,
                    state_since: T0,
                    state_reason: None,
                    task_id: None,
                })?;
                tx.insert_binding(&BindingRow {
                    seat: "p:captain".into(),
                    generation: 2,
                    session: "sess".into(),
                    pane_id: CAP_PANE.into(),
                    kind: "codex".into(),
                    session_ref: None,
                    status: gear_store::BindingStatus::Active,
                    handover_id: None,
                    created: T0,
                })?;
                testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, LANE_PANE, "opencode", T0)
            })
            .unwrap();
        let mut herdr = FakeHerdr::new();
        herdr.add_agent(CAP_PANE, "codex", AgentStatus::Idle);
        herdr.add_agent(LANE_PANE, "opencode", AgentStatus::Idle);
        (store, herdr)
    }

    /// A lane parked until a time that has passed, with its agent still in its
    /// pane.
    fn park_heat_until_the_past(store: &mut Store) {
        store
            .write(|tx| {
                tx.set_seat_state(
                    "p:heat",
                    gear_model::SeatState::Parked,
                    Some("until=2020-01-01T00:00:00Z"),
                    T0,
                )
            })
            .unwrap();
    }

    /// T-216 (F2): `gear plan check` proves its caller as `gear plan apply`
    /// does: a stale captain (an older generation) is refused and nothing is
    /// written.
    #[test]
    fn plan_check_refuses_a_stale_captain_and_writes_nothing() {
        let (mut store, mut herdr) = world_with_a_replaced_captain();
        park_heat_until_the_past(&mut store);
        let stale = gear_app::Env {
            ctx: Some(GearCtx { project: "p".into(), seat: "captain".into(), generation: 1 }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        };

        let error = run_plan_check(&mut store, &stale, &mut herdr).unwrap_err();

        assert_eq!(
            error.code,
            gear_model::ErrorCode::StaleGeneration,
            "the stale caller is refused: {error}"
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(
            seat.state,
            gear_model::SeatState::Parked,
            "nothing was written: the park stays"
        );
    }

    /// T-216 (F2): a captain found from its own pane, without `GEAR_CTX`, may
    /// end a park: the seat is resolved from the pane and proved.
    #[test]
    fn plan_check_from_the_captains_pane_without_geom_ctx_ends_an_expired_park() {
        let (mut store, mut herdr) = world();
        park_heat_until_the_past(&mut store);
        // The caller runs in the captain's pane; there is no GEAR_CTX.
        let anonymous = gear_app::Env {
            ctx: None,
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        };
        let env = resolve_caller(&mut store, &mut herdr, anonymous).unwrap();
        assert!(env.ctx.is_some(), "the captain is found in its pane");

        let out = run_plan_check(&mut store, &env, &mut herdr).unwrap();

        assert!(
            out.iter().any(|line| line.contains("heat")),
            "the park ended: {out:?}"
        );
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(seat.state, gear_model::SeatState::Active, "its agent continues");
        assert_eq!(seat.state_reason.as_deref(), Some("park ended"));
    }

    /// T-216 (F4): an ordinary apply ends an expired park whose lane's agent
    /// is still running — the apply reads the session's agents, not only for a
    /// release.
    #[test]
    fn an_ordinary_apply_ends_an_expired_park_with_a_live_agent() {
        let (mut store, mut herdr) = world();
        park_heat_until_the_past(&mut store);
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, "w1:p3", "opencode", T0)
            })
            .unwrap();
        // An ordinary apply: the captain holds another lane.
        let path = write_draft("<!-- gear-plan project=p base=0 -->\n# Staff\n- hold wire reason=wait\n");

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
            false,
        )
        .unwrap();

        assert!(out.contains("plan version 1"), "{out}");
        let seat = store.read(|tx| tx.get_seat("p:heat")).unwrap().unwrap();
        assert_eq!(
            seat.state,
            gear_model::SeatState::Active,
            "the expired park ended: its agent continues"
        );
        assert_eq!(seat.state_reason.as_deref(), Some("park ended"));
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
        let out = run_plan(&mut store, &env("captain", CAP_PANE), &mut herdr, Some(&path), None, None, "rebind", T0+1, false, false,
        ).unwrap();
        assert!(out.contains("rebind heat: repo root → api, owns [src/] → [src/, tests/]: waits for T-1"), "{out}");
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
            false,
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

    /// T-163: `gear queue` is the captain's plain view of the work (design
    /// §8.3): every lane, what it does now, why an idle lane is idle, and
    /// what it has left — read from the store.
    #[test]
    fn queue_prints_the_lanes_and_the_reviewers() {
        let (mut store, _herdr) = world();
        store
            .write(|tx| {
                // Two lanes beside the ones the world seeds.
                testing::seed_active_seat(tx, "p", "hull", SeatRole::Lane, "w1:p5", "opencode", T0)?;
                testing::seed_active_seat(tx, "p", "keel", SeatRole::Lane, "w1:p6", "opencode", T0)?;
                // The hull lane works on T-1 and has two tasks left.
                let mut working = lane_task("T-1", "hull", gear_model::TaskState::Running, &[]);
                working.estimate_min = Some(60);
                working.rank = 1;
                tx.insert_task(&working)?;
                let mut queued = lane_task("T-2", "hull", gear_model::TaskState::Ready, &[]);
                queued.estimate_min = Some(35);
                queued.rank = 2;
                tx.insert_task(&queued)?;
                let mut waiting = lane_task("T-3", "hull", gear_model::TaskState::Ready, &["T-9"]);
                waiting.estimate_min = Some(30);
                waiting.rank = 3;
                tx.insert_task(&waiting)?;
                let mut queued = lane_task("T-4", "hull", gear_model::TaskState::Ready, &[]);
                queued.estimate_min = Some(60);
                queued.rank = 4;
                tx.insert_task(&queued)?;
                // The keel lane waits for a task of another lane: its next
                // task is queued, but the dependency is not done.
                let mut waiting = lane_task("T-9", "keel", gear_model::TaskState::Ready, &["T-1"]);
                waiting.estimate_min = Some(40);
                waiting.rank = 9;
                tx.insert_task(&waiting)?;
                // A reviewer works on T-2; another one stands by.
                testing::seed_temp_seat(tx, "p", "r-1", "T-2", TEMP_PANE, "opencode", T0)?;
                Ok(())
            })
            .unwrap();
        // The hull lane's attempt: 38 minutes of work.
        store
            .write(|tx| {
                // The task is ready when the attempt starts, then runs.
                let mut task = tx.get_task("p", "T-1")?.unwrap();
                task.state = gear_model::TaskState::Ready;
                tx.update_task(&task)?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:hull", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                // Only the start is recorded: the round is still running, so
                // the elapsed minutes run to the clock (T-163 rework F1).
                tx.set_attempt_time("p", "T-1", 1, gear_store::AttemptStep::Started, T0)?;
                Ok(())
            })
            .unwrap();

        let out = run_queue(&mut store, &env("captain", CAP_PANE), false, T0 + 38 * 60).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // The working lane: the task, its actual and estimated minutes, and
        // what it has left.
        let hull = lines.iter().find(|line| line.starts_with("hull")).unwrap();
        assert!(hull.contains("working T-1 38/60m"), "{hull}");
        assert!(hull.contains("· left 3 (125m):"), "{hull}");
        assert!(hull.contains("T-2 35m"), "{hull}");
        assert!(hull.contains("T-3 30m needs T-9"), "{hull}");
        assert!(
            hull.contains("T-2 35m, T-3 30m needs T-9, T-4 60m"),
            "the queue is in the order of the plan: {hull}"
        );
        assert!(hull.contains("T-4 60m"), "{hull}");
        // The idle lane: why it is idle, through the board's own function.
        let keel = lines.iter().find(|line| line.starts_with("keel")).unwrap();
        assert!(keel.contains("idle: next T-9 waits for T-1 (hull, in progress)"), "{keel}");
        assert!(keel.contains("· left 1 (40m)"), "{keel}");
        // The reviewers are on the last line.
        let last = lines.last().unwrap();
        assert!(last.starts_with("r-1 reviewing T-2"), "{last}");

        // --json has the same facts.
        let json = run_queue(&mut store, &env("captain", CAP_PANE), true, T0 + 38 * 60).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let lanes = parsed["lanes"].as_array().unwrap();
        assert_eq!(lanes.len(), 3, "{json}");
        // The lane with nothing planned says so.
        let heat = lanes.iter().find(|lane| lane["lane"] == "heat").unwrap();
        assert_eq!(heat["idle"], "nothing left planned");
        assert_eq!(heat["left"].as_array().unwrap().len(), 0);
        let hull = lanes.iter().find(|lane| lane["lane"] == "hull").unwrap();
        assert_eq!(hull["working"]["id"], "T-1");
        assert_eq!(hull["working"]["actual_min"], 38, "the elapsed minutes of a running task");
        assert_eq!(hull["left"].as_array().unwrap().len(), 3);
        assert_eq!(hull["left_min"], 125);
        assert_eq!(parsed["reviewers"][0]["reviewing"], "T-2");
    }

    /// T-163: `gear plan show T-n` prints one task's block in plan-file
    /// form, and the block parses back to the same task.
    /// T-163 rework F2: a lane that is idle for a reason (held, quota
    /// parked, waiting for a task) is not working, and its unfinished task
    /// stays in its left list. The text and the JSON say the same.
    #[test]
    fn queue_shows_a_held_lane_as_idle_with_its_reason() {
        let (mut store, _herdr) = world();
        store
            .write(|tx| {
                let mut working = lane_task("T-1", "heat", gear_model::TaskState::Ready, &[]);
                working.estimate_min = Some(60);
                tx.insert_task(&working)?;
                gear_app::tasks::start_attempt(tx, "p", "T-1", "p:heat", 1, T0)?;
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-1", 1, T0)?;
                tx.set_attempt_time("p", "T-1", 1, gear_store::AttemptStep::Started, T0)?;
                // The captain holds the lane: it is idle for a reason.
                tx.set_seat_state(
                    "p:heat",
                    gear_model::SeatState::Held,
                    Some("hold: the captain paused it"),
                    T0 + 60,
                )?;
                Ok(())
            })
            .unwrap();

        let out = run_queue(&mut store, &env("captain", CAP_PANE), false, T0 + 38 * 60).unwrap();
        let heat = out.lines().find(|line| line.starts_with("heat")).unwrap();
        assert!(heat.contains("idle: held: the captain paused it"), "{heat}");
        assert!(!heat.contains("working"), "{heat}");
        assert!(heat.contains("left 1 (60m)"), "the unfinished task is left: {heat}");

        // The JSON says the same.
        let json = run_queue(&mut store, &env("captain", CAP_PANE), true, T0 + 38 * 60).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let heat = parsed["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|lane| lane["lane"] == "heat")
            .unwrap();
        assert!(heat["working"].is_null(), "{heat}");
        assert_eq!(heat["idle"], "held: the captain paused it");
        assert_eq!(heat["left"].as_array().unwrap().len(), 1);
    }

    /// T-163 rework F3: a released or retired reviewer is not in the queue;
    /// a standby one says how long it has been standing by.
    #[test]
    fn queue_shows_only_the_available_reviewers() {
        let (mut store, _herdr) = world();
        store
            .write(|tx| {
                // A reviewer with a job.
                testing::seed_temp_seat(tx, "p", "r-1", "T-1", TEMP_PANE, "opencode", T0)?;
                // A standby reviewer.
                testing::seed_temp_seat(tx, "p", "r-2", "T-2", "w1:p4", "opencode", T0)?;
                tx.set_seat_state("p:r-2", gear_model::SeatState::Standby, None, T0)?;
                // A released reviewer: its agent and its pane are gone (§3.6).
                testing::seed_temp_seat(tx, "p", "r-3", "T-3", "w1:p5", "opencode", T0)?;
                tx.set_seat_state("p:r-3", gear_model::SeatState::Released, None, T0)?;
                // A retired one.
                testing::seed_temp_seat(tx, "p", "r-4", "T-4", "w1:p6", "opencode", T0)?;
                tx.set_seat_state("p:r-4", gear_model::SeatState::Retired, None, T0)?;
                Ok(())
            })
            .unwrap();

        let out = run_queue(&mut store, &env("captain", CAP_PANE), false, T0 + 180).unwrap();
        let last = out.lines().last().unwrap();
        assert!(last.contains("r-1 reviewing T-1"), "{last}");
        assert!(last.contains("r-2 standby 3m"), "{last}");
        assert!(!last.contains("r-3"), "a released reviewer is not available: {last}");
        assert!(!last.contains("r-4"), "a retired reviewer is not available: {last}");

        // The JSON says the same.
        let json = run_queue(&mut store, &env("captain", CAP_PANE), true, T0 + 180).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let names: Vec<&str> = parsed["reviewers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|reviewer| reviewer["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["r-1", "r-2"]);
    }

    #[test]
    fn plan_show_prints_a_block_that_parses_back() {
        let (mut store, _herdr) = world();
        store
            .write(|tx| {
                let mut task = lane_task("T-7", "heat", gear_model::TaskState::Ready, &["T-8"]);
                task.priority = "must".into();
                task.milestone = Some("M2".into());
                task.required = true;
                task.estimate_min = Some(60);
                task.followup_of = Some("T-6".into());
                tx.insert_task(&task)?;
                tx.insert_contract(&gear_store::ContractRow {
                    project: "p".into(),
                    task: "T-7".into(),
                    version: 1,
                    goal: "the goal of T-7".into(),
                    accept: "the acceptance of T-7".into(),
                    evidence_files: vec!["ops/reports/T-7.md".into()],
                    evidence_due_min: Some(90),
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })?;
                Ok(())
            })
            .unwrap();

        let out = store
            .read(|tx| gear_app::plan::show(tx, "p", "T-7"))
            .unwrap();
        assert!(out.starts_with("## T-7 [must] lane=heat milestone=M2 required"), "{out}");
        assert!(out.contains("goal: the goal of T-7"), "{out}");
        assert!(out.contains("accept: the acceptance of T-7"), "{out}");
        assert!(out.contains("evidence-files: ops/reports/T-7.md"), "{out}");
        assert!(out.contains("evidence-due: 90"), "{out}");
        assert!(out.contains("estimate: 60"), "{out}");
        assert!(out.contains("deps: T-8"), "{out}");
        assert!(out.contains("followup-of: T-6"), "{out}");

        // The block parses back to the same task.
        let text = format!("<!-- gear-plan project=p base=0 -->\n# Queue\n{out}\n");
        let draft = gear_app::draft::parse(&text).unwrap();
        let item = &draft.queue[0];
        assert_eq!(item.id.as_deref(), Some("T-7"));
        assert_eq!(item.lane.as_deref(), Some("heat"));
        assert_eq!(item.milestone.as_deref(), Some("M2"));
        assert!(item.required);
        assert_eq!(item.goal.as_deref(), Some("the goal of T-7"));
        assert_eq!(item.accept.as_deref(), Some("the acceptance of T-7"));
        assert_eq!(item.estimate, Some(60));
        assert_eq!(item.deps, vec!["T-8".to_string()]);
        assert_eq!(item.followup_of.as_deref(), Some("T-6"));
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
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, gear_model::ErrorCode::BadFormat);
        assert!(
            err.detail.contains("gear/voyages/M1/plan.md"),
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
            T0,
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
            T0,
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

    /// T-171 rework F1: the landing's default command names no voyage: the
    /// report must show the voyage that departed last, so a finished voyage
    /// never hands the captain an older one.
    #[test]
    fn report_voyage_without_a_name_shows_the_last_departed_voyage() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                for (milestone, departure) in [("M1", T0 - 10 * 86400), ("M4", T0)] {
                    tx.upsert_voyage(&gear_store::VoyageRow {
                        project: "p".into(),
                        milestone: milestone.into(),
                        title: format!("the {milestone} station"),
                        done: Some(format!("(1) the {milestone} clause")),
                        bar: Some("STABLE".into()),
                        eta: Some("12h".into()),
                        departure: Some(departure),
                        departure_tasks: Some(1),
                        departure_estimate_min: Some(30),
                        updated: T0,
                    })
                    .unwrap();
                }
                Ok(())
            })
            .unwrap();

        let out = run_report_voyage(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            None,
            false,
            false,
            T0 + 60,
        )
        .unwrap();
        assert!(out.contains("Done (M4)"), "the voyage that departed last: {out}");
        assert!(!out.contains("Done (M1)"), "{out}");
    }

    /// T-171 rework F6: the text report prints each round's own tokens and
    /// cost, not only the task's total.
    #[test]
    fn report_time_prints_a_round_own_tokens() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                tx.insert_task(&lane_task("T-7", "heat", gear_model::TaskState::Ready, &[]))
                    .unwrap();
                tx.insert_contract(&gear_store::ContractRow {
                    project: "p".into(),
                    task: "T-7".into(),
                    version: 1,
                    goal: "g".into(),
                    accept: "a".into(),
                    evidence_files: vec![],
                    evidence_due_min: None,
                    body: String::new(),
                    created: T0,
                    mechanical: None,
                })
                .unwrap();
                gear_app::tasks::start_attempt(tx, "p", "T-7", "p:heat", 1, T0).unwrap();
                gear_app::tasks::on_delivery_confirmed(tx, "p", "T-7", 1, T0).unwrap();
                for (role, seat, input, cost) in [
                    ("dev", "p:heat", 100i64, 0.10f64),
                    ("review", "p:r-1", 200, 0.20),
                ] {
                    let key = gear_store::RoundKey {
                        project: "p".into(),
                        task: "T-7".into(),
                        attempt: 1,
                        role: role.into(),
                    };
                    tx.insert_round(&gear_store::NewRound {
                        key: key.clone(),
                        seat: seat.into(),
                        generation: 1,
                        cli: "opencode".into(),
                        model: None,
                        start_at: T0,
                    })
                    .unwrap();
                    tx.set_round_tokens(
                        &key,
                        &gear_store::RoundTokens {
                            input: Some(input),
                            output: Some(10),
                            cache_read: Some(0),
                            cache_write: Some(0),
                            reasoning: Some(0),
                            cost_usd: Some(cost),
                            cost_source: "prices".into(),
                        },
                        T0 + 60,
                    )
                    .unwrap();
                }
                Ok(())
            })
            .unwrap();

        let out = run_report_time(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &gear_app::time::Filter::default(),
            false,
            T0,
        )
        .unwrap();
        assert!(out.contains("T-7 attempt 1 dev · tokens 100 in / 10 out"), "{out}");
        assert!(out.contains("T-7 attempt 1 review · tokens 200 in / 10 out"), "{out}");
    }

    /// A task of a voyage: a lane works on it and the plan knows its time.
    fn voyage_task(id: &str, lane: &str, milestone: &str) -> TaskRow {
        let mut task = lane_task(id, lane, gear_model::TaskState::Ready, &[]);
        task.milestone = Some(milestone.to_string());
        task.estimate_min = Some(60);
        task
    }

    /// A pi session log with one assistant record: the usage a report counts
    /// (T-207).
    fn pi_log(name: &str, at: i64) -> std::path::PathBuf {
        let path = scratch("gear-t207").join(format!("{name}.jsonl"));
        let record = serde_json::json!({
            "type": "message",
            "timestamp": at,
            "message": {
                "role": "assistant",
                "usage": {
                    "input": 1200,
                    "output": 300,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "reasoning": 0,
                    "cost": {"total": 0.42}
                }
            }
        });
        std::fs::write(&path, format!("{record}\n")).unwrap();
        path
    }

    /// The contract of a task the lane starts.
    fn contract_of(task: &str) -> gear_store::ContractRow {
        gear_store::ContractRow {
            project: "p".into(),
            task: task.into(),
            version: 1,
            goal: "g".into(),
            accept: "a".into(),
            evidence_files: vec![],
            evidence_due_min: None,
            body: String::new(),
            created: T0,
            mechanical: None,
        }
    }

    /// T-207: the reports count the open rounds — the round that is still
    /// running — and never show an unknown usage as zero.
    ///
    /// The store counts a round when it ends, so an open round has no numbers
    /// yet: the report counts it as it reads (design §4.3) and shows what it
    /// counted, in the round's own line and in the task's and the lane's sums.
    #[test]
    fn the_reports_count_the_open_round() {
        let (mut store, mut herdr) = world();
        let log = pi_log("open", T0 + 10);
        store
            .write(|tx| {
                tx.insert_task(&voyage_task("T-7", "heat", "M2")).unwrap();
                tx.insert_contract(&contract_of("T-7")).unwrap();
                gear_app::tasks::start_attempt(tx, "p", "T-7", "p:heat", 1, T0).unwrap();
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
                    cli: "pi".into(),
                    model: None,
                    start_at: T0,
                })
                .unwrap();
                // The session the lane runs in: pi's log, which the report reads.
                tx.add_round_session(&key, log.to_str().unwrap()).unwrap();
                tx.upsert_voyage(&gear_store::VoyageRow {
                    project: "p".into(),
                    milestone: "M2".into(),
                    title: "the second station".into(),
                    done: Some("(1) the voyage sails".into()),
                    bar: Some("STABLE".into()),
                    eta: Some("12h".into()),
                    departure: Some(T0),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(60),
                    updated: T0,
                })
                .unwrap();
                Ok(())
            })
            .unwrap();

        let out = run_report_time(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &gear_app::time::Filter::default(),
            false,
            T0 + 60,
        )
        .unwrap();
        // The round's own count: the open round is counted as the report reads.
        assert!(out.contains("T-7 attempt 1 dev · tokens 1200 in / 300 out"), "{out}");
        assert!(out.contains("cost $0.4200"), "{out}");
        // The task's sum and the voyage's sum carry the same numbers.
        assert!(out.contains("T-7 · estimate 60m"), "{out}");
        assert!(out.contains("Voyage M2 · 1 task"), "{out}");

        let voyage = run_report_voyage(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some("M2"),
            false,
            false,
            T0 + 60,
        )
        .unwrap();
        assert!(voyage.contains("Time and tokens · tokens 1200 in / 300 out"), "{voyage}");
        assert!(voyage.contains("lane heat · tokens 1200 in / 300 out"), "{voyage}");

        // The store is not written: the open round keeps its empty columns.
        let stored = store.read(|tx| tx.rounds_of_project("p")).unwrap();
        let open = stored.iter().find(|row| row.task == "T-7").unwrap();
        assert_eq!(open.input, None, "the report writes no tokens");
        assert_eq!(open.counted_at, None, "the report counts nothing into the store");
        std::fs::remove_file(&log).ok();
    }

    /// T-207: an open round whose log cannot be read stays unknown: the
    /// report shows a question mark, never a zero.
    #[test]
    fn an_unreadable_open_round_is_unknown_not_zero() {
        let (mut store, mut herdr) = world();
        store
            .write(|tx| {
                tx.insert_task(&voyage_task("T-8", "heat", "M2")).unwrap();
                tx.insert_contract(&contract_of("T-8")).unwrap();
                gear_app::tasks::start_attempt(tx, "p", "T-8", "p:heat", 1, T0).unwrap();
                let key = gear_store::RoundKey {
                    project: "p".into(),
                    task: "T-8".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: None,
                    start_at: T0,
                })
                .unwrap();
                // The session file is gone: the log cannot be read.
                tx.add_round_session(&key, "/nonexistent/gear-t207-gone.jsonl")
                    .unwrap();
                tx.upsert_voyage(&gear_store::VoyageRow {
                    project: "p".into(),
                    milestone: "M2".into(),
                    title: "the second station".into(),
                    done: Some("(1) the voyage sails".into()),
                    bar: Some("STABLE".into()),
                    eta: Some("12h".into()),
                    departure: Some(T0),
                    departure_tasks: Some(1),
                    departure_estimate_min: Some(60),
                    updated: T0,
                })
                .unwrap();
                Ok(())
            })
            .unwrap();

        let out = run_report_time(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &gear_app::time::Filter::default(),
            false,
            T0 + 60,
        )
        .unwrap();
        assert!(out.contains("T-8 attempt 1 dev · tokens ? / cost ?"), "{out}");
        assert!(!out.contains("tokens 0 in / 0 out"), "{out}");

        let voyage = run_report_voyage(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            Some("M2"),
            false,
            false,
            T0 + 60,
        )
        .unwrap();
        assert!(voyage.contains("Time and tokens · tokens ? / cost ?"), "{voyage}");
        assert!(voyage.contains("lane heat · tokens ? / cost ?"), "{voyage}");
        assert!(!voyage.contains("tokens 0 in / 0 out"), "{voyage}");
    }

    /// T-207: a closed round keeps the numbers the store counted: the report
    /// does not count it again.
    #[test]
    fn a_closed_round_keeps_its_counted_numbers() {
        let (mut store, mut herdr) = world();
        let log = pi_log("closed", T0 + 10);
        store
            .write(|tx| {
                tx.insert_task(&voyage_task("T-9", "heat", "M2")).unwrap();
                tx.insert_contract(&contract_of("T-9")).unwrap();
                gear_app::tasks::start_attempt(tx, "p", "T-9", "p:heat", 1, T0).unwrap();
                let key = gear_store::RoundKey {
                    project: "p".into(),
                    task: "T-9".into(),
                    attempt: 1,
                    role: "dev".into(),
                };
                tx.insert_round(&gear_store::NewRound {
                    key: key.clone(),
                    seat: "p:heat".into(),
                    generation: 1,
                    cli: "pi".into(),
                    model: None,
                    start_at: T0,
                })
                .unwrap();
                tx.add_round_session(&key, log.to_str().unwrap()).unwrap();
                // The round ended: the engine counted it and stored the count.
                tx.close_round(&key, T0 + 30).unwrap();
                tx.set_round_tokens(
                    &key,
                    &gear_store::RoundTokens {
                        input: Some(7),
                        output: Some(8),
                        cache_read: Some(0),
                        cache_write: Some(0),
                        reasoning: Some(0),
                        cost_usd: Some(0.01),
                        cost_source: "prices".into(),
                    },
                    T0 + 30,
                )
                .unwrap();
                Ok(())
            })
            .unwrap();

        let out = run_report_time(
            &mut store,
            &env("captain", CAP_PANE),
            &mut herdr,
            &gear_app::time::Filter::default(),
            false,
            T0 + 60,
        )
        .unwrap();
        // The stored count, not a fresh one: the round is over.
        assert!(out.contains("T-9 attempt 1 dev · tokens 7 in / 8 out"), "{out}");
        assert!(out.contains("cost $0.0100"), "{out}");
        let stored = store.read(|tx| tx.rounds_of_task("p", "T-9")).unwrap();
        assert_eq!(stored[0].input, Some(7), "the stored count is untouched");
        assert_eq!(stored[0].counted_at, Some(T0 + 30));
        std::fs::remove_file(&log).ok();
    }

    /// A world with T-7 started and its delivery view recorded, so a result can
    /// be handed in.
    fn world_with_attempt() -> (Store, FakeHerdr) {
        let (mut store, herdr) = world();
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
        (store, herdr)
    }

    #[test]
    fn send_candidate_reaches_mail() {
        let (mut store, mut herdr) = world_with_attempt();

        let mut c = send_cmd("result");
        c.task = Some("T-7".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("abc1234".into());
        let out = run_send(
            &mut store,
            &env("heat", LANE_PANE),
            &mut herdr,
            &c,
            "done, see abc1234".into(),
            None,
            T0 + 2,
            false,
        )
        .unwrap();

        assert!(out.contains("Delivered to p:captain"), "{out}");
        let attempt = store.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap();
        assert_eq!(attempt.state, gear_model::AttemptState::Submitted);
    }

    /// A `git` in `dir`: the fixture builds its own commit (T-421).
    fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Run `f` with the process working directory at `dir`: the candidate is
    /// resolved in the caller's working directory (T-421). The lock keeps two
    /// such tests from fighting over one process-wide setting.
    fn in_dir<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        let out = f();
        std::env::set_current_dir(before).unwrap();
        out
    }

    /// T-421: a hand-in names a commit, never a branch. A branch is resolved in
    /// the caller's working directory: the store keeps the commit it points at.
    #[test]
    fn candidate_branch_is_sent_as_its_commit() {
        let dir = scratch("send-candidate-branch");
        git_in(&dir, &["init", "-q", "-b", "dev"]);
        std::fs::write(dir.join("work"), "x").unwrap();
        git_in(&dir, &["add", "work"]);
        git_in(
            &dir,
            &[
                "-c",
                "user.name=gear",
                "-c",
                "user.email=gear@example.test",
                "commit",
                "-q",
                "-m",
                "one",
            ],
        );
        let head = git_in(&dir, &["rev-parse", "HEAD"]);
        git_in(&dir, &["branch", "heat/t421"]);

        let (mut store, mut herdr) = world_with_attempt();
        let mut c = send_cmd("result");
        c.task = Some("T-7".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("heat/t421".into());
        in_dir(&dir, || {
            run_send(
                &mut store,
                &env("heat", LANE_PANE),
                &mut herdr,
                &c,
                "handed in".into(),
                None,
                T0 + 2,
                false,
            )
        })
        .unwrap();

        let attempt = store.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap();
        assert_eq!(attempt.state, gear_model::AttemptState::Submitted);
        assert_eq!(attempt.candidate.as_deref(), Some(head.as_str()), "the branch's sha, not its name");
    }

    /// T-421: a name that is no commit in the caller's working directory is
    /// refused, and nothing is stored.
    #[test]
    fn candidate_that_is_no_commit_is_refused_and_stores_nothing() {
        // A scratch directory: not a repository, so no ref resolves there.
        let dir = scratch("send-candidate-unknown");
        let (mut store, mut herdr) = world_with_attempt();
        let mut c = send_cmd("result");
        c.task = Some("T-7".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some("no-such-thing".into());
        let error = in_dir(&dir, || {
            run_send(
                &mut store,
                &env("heat", LANE_PANE),
                &mut herdr,
                &c,
                "handed in".into(),
                None,
                T0 + 2,
                false,
            )
        })
        .unwrap_err();

        assert_eq!(error.code, gear_model::ErrorCode::BadFormat);
        assert_eq!(
            error.detail,
            "--candidate no-such-thing is not a commit here: hand in the commit itself, e.g. \
             --candidate $(git rev-parse HEAD)"
        );
        // Nothing was stored: the attempt still waits for its hand-in, and the
        // captain got no message.
        let attempt = store.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap();
        assert_eq!(attempt.state, gear_model::AttemptState::Started);
        assert!(attempt.candidate.is_none());
        assert!(store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 3)).unwrap().is_empty());
    }

    /// T-421: a value that is already a sha is sent as it is — no git needed.
    #[test]
    fn candidate_sha_is_sent_unchanged() {
        // Not a repository: a sha needs no resolution.
        let dir = scratch("send-candidate-sha");
        let (mut store, mut herdr) = world_with_attempt();
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let mut c = send_cmd("result");
        c.task = Some("T-7".into());
        c.attempt = Some(1);
        c.contract = Some(1);
        c.candidate = Some(sha.into());
        in_dir(&dir, || {
            run_send(
                &mut store,
                &env("heat", LANE_PANE),
                &mut herdr,
                &c,
                "handed in".into(),
                None,
                T0 + 2,
                false,
            )
        })
        .unwrap();

        let attempt = store.read(|tx| tx.get_attempt("p", "T-7", 1)).unwrap().unwrap();
        assert_eq!(attempt.state, gear_model::AttemptState::Submitted);
        assert_eq!(attempt.candidate.as_deref(), Some(sha));
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
    fn direction_result_is_bound_to_the_reviewer_and_does_not_submit() {
        let cli=Cli::try_parse_from(["gear","send","--kind","result","--task","T-7","--attempt","1","--direction","ok"]).unwrap();
        assert!(matches!(cli.cmd,Some(Cmd::Send(ref c)) if c.direction.as_deref()==Some("ok")));
        let (mut store, mut herdr) = review_world();
        store.write(|tx| tx.set_meta("engine.direction.p.T-7.1", &serde_json::json!({
            "seat":"p:r-1", "generation":1, "state":"running", "sha":"abc123"
        }).to_string())).unwrap();
        let task_before=store.read(|tx|tx.get_task("p","T-7")).unwrap().unwrap().state;
        let mut c=send_cmd("result");
        c.task=Some("T-7".into()); c.attempt=Some(1); c.direction=Some("ok".into());
        let out=run_send(&mut store,&env("r-1",TEMP_PANE),&mut herdr,&c,"ok".into(),None,T0+1,false).unwrap();
        assert!(!out.contains("Handed in"), "{out}");
        let history=store.read(|tx|tx.thread_messages("p","T-7:1")).unwrap();
        assert_eq!(history.len(),1);
        assert_eq!(history[0].kind, gear_model::MsgKind::Result);
        assert_eq!(history[0].body,"ok");
        let meta:serde_json::Value=serde_json::from_str(history[0].meta.as_deref().unwrap()).unwrap();
        assert_eq!(meta,serde_json::json!({"direction":true,"sha":"abc123"}));
        assert_eq!(store.read(|tx|tx.get_attempt("p","T-7",1)).unwrap().unwrap().state,gear_model::AttemptState::Started);
        assert_eq!(store.read(|tx|tx.get_task("p","T-7")).unwrap().unwrap().state,task_before);
        assert!(store.read(|tx|tx.get_review_verdict("p","T-7",1)).unwrap().is_none());
        let replay=run_send(&mut store,&env("r-1",TEMP_PANE),&mut herdr,&c,"ok".into(),None,T0+2,false).unwrap();
        assert!(replay.contains("replayed"));
    }

    #[test]
    fn direction_result_rejects_lane_and_other_reviewer() {
        let (mut store,mut herdr)=review_world();
        store.write(|tx| {
            testing::seed_temp_seat(tx,"p","r-2","T-7","w1:p4","opencode",T0)?;
            tx.set_meta("engine.direction.p.T-7.1",&serde_json::json!({"seat":"p:r-1","generation":1,"state":"running","sha":"abc123"}).to_string())
        }).unwrap();
        herdr.add_agent("w1:p4","opencode",AgentStatus::Idle);
        let mut c=send_cmd("result"); c.task=Some("T-7".into()); c.attempt=Some(1); c.direction=Some("ok".into());
        for (name,pane) in [("heat",LANE_PANE),("r-2","w1:p4")] {
            let err=run_send(&mut store,&env(name,pane),&mut herdr,&c,"ok".into(),None,T0+1,false).unwrap_err();
            assert_eq!(err.code,gear_model::ErrorCode::Forbidden);
        }
        c.direction=None; c.contract=Some(1); c.candidate=Some("abc1234".into());
        let err=run_send(&mut store,&env("r-1",TEMP_PANE),&mut herdr,&c,"ok".into(),None,T0+1,false).unwrap_err();
        assert_eq!(err.code,gear_model::ErrorCode::Forbidden);
    }

    #[test]
    fn direction_result_after_lane_submission_preserves_submitted_state() {
        let (mut store, mut herdr) = review_world();
        store.write(|tx| tx.set_meta("engine.direction.p.T-7.1", &serde_json::json!({
            "seat":"p:r-1", "generation":1, "state":"running", "sha":"checked123"
        }).to_string())).unwrap();
        let mut lane = send_cmd("result");
        lane.task=Some("T-7".into()); lane.attempt=Some(1); lane.contract=Some(1);
        lane.candidate=Some("c0ffee1".into()); lane.request_id=Some("lane-result".into());
        run_send(&mut store,&env("heat",LANE_PANE),&mut herdr,&lane,"done".into(),None,T0+1,false).unwrap();
        let task_before=store.read(|tx|tx.get_task("p","T-7")).unwrap().unwrap().state;
        assert_eq!(store.read(|tx|tx.get_attempt("p","T-7",1)).unwrap().unwrap().state,gear_model::AttemptState::Submitted);
        assert_eq!(task_before,gear_model::TaskState::Verifying);
        let mut direction=send_cmd("result");
        direction.task=Some("T-7".into()); direction.attempt=Some(1); direction.direction=Some("ok".into());
        run_send(&mut store,&env("r-1",TEMP_PANE),&mut herdr,&direction,"ok".into(),None,T0+2,false).unwrap();
        let history=store.read(|tx|tx.thread_messages("p","T-7:2")).unwrap();
        assert_eq!(history.len(),1);
        assert_eq!(serde_json::from_str::<serde_json::Value>(history[0].meta.as_deref().unwrap()).unwrap(),serde_json::json!({"direction":true,"sha":"checked123"}));
        assert_eq!(store.read(|tx|tx.get_attempt("p","T-7",1)).unwrap().unwrap().state,gear_model::AttemptState::Submitted);
        assert_eq!(store.read(|tx|tx.get_task("p","T-7")).unwrap().unwrap().state,task_before);
        assert!(store.read(|tx|tx.get_review_verdict("p","T-7",1)).unwrap().is_none());
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
        // T-241 rework: the fake injects exactly one refusal, and an urgent
        // doorbell to a *working* lane now spends it on the Escape that cuts the
        // turn (the five ops.rs tests assert that order). This test is about the
        // refused prompt and its recovery, so the lane is idle here: no Escape
        // is sent, and the refusal lands on the prompt itself.
        let (mut store, mut herdr) = working_world("codex", AgentStatus::Idle);
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
                        mechanical: None,
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

    /// §5.1 (T-193): a review is charged from the reviewer's own clock — the
    /// review round's start — and never from the writer's check-in. The
    /// writer's attempt carries the writer's times only.
    #[test]
    fn a_review_is_charged_from_the_reviewers_own_clock() {
        let (mut store, mut herdr) = world_with_task();
        herdr.add_agent("w1:p9", "codex", AgentStatus::Idle);
        // The lane hands the candidate in: the writer checked in at T0 and its
        // packet went out at T0+10.
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
        store
            .write(|tx| tx.set_attempt_sent("p", "T-1", 1, T0 + 10))
            .unwrap();
        // A reviewer seat, bound to the task.
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w1:p9", "codex", T0)?;
                tx.set_seat_task("p:review-1", "T-1")
            })
            .unwrap();
        // The review round starts an hour later: the reviewer worked from
        // there, and the writer's hour is not the reviewer's.
        store
            .write(|tx| {
                tx.insert_round(&gear_store::NewRound {
                    key: gear_store::RoundKey {
                        project: "p".into(),
                        task: "T-1".into(),
                        attempt: 1,
                        role: "review".into(),
                    },
                    seat: "p:review-1".into(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T0 + 3600,
                })
            })
            .unwrap();

        // The verdict five minutes after the review started: inside the
        // review's own budget, even though the writer checked in an hour
        // before.
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
        // The writer's attempt still carries the writer's times only.
        let attempt = store.read(|tx| tx.get_attempt("p", "T-1", 1)).unwrap().unwrap();
        assert_eq!(attempt.sent_at, Some(T0 + 10), "{attempt:?}");
        assert_eq!(attempt.started, T0, "{attempt:?}");
    }

    /// §5.1, §6.2 (T-193): a review over its own budget needs a friction
    /// report — the reviewer's own clock says so, not the writer's.
    #[test]
    fn a_review_over_its_own_budget_needs_a_friction_report() {
        let (mut store, mut herdr) = world_with_task();
        herdr.add_agent("w1:p9", "codex", AgentStatus::Idle);
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
        store
            .write(|tx| tx.set_attempt_sent("p", "T-1", 1, T0 + 10))
            .unwrap();
        store
            .write(|tx| {
                testing::seed_active_seat(tx, "p", "review-1", SeatRole::Temp, "w1:p9", "codex", T0)?;
                tx.set_seat_task("p:review-1", "T-1")
            })
            .unwrap();
        store
            .write(|tx| {
                tx.insert_round(&gear_store::NewRound {
                    key: gear_store::RoundKey {
                        project: "p".into(),
                        task: "T-1".into(),
                        attempt: 1,
                        role: "review".into(),
                    },
                    seat: "p:review-1".into(),
                    generation: 1,
                    cli: "codex".into(),
                    model: None,
                    start_at: T0 + 3600,
                })
            })
            .unwrap();

        // The verdict half an hour after the review started: over the review's
        // budget (30 min by default).
        let mut c = send_cmd("review");
        c.task = Some("T-1".into());
        c.attempt = Some(1);
        c.verdict = Some("pass".into());
        let error = run_send(
            &mut store,
            &env("review-1", "w1:p9"),
            &mut herdr,
            &c,
            "all good".into(),
            None,
            T0 + 3600 + 1801,
            false,
        )
        .unwrap_err();
        assert!(
            error.detail.contains("FRICTION_REQUIRED") || error.detail.contains("--friction"),
            "{}",
            error.detail
        );

        // With a report: it is stored, and it is required.
        c.friction = Some("the reviewer's checkout was slow".into());
        run_send(
            &mut store,
            &env("review-1", "w1:p9"),
            &mut herdr,
            &c,
            "all good".into(),
            None,
            T0 + 3600 + 1801,
            false,
        )
        .unwrap();
        let reports = store.read(|tx| tx.frictions_of_project("p")).unwrap();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(reports[0].role, "review");
        assert!(reports[0].required, "the report was required: {reports:?}");
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
        scratch(&format!("gear-lock-{tag}"))
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

    /// T-398: the start line says which build the engine runs, so a log shows
    /// a replaced binary being run by an old engine.
    #[test]
    fn the_engine_start_line_names_the_build_it_runs() {
        let line = engine_start_line(42, std::path::Path::new("/state/gear.db"), "0.5.0", "abc1234");
        assert_eq!(line, "engine start pid=42 db=/state/gear.db version=0.5.0 build=abc1234\n");
        // This binary's own line: the crate version and the sha build.rs read.
        let line = engine_start_line(std::process::id(), std::path::Path::new("/x/gear.db"), env!("CARGO_PKG_VERSION"), env!("GEAR_BUILD_SHA"));
        assert!(line.contains("version="), "{line}");
        assert!(line.contains("build=") && !line.contains("build=\n"), "{line}");
    }

    /// T-398 (rework F1): the embedded sha is the commit this binary was built
    /// from. build.rs reruns when the branch's ref moves, so a rebuild after a
    /// commit on the same branch cannot keep the preceding commit's sha.
    #[test]
    fn the_build_sha_is_the_commit_this_binary_was_built_from() {
        let head = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .output();
        // No git, or no repository: the binary says it was built without one.
        let head = head.ok().filter(|out| out.status.success()).map(|out| {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        });
        match head {
            Some(head) => assert_eq!(env!("GEAR_BUILD_SHA"), head),
            None => assert_eq!(env!("GEAR_BUILD_SHA"), "unknown"),
        }
    }

    /// T-398: a binary replaced on disk under a running engine is told once —
    /// one engine-log line and one captain item per project, not one a tick.
    #[test]
    fn a_changed_binary_on_disk_is_told_once_over_two_ticks() {
        let (mut store, mut herdr) = world();
        let dir = scratch("gear-binary");
        let binary = dir.join("gear");
        std::fs::write(&binary, b"the old build").unwrap();
        let mut watch = BinaryWatch::at_start(binary.clone());
        // A rebuild replaces the file: new content, new modification time (a
        // coarse file system may keep the second, so the time is set by hand).
        std::fs::write(&binary, b"the new build").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&binary)
            .unwrap()
            .set_modified(later)
            .unwrap();

        let mut clock = || T0;
        let mut tick = |_: &mut Store,
                        _: &mut dyn gear_herdr::HerdrAct,
                        _: &gear_engine::EngineCfg,
                        _: i64| Ok(gear_engine::TickReport::default());
        let mut out: Vec<u8> = Vec::new();
        run_engine_with(
            &mut store,
            &mut herdr,
            &gear_engine::EngineCfg::default(),
            &mut clock,
            Some(2),
            &mut out,
            &|| false,
            &mut tick,
            &mut watch,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        assert_eq!(text.matches(BINARY_CHANGED).count(), 1, "one log line over two ticks: {text}");
        let items = store.read(|tx| tx.thread_messages("p", "engine:binary-changed")).unwrap();
        assert_eq!(items.len(), 1, "one captain item: {items:?}");
        assert_eq!(items[0].recipient, "p:captain");
        std::fs::remove_dir_all(&dir).ok();
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
            &|| false,
            &mut failing,
            &mut BinaryWatch::at_start(std::path::PathBuf::from("/nonexistent/gear")),
        )
        .unwrap();

        assert_eq!(tick_calls.get(), 3, "a failing tick does not stop the loop");
        assert!(String::from_utf8(out).unwrap().contains("boom"));
    }

    #[test]
    fn stop_during_a_tick_leaves_the_loop_after_it() {
        let (mut store, mut herdr) = world();
        let mut clock = || T0;
        // `gear admin stop` arrives while this tick (and the merge in it) runs.
        let stop = std::cell::Cell::new(false);
        let tick_calls = std::cell::Cell::new(0);
        let mut tick = |_: &mut Store,
                        _: &mut dyn gear_herdr::HerdrAct,
                        _: &gear_engine::EngineCfg,
                        _: i64| {
            tick_calls.set(tick_calls.get() + 1);
            stop.set(true);
            Ok(gear_engine::TickReport {
                settled: vec!["p:T-1/1".to_string()],
                ..gear_engine::TickReport::default()
            })
        };
        let mut out: Vec<u8> = Vec::new();
        // No tick limit: only the stop ends the loop, and the tick's report is
        // written before it leaves. (The sleep checks the flag first, so no
        // test waits on the wall clock.)
        run_engine_with(
            &mut store,
            &mut herdr,
            &gear_engine::EngineCfg::default(),
            &mut clock,
            None,
            &mut out,
            &|| stop.get(),
            &mut tick,
            &mut BinaryWatch::at_start(std::path::PathBuf::from("/nonexistent/gear")),
        )
        .unwrap();

        assert_eq!(tick_calls.get(), 1, "the tick that saw the stop finishes, then the loop leaves");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("p:T-1/1"), "the running tick's report is written: {text}");
    }

    #[test]
    fn stop_already_set_runs_no_tick() {
        let (mut store, mut herdr) = world();
        let mut clock = || T0;
        let tick_calls = std::cell::Cell::new(0);
        let mut tick = |_: &mut Store,
                        _: &mut dyn gear_herdr::HerdrAct,
                        _: &gear_engine::EngineCfg,
                        _: i64| {
            tick_calls.set(tick_calls.get() + 1);
            Ok(gear_engine::TickReport::default())
        };
        let mut out: Vec<u8> = Vec::new();
        run_engine_with(
            &mut store,
            &mut herdr,
            &gear_engine::EngineCfg::default(),
            &mut clock,
            None,
            &mut out,
            &|| true,
            &mut tick,
            &mut BinaryWatch::at_start(std::path::PathBuf::from("/nonexistent/gear")),
        )
        .unwrap();

        assert_eq!(tick_calls.get(), 0, "a stop before the first tick runs none");
    }

    #[test]
    fn stop_notice_names_every_project_the_engine_pauses() {
        let notice = stop_notice(&["harbor".to_string(), "mast".to_string()]);
        assert_eq!(
            notice,
            "this engine runs 2 project(s) of this store: harbor, mast; each pauses until the \
             engine starts again (the next gear call starts it)"
        );
    }

    #[test]
    fn stop_notice_counts_a_single_project() {
        let notice = stop_notice(&["harbor".to_string()]);
        assert!(notice.starts_with("this engine runs 1 project(s) of this store: harbor;"), "{notice}");
    }

    #[test]
    fn projects_in_reads_the_store_read_only() {
        let dir = scratch("gear-stop");
        let db = dir.join("gear.db");
        let mut store = Store::open(&db).unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "harbor", "sess", T0)?;
                testing::seed_project(tx, "mast", "sess", T0)
            })
            .unwrap();
        let before = std::fs::metadata(&db).unwrap().len();
        assert_eq!(projects_in(&db), Some(vec!["harbor".to_string(), "mast".to_string()]));
        assert_eq!(std::fs::metadata(&db).unwrap().len(), before, "a stop writes nothing");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn projects_in_says_nothing_for_an_unreadable_store() {
        let missing = scratch("gear-stop-missing").join("gear.db");
        assert_eq!(projects_in(&missing), None, "the stop goes on without a notice");
    }

    #[test]
    fn wait_for_lock_retries_until_the_engine_lets_go() {
        let mut tries = 0;
        let mut try_lock = || {
            tries += 1;
            tries == 3
        };
        let mut sleeps = 0;
        let mut sleep = || sleeps += 1;
        assert!(wait_for_lock(&mut try_lock, &mut sleep, 1200));
        assert_eq!(tries, 3, "it keeps trying until the lock is free");
        assert_eq!(sleeps, 2, "one second between tries, none after the last");
    }

    #[test]
    fn wait_for_lock_gives_up_after_tries() {
        let mut tries = 0;
        let mut try_lock = || {
            tries += 1;
            false
        };
        let mut sleeps = 0;
        let mut sleep = || sleeps += 1;
        assert!(!wait_for_lock(&mut try_lock, &mut sleep, 1200));
        assert_eq!(tries, 1200, "20 minutes, once a second");
        assert_eq!(sleeps, 1199, "the last try is not followed by a sleep");
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
                    mechanical: None,
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
        let dir = scratch("gear-standard");
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
            false,
        )
        .unwrap();
        assert_eq!(lane_state(&mut store), gear_model::SeatState::Queued);
        std::fs::remove_file(&path).ok();
    }

}

#[cfg(test)]
mod default_store_tests {
    use super::*;

    #[test]
    fn each_herdr_session_gets_its_own_store_unless_one_is_registered() {
        let home = crate::tests::scratch("gear-default-store");
        let sock = |name: &str| home.join(".config/herdr/sessions").join(name).join("herdr.sock").to_string_lossy().to_string();
        let shared = home.join(".local/state/gear/gear.db");
        assert_eq!(default_store_in(&home, None), shared, "no Herdr: the shared store");
        let plain = home.join(".config/herdr/herdr.sock").to_string_lossy().to_string();
        assert_eq!(default_store_in(&home, Some(&plain)), shared, "the default session: the shared store");
        assert_eq!(
            default_store_in(&home, Some(&sock("b"))),
            home.join(".local/state/gear/b/gear.db"),
            "a named session: its own store"
        );
        register_store(&targets_path_in(&home), &sock("a"), "/x/a.db").expect("registry");
        assert_eq!(default_store_in(&home, Some(&sock("a"))), std::path::PathBuf::from("/x/a.db"), "a registered store wins");
        let _ = std::fs::remove_dir_all(&home);
    }
}
