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
    /// Print the version-matched skill: (none)=SKILL.md | conventions | decompose | agent:opencode.
    #[arg(long, num_args = 0..=1, default_missing_value = "main")]
    skill: Option<String>,
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Report: progress | question | blocked | result | ack | instruction.
    Send(SendCmd),
    /// Captain: handle exact message ids.
    Reply(ReplyCmd),
    /// Captain: plan operations.
    Plan {
        #[command(subcommand)]
        op: PlanOp,
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
        return print_skill(topic);
    }
    match cli.cmd {
        None => cmd_home(cli.show.as_deref(), cli.diff, cli.all, cli.json),
        Some(Cmd::Send(c)) => cmd_send(c, cli.json),
        Some(Cmd::Reply(c)) => cmd_reply(c, cli.json),
        Some(Cmd::Plan { op }) => Err(not_yet("plan (slice 2)")),
        Some(Cmd::Run) => cmd_run(),
        Some(Cmd::Admin { op }) => Err(not_yet("admin (slice 3)")),
    }
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
    reject_slice2(show, diff, all)?;
    let env = env_from_process()?;
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    run_home(&mut store, &env, &mut herdr, now_ts(), json, &mut std::io::stdout())
}

/// The `--show`/`--diff`/`--all` views are slice 2: reject them by name
/// instead of printing a page that silently ignores them.
fn reject_slice2(show: Option<&str>, diff: bool, all: bool) -> GearResult<()> {
    for (flag, given) in [("--show", show.is_some()), ("--diff", diff), ("--all", all)] {
        if given {
            return Err(GearError::new(
                gear_model::ErrorCode::BadFormat,
                format!("{flag} is not supported in slice 1"),
            ));
        }
    }
    Ok(())
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
    let out = run_send(&mut store, &env, &mut herdr, &c, body, now_ts(), json)?;
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

/// `gear send` body, over injected state (the command itself only wires this
/// to the process environment, the socket and the store).
fn run_send(
    store: &mut gear_store::Store,
    env: &gear_app::Env,
    herdr: &mut dyn gear_herdr::HerdrRead,
    c: &SendCmd,
    body: String,
    now: i64,
    json: bool,
) -> GearResult<String> {
    reject_unsupported(&[
        ("--need", c.need.is_some()),
        ("--candidate", c.candidate.is_some()),
        ("--preempted", c.preempted.is_some()),
        ("--saved", c.saved.is_some()),
    ])?;
    let args = gear_app::mail::SendArgs {
        kind: parse_kind(&c.kind)?,
        body,
        task: c.task.clone(),
        attempt: c.attempt,
        contract: c.contract,
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
        | gear_model::MsgKind::Instruction => Ok(kind),
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
    // The single-instance lock lives next to the database (design/08 startup).
    let db = std::env::var_os("GEAR_DB").ok_or_else(|| {
        GearError::new(gear_model::ErrorCode::NoTarget, "GEAR_DB is not set")
    })?;
    let dir = std::path::Path::new(&db)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let _lock = take_engine_lock(&dir)?; // held for the life of the loop
    let mut store = open_store()?;
    let mut herdr = open_herdr()?;
    let cfg = gear_engine::EngineCfg::default();
    let mut clock = now_ts;
    run_engine(&mut store, &mut herdr, &cfg, &mut clock, None, &mut std::io::stdout())
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
                writeln!(
                    out,
                    "tick: observe_failed={:?} intended={} executed={} confirmed={} flagged={}",
                    report.observe_failed,
                    report.intended.len(),
                    report.executed.len(),
                    report.confirmed.len(),
                    report.flagged.len()
                )
                .map_err(write_error)?;
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
const SKILL_TOPICS: &[(&str, &str)] = &[
    ("main", include_str!("../../../skills/gear/SKILL.md")),
    ("conventions", include_str!("../../../docs/design/17-conventions.md")),
    ("decompose", include_str!("../../../skills/gear/references/decompose.md")),
    ("agent:opencode", include_str!("../../../skills/gear/references/agents/opencode.md")),
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
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_model::{GearCtx, MsgKind, MsgState, OpKind, OpState, SeatRole};
    use gear_store::{testing, Store};
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
        let flags: [(&str, fn(&mut SendCmd)); 4] = [
            ("--need", |c| c.need = Some("a decision".into())),
            ("--candidate", |c| c.candidate = Some("T-2".into())),
            ("--preempted", |c| c.preempted = Some("T-3".into())),
            ("--saved", |c| c.saved = Some("branch".into())),
        ];
        for (flag, set) in flags {
            let (mut store, mut herdr) = world();
            let mut c = send_cmd("question");
            set(&mut c);
            let err =
                run_send(&mut store, &env("heat", LANE_PANE), &mut herdr, &c, "x".into(), T0 + 1, false)
                    .unwrap_err();
            assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{flag}");
            assert!(err.detail.contains(flag), "{flag}: {}", err.detail);
        }
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

    #[test]
    fn slice2_flags_are_bad_format() {
        for (flag, rejected) in [
            ("--show", reject_slice2(Some("M-1"), false, false)),
            ("--diff", reject_slice2(None, true, false)),
            ("--all", reject_slice2(None, false, true)),
        ] {
            let err = rejected.unwrap_err();
            assert_eq!(err.code, gear_model::ErrorCode::BadFormat, "{flag}");
            assert!(err.detail.contains(flag), "{flag}: {}", err.detail);
        }
        assert!(reject_slice2(None, false, false).is_ok());
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
