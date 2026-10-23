//! `gear` — the only binary (design/13, /14, /16).
//!
//! Agent commands: `gear`, `gear send`, `gear reply`, `gear plan apply`, `gear --skill`.
//! User commands: `gear run`, `gear admin …`, `gear --all`.
//!
//! Output contract: success → human text (or JSON with --json); failure →
//! `ERROR <CODE>: <detail>` on stderr with `ErrorCode::exit_code()`.
//! Every write command prints `request: <id>` to stderr **before** committing.

#![allow(unused_variables, dead_code)] // skeleton

use clap::{Args, Parser, Subcommand};
use gear_model::{GearError, GearResult};

#[derive(Parser, Debug)]
#[command(name = "gear", about = "GEAR: keep an agent fleet working under one captain")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Expand an item: message/thread id, task id, `queue`, `draft`.
    #[arg(long, global = false)]
    show: Option<String>,
    /// With --show <task>: diff since the last work check.
    #[arg(long)]
    diff: bool,
    /// User only: read-only overview of every project.
    #[arg(long)]
    all: bool,
    /// Print the version-matched skill: (none)=SKILL.md | conventions | decompose | agent:opencode | agent:codex.
    #[arg(long, num_args = 0..=1, default_missing_value = "main")]
    skill: Option<String>,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Report: progress | question | blocked | result | ack | instruction | review.
    Send(SendCmd),
    /// Captain: handle exact message ids.
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
    /// User: run the engine loop (plugin pane entrypoint).
    Run,
    /// User: project/seat administration.
    Admin {
        #[command(subcommand)]
        op: AdminOp,
    },
}

#[derive(Args, Debug)]
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
    #[arg(long, conflicts_with_all = ["wait", "note"])]
    resolve: bool,
    #[arg(long, value_name = "WHO")]
    wait: Option<String>,
    #[arg(long, requires = "wait")]
    until: Option<String>,
    #[arg(long)]
    note: bool,
    #[arg(long)]
    downgrade: bool,
    #[arg(long = "next-check")]
    next_check: Option<String>,
    #[arg(long = "request-id")]
    request_id: Option<String>,
    #[arg(long, short)]
    message: String,
}

#[derive(Subcommand, Debug)]
enum PlanOp {
    /// Apply a plan draft (default: the project's plan.draft.md).
    Apply {
        path: Option<std::path::PathBuf>,
        #[arg(long, value_name = "V-ID")]
        keep: Option<String>,
        #[arg(long, requires = "keep")]
        next: Option<String>,
        #[arg(long = "request-id")]
        request_id: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum AdminOp {
    Init { config: std::path::PathBuf },
    /// Print what this machine offers, for the captain's kickoff proposal.
    Kickoff,
    /// Stop the background engine (it starts again on the next `gear` call).
    Stop,
    Seat {
        #[command(subcommand)]
        op: SeatOp,
    },
    Captain {
        #[command(subcommand)]
        op: CaptainOp,
    },
}

#[derive(Subcommand, Debug)]
enum SeatOp {
    Confirm { seat: String },
    Remove { seat: String },
}

#[derive(Subcommand, Debug)]
enum CaptainOp {
    Replace {
        #[arg(long)]
        project: String,
        #[arg(long)]
        pane: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        resume: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ERROR {}: {}", e.code, e.detail);
            e.code.exit_code()
        }
    };
    std::process::exit(code);
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
        None => cmd_home(cli.show.as_deref(), cli.diff, cli.all, cli.json),
        Some(Cmd::Send(c)) => cmd_send(c, cli.json),
        Some(Cmd::Reply(c)) => cmd_reply(c, cli.json),
        Some(Cmd::Plan { op }) => match op {
            PlanOp::Apply { path, keep, next, request_id } => {
                cmd_plan(path, keep, next, request_id, cli.json)
            }
        },
        Some(Cmd::Board { deps }) => cmd_board(deps, cli.json),
        Some(Cmd::Run) => cmd_run(),
        Some(Cmd::Admin { op: AdminOp::Init { config } }) => cmd_admin_init(&config, cli.json),
        Some(Cmd::Admin { op: AdminOp::Kickoff }) => cmd_admin_kickoff(cli.json),
        Some(Cmd::Admin { op: AdminOp::Stop }) => cmd_admin_stop(),
        Some(Cmd::Admin { op }) => Err(not_yet("admin seat/captain (after the walking skeleton)")),
    }
}

/// Does this command start the engine when its lock is free (PRD §16)?
///
/// Most commands do: the project must keep moving. These do not:
/// - `run` *is* the engine, and `admin stop` stops it;
/// - `board` only observes — starting the engine would hide the "engine down"
///   warning it exists to show (PRD §15, the review of T-3);
/// - `admin kickoff` only reads — starting the engine would open (and migrate)
///   the store, and a kickoff runs before the project exists (the review of
///   T-10). `--skill` and `--help` never get here.
fn starts_engine(cmd: &Option<Cmd>) -> bool {
    !matches!(
        cmd,
        Some(Cmd::Run)
            | Some(Cmd::Admin { op: AdminOp::Stop })
            | Some(Cmd::Admin { op: AdminOp::Kickoff })
            | Some(Cmd::Board { .. })
    )
}

fn not_yet(what: &str) -> GearError {
    GearError::new(gear_model::ErrorCode::Forbidden, format!("{what} is not implemented yet"))
}

/// Build `gear_app::Env` from GEAR_CTX / HERDR_PANE_ID / GEAR_ADMIN.
///
/// An absent `GEAR_CTX` means an anonymous caller (the user's own shell or an
/// unmanaged agent); a malformed one is an error, never a silent `None`.
fn env_from_process() -> GearResult<gear_app::Env> {
    env_from(|name| std::env::var(name).ok())
}

/// [`env_from_process`] over an injected variable lookup (testable).
fn env_from(vars: impl Fn(&str) -> Option<String>) -> GearResult<gear_app::Env> {
    let ctx = match vars(gear_model::ctx::ENV_CTX) {
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

/// `gear admin init <config>`: the user registers a project (design/01). The
/// caller check comes first, so an agent or a shell without `GEAR_ADMIN=1`
/// is refused before anything is read. The store is `GEAR_DB`.
fn cmd_admin_init(config: &std::path::Path, json: bool) -> GearResult<()> {
    let env = env_from_process()?;
    if env.ctx.is_some() || !env.admin {
        return Err(GearError::new(
            gear_model::ErrorCode::Forbidden,
            "`gear admin init` runs in the user's own shell with GEAR_ADMIN=1, never in an agent",
        ));
    }
    let text = std::fs::read_to_string(config).map_err(|e| {
        GearError::new(gear_model::ErrorCode::BadFormat, format!("cannot read {}: {e}", config.display()))
    })?;
    let mut store = open_store()?;
    let r = gear_app::admin::init(&mut store, &env, &text, now_ts())?;
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

fn cmd_home(show: Option<&str>, diff: bool, all: bool, json: bool) -> GearResult<()> {
    // `--diff`/`--all` are views that are not implemented yet: reject them by
    // name instead of printing a page that silently ignores them.
    reject_slice2(diff, all)?;
    if let Some(target) = show {
        let env = env_from_process()?;
        let mut store = open_store()?;
        let mut herdr = open_herdr()?;
        let out = run_show(&mut store, &env, &mut herdr, target, json)?;
        println!("{out}");
        return Ok(());
    }
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    run_home(&mut store, &env, &mut herdr, now_ts(), json, &mut std::io::stdout())
}

/// The `--diff`/`--all` views are slice 2: reject them by name instead of
/// printing a page that silently ignores them.
fn reject_slice2(diff: bool, all: bool) -> GearResult<()> {
    for (flag, given) in [("--diff", diff), ("--all", all)] {
        if given {
            return Err(GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!("{flag} is not supported in slice 1"),
            ));
        }
    }
    Ok(())
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
    let fp = gear_app::identity::observe(env, herdr)?;
    let text = store.read(|tx| {
        // The identity of the home page: a registered seat of the project.
        let (ctx, _seat) = gear_app::identity::require_seat(tx, env, fp.as_ref(), None, None)?;
        gear_app::plan::render_task_packet(tx, &ctx.project, target)
    })?;
    if json {
        return serde_json::to_string(&text).map_err(json_error);
    }
    Ok(text)
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
    let fp = gear_app::identity::observe(env, herdr)?;
    let home = gear_app::home::build(store, env, fp.as_ref(), now)?;
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
    gear_board::run(&mut store, &opts, &mut out)
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
    let out = run_reply(&mut store, &env, &mut herdr, &c, now_ts(), json)?;
    println!("{out}");
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
            "give the draft path: gear plan apply <path>".to_string(),
        ));
    };
    let text = std::fs::read_to_string(path).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let draft = gear_app::draft::parse(&text)?;
    let fp = gear_app::identity::observe(env, herdr)?;
    let result = gear_app::plan::apply(store, env, fp.as_ref(), &draft, &text, request_id, now)?;
    if json {
        return serde_json::to_string(&result).map_err(json_error);
    }
    Ok(render_apply(&result))
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
    lines.join("\n")
}

/// `gear send` body, over injected state (the command itself only wires this
/// to the process environment, the socket and the store).
fn run_send(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
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
        request_id: require_request_id(&c.request_id)?,
    };
    let fp = gear_app::identity::observe(env, herdr)?;
    let result = gear_app::mail::send(store, env, fp.as_ref(), &args, now)?;
    if json {
        return serde_json::to_string(&result).map_err(json_error);
    }
    {
        let mut line = format!(
            "Delivered to {} inbox {} (thread {})",
            result.recipient, result.msg_id, result.thread
        );
        if result.replayed {
            line.push_str(" [replayed]");
        }
        Ok(line)
    }
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
    reject_unsupported(&[("--next-check", c.next_check.is_some())])?;
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
    let fp = gear_app::identity::observe(env, herdr)?;
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
        | gear_model::MsgKind::Blocked
        | gear_model::MsgKind::Result
        | gear_model::MsgKind::Ack
        | gear_model::MsgKind::Instruction
        | gear_model::MsgKind::Review => Ok(kind),
        gear_model::MsgKind::Reply | gear_model::MsgKind::System => Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("--kind {text} is not a `gear send` kind"),
        )),
    }
}

/// Reply mode and its wait fields: `--wait <who> [--until <t>]`, `--note`, or
/// `--resolve` (the default; `--downgrade` alone also means a note).
fn reply_mode(c: &ReplyCmd, now: i64) -> GearResult<(gear_app::mail::ReplyMode, Option<String>, Option<i64>)> {
    use gear_app::mail::ReplyMode;
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
    let path = match std::env::var_os("HERDR_SOCKET_PATH") {
        Some(path) => std::path::PathBuf::from(path),
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
    for kind in ["opencode", "claude", "codex"] {
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

/// Skill topics and their embedded text. The skill is compiled into the
/// binary, so the printed rules always match this binary's version.
/// The `--skill` help text must list exactly these names (test `help_lists_every_skill_topic`).
const SKILL_TOPICS: &[(&str, &str)] = &[
    ("main", include_str!("../../../skills/gear/SKILL.md")),
    ("conventions", include_str!("../../../docs/design/17-conventions.md")),
    ("decompose", include_str!("../../../skills/gear/references/decompose.md")),
    ("agent:opencode", include_str!("../../../skills/gear/references/agents/opencode.md")),
    ("agent:codex", include_str!("../../../skills/gear/references/agents/codex.md")),
];

fn print_skill(topic: &str) -> GearResult<()> {
    let Some((_, text)) = SKILL_TOPICS.iter().find(|(name, _)| *name == topic) else {
        let available = SKILL_TOPICS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ");
        return Err(GearError::new(
            gear_model::ErrorCode::BadFormat,
            format!("unknown skill topic {topic}; available: {available}"),
        ));
    };
    // The text is printed unchanged: no added newline, nothing trimmed.
    print!("{text}");
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
        for (name, _) in SKILL_TOPICS {
            let shown = if *name == "main" { "(none)=SKILL.md" } else { name };
            assert!(help.contains(shown), "--skill help misses topic {name}");
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
            downgrade: false,
            next_check: None,
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
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\ndeps: -\n"
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
        let tasks = store.read(|tx| tx.tasks_of_project("p")).unwrap();
        assert_eq!(tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["T-1"]);
    }

    #[test]
    fn plan_apply_without_path_is_bad_format() {
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
        assert!(err.detail.contains("draft path"), "{}", err.detail);
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

    #[test]
    fn send_candidate_reaches_mail() {
        let (mut store, mut herdr) = world();
        // A started attempt on T-7, so a result can be submitted.
        store
            .write(|tx| {
                tx.insert_task(&TaskRow {
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
                    updated: T0,
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
                    updated: T0,
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
        // The captain's inbox holds the item.
        let inbox = store.read(|tx| tx.actionable_for("p", "p:captain", T0 + 2)).unwrap();
        assert_eq!(inbox.len(), 1);
        assert!(inbox[0].body.contains("pass"), "{}", inbox[0].body);
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
            vec!["--dangerously-bypass-approvals-and-sandbox".to_string()]
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
            ("GEAR_CTX", "oxford:heat:7"),
            ("HERDR_PANE_ID", "w1:p2"),
            ("GEAR_ADMIN", "1"),
        ]))
        .unwrap();
        assert_eq!(
            env.ctx,
            Some(gear_model::GearCtx { project: "oxford".into(), seat: "heat".into(), generation: 7 })
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
                    updated: T0,
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
    fn slice2_flags_are_bad_format() {
        for (flag, rejected) in [
            ("--diff", reject_slice2(true, false)),
            ("--all", reject_slice2(false, true)),
        ] {
            let err = rejected.unwrap_err();
            assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{flag}");
            assert!(err.detail.contains(flag), "{flag}: {}", err.detail);
        }
        assert!(reject_slice2(false, false).is_ok());
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
}
