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
    // The socket is HERDR_SOCKET_PATH, else the default session socket (04).
    let socket = match std::env::var_os("HERDR_SOCKET_PATH") {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
            home.join(".config/herdr/herdr.sock")
        }
    };
    let mut herdr = gear_herdr::SocketHerdr::connect(&socket).map_err(|e| {
        GearError::new(
            gear_model::ErrorCode::NotRegistered,
            format!("cannot reach herdr at {}: {}", socket.display(), e.0),
        )
    })?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    run_home(&mut store, &env, &mut herdr, now, json, &mut std::io::stdout())
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

#[cfg(test)]
mod tests {
    use super::*;
    use gear_herdr::fake::FakeHerdr;
    use gear_herdr::AgentStatus;
    use gear_model::{GearCtx, MsgKind, MsgState, SeatRole};
    use gear_store::{testing, Store};
    use std::collections::BTreeMap;

    /// A variable lookup over a fixed map (production reads the process env).
    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name: &str| map.get(name).cloned()
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

    const T0: i64 = 1_800_000_000;
    const CAP_PANE: &str = "w1:p1";
    const LANE_PANE: &str = "w1:p2";

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
