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
    /// Print the version-matched skill: (none)=SKILL.md | captain | plan-format | errors.
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
fn env_from_process() -> GearResult<gear_app::Env> {
    todo!("slice1")
}

/// Open the store at GEAR_DB (agents) or the admin target (design/14).
fn open_store() -> GearResult<gear_store::Store> {
    todo!("slice1")
}

/// Generate a request id and print `request: <id>` to stderr before any write.
fn announce_request_id(given: Option<String>) -> String {
    todo!("slice1")
}

fn cmd_home(show: Option<&str>, diff: bool, all: bool, json: bool) -> GearResult<()> {
    todo!("slice1: identity::observe (SocketHerdr) -> home::build -> print -> home::record_view")
}

fn cmd_send(c: SendCmd, json: bool) -> GearResult<()> {
    todo!("slice1")
}

fn cmd_reply(c: ReplyCmd, json: bool) -> GearResult<()> {
    todo!("slice1")
}

fn cmd_run() -> GearResult<()> {
    todo!("slice1: single-instance lock, ops::recover_on_start, loop tick every cfg.tick_s")
}

/// Skill topics and their embedded text. The skill is compiled into the
/// binary, so the printed rules always match this binary's version.
const SKILL_TOPICS: &[(&str, &str)] = &[
    ("main", include_str!("../../../skills/gear/SKILL.md")),
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
