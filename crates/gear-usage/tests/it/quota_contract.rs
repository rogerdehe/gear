//! T-138: the quota readers of §9.1 — `read_quota` for minimax, opencode-go
//! and codex.
//!
//! The HTTP calls go through `curl`, so the tests put a **fake curl** on the
//! PATH: it prints a recorded body (hand-written from §9.1), writes the
//! header it was given to a file, and refuses to run when the key is on the
//! command line.

use std::path::Path;
use std::sync::Mutex;

use gear_usage::{codex_day_dir, read_quota_in, QuotaReader};
use serde_json::json;

use crate::shared::{iso, now, write_lines, T13};

/// The PATH is the one thing the tests of this file share: only one of them
/// may hold it at a time.
static PATH_LOCK: Mutex<()> = Mutex::new(());

const KEY: &str = "sk-secret-key";

/// A fake curl that prints `body` and writes the header of the call to
/// `headers`. The answer carries the status on its last line, as curl writes
/// it with `-w`.
fn curl_script(body: &str, headers: &Path) -> String {
    curl_script_with_status(body, headers, 200)
}

/// The same, with the HTTP status of the answer.
fn curl_script_with_status(body: &str, headers: &Path, status: u16) -> String {
    let script = r#"#!/bin/sh
for arg in "$@"; do
  case "$arg" in
    *sk-secret*) echo "the key is on the command line" >&2; exit 3;;
  esac
done
echo "$@" > "ARGS"
cat > "HEADERS"
cat <<'JSON'
BODY
JSON
printf '\n%s' "STATUS"
"#;
    script
        .replace("ARGS", &headers.with_file_name("args.txt").display().to_string())
        .replace("HEADERS", &headers.display().to_string())
        .replace("BODY", body)
        .replace("STATUS", &status.to_string())
}

/// A fake curl that fails with `code` and says why.
fn failing_curl(code: i32, reason: &str) -> String {
    format!("#!/bin/sh\necho \"curl: ({code}) {reason}\" >&2\nexit {code}\n")
}

/// A fake curl that prints the body and no status line.
fn curl_script_without_status(body: &str, headers: &Path) -> String {
    let script = r#"#!/bin/sh
cat > "HEADERS"
cat <<'JSON'
BODY
JSON
"#;
    script
        .replace("HEADERS", &headers.display().to_string())
        .replace("BODY", body)
}

/// Put a fake curl on the PATH, run `f`, and put the PATH back.
fn with_fake_curl(dir: &Path, script: &str, f: impl FnOnce(&Path)) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let curl = bin.join("curl");
    std::fs::write(&curl, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _guard = PATH_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let old = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", bin.display(), old));
    f(dir);
    std::env::set_var("PATH", old);
}

#[test]
fn the_minimax_windows_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let headers = dir.path().join("headers.txt");
    let body = json!({
        "model_remains": [
            {
                "model_name": "MiniMax-M3.1-Flash",
                "current_interval_remaining_percent": 42.5,
                "end_time": 1_793_016_000_000i64,
                "current_weekly_remaining_percent": 80.0,
                "weekly_end_time": 1_793_600_000_000i64
            }
        ],
        "base_resp": {"status_code": 0, "status_msg": "success"}
    })
    .to_string();
    with_fake_curl(
        dir.path(),
        &curl_script(&body, &headers),
        |dir| {
            let reading = read_quota_in(QuotaReader::Minimax, KEY, None, T13, dir).unwrap();
            // The call is the one of §9.1: curl, the timeout, the header on
            // standard input and the URL of the provider.
            let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
            assert!(args.contains("-sS"), "{args}");
            assert!(args.contains("--max-time 15"), "{args}");
            assert!(args.contains("-H @-"), "{args}");
            assert!(
                args.contains("https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains"),
                "{args}"
            );
            assert_eq!(reading.provider, "minimax");
            assert_eq!(reading.read_at, T13);
            assert_eq!(reading.windows.len(), 2, "{reading:?}");
            let five_hours = &reading.windows[0];
            assert_eq!(five_hours.name, "MiniMax-M3.1-Flash 5h");
            // The provider says how much is left; the reading says how much is
            // used.
            assert!((five_hours.used_percent - 57.5).abs() < 1e-9, "{five_hours:?}");
            assert_eq!(five_hours.resets_at, Some(1_793_016_000));
            let weekly = &reading.windows[1];
            assert_eq!(weekly.name, "MiniMax-M3.1-Flash weekly");
            assert!((weekly.used_percent - 20.0).abs() < 1e-9, "{weekly:?}");
            assert_eq!(weekly.resets_at, Some(1_793_600_000));
        },
    );
}

#[test]
fn the_opencode_go_windows_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let headers = dir.path().join("headers.txt");
    let body = json!({
        "usage": {
            "rolling": {"percent": 42.0, "resetsAt": 1_793_016_000_000i64},
            "weekly": {"percent": 55.5, "resetsAt": 1_793_600_000_000i64},
            "monthly": {"percent": 60.25, "resetsAt": 1_796_000_000_000i64}
        }
    })
    .to_string();
    with_fake_curl(
        dir.path(),
        &curl_script(&body, &headers),
        |dir| {
            let reading = read_quota_in(QuotaReader::OpencodeGo, KEY, None, T13, dir).unwrap();
            assert_eq!(reading.provider, "opencode-go");
            assert_eq!(reading.windows.len(), 3, "{reading:?}");
            assert_eq!(reading.windows[0].name, "rolling");
            assert!((reading.windows[0].used_percent - 42.0).abs() < 1e-9);
            assert_eq!(reading.windows[0].resets_at, Some(1_793_016_000));
            assert_eq!(reading.windows[1].name, "weekly");
            assert!((reading.windows[1].used_percent - 55.5).abs() < 1e-9);
            assert_eq!(reading.windows[1].resets_at, Some(1_793_600_000));
            assert_eq!(reading.windows[2].name, "monthly");
            assert!((reading.windows[2].used_percent - 60.25).abs() < 1e-9);
            assert_eq!(reading.windows[2].resets_at, Some(1_796_000_000));
        },
    );
}

#[test]
fn the_key_never_appears_on_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let headers = dir.path().join("headers.txt");
    let body = json!({"usage": {"rolling": {"percent": 1.0, "resetsAt": 1_793_016_000_000i64}}}).to_string();
    with_fake_curl(
        dir.path(),
        &curl_script(&body, &headers),
        |dir| {
            // The fake curl refuses to run when the key is among its
            // arguments: the reading comes back, so the key was not there.
            let reading = read_quota_in(QuotaReader::OpencodeGo, KEY, None, T13, dir).unwrap();
            assert_eq!(reading.windows.len(), 1, "{reading:?}");
            // It was on standard input, in the header.
            let header = std::fs::read_to_string(dir.join("headers.txt")).unwrap();
            assert!(
                header.contains(&format!("Authorization: Bearer {KEY}")),
                "{header}"
            );
        },
    );
}

#[test]
fn an_http_error_gives_the_status_and_the_body() {
    // Curl is happy with a 401, a 429 and a 500: the status has to be read
    // from the answer, or the reason of the error is lost (review F1).
    for (status, body) in [
        (401u16, r#"{"error":"unauthorized"}"#),
        (429, r#"{"error":"rate limited"}"#),
        (500, r#"{"error":"the provider is down"}"#),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let headers = dir.path().join("headers.txt");
        with_fake_curl(
            dir.path(),
            &curl_script_with_status(body, &headers, status),
            |dir| {
                let error = read_quota_in(QuotaReader::Minimax, KEY, None, T13, dir)
                    .err()
                    .expect("an HTTP error is an error");
                assert!(error.contains(&format!("HTTP {status}")), "{error}");
                assert!(error.contains("minimax"), "{error}");
                // The body of the provider is in the reason.
                assert!(error.contains(body), "{error}");
            },
        );
    }
}

#[test]
fn a_call_that_fails_gives_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    with_fake_curl(
        dir.path(),
        &failing_curl(22, "The requested URL returned error: 401"),
        |dir| {
            let error = read_quota_in(QuotaReader::Minimax, KEY, None, T13, dir)
                .err()
                .expect("a call that fails is an error");
            assert!(error.contains("401"), "{error}");
            assert!(error.contains("curl"), "{error}");
        },
    );
}

#[test]
fn an_answer_without_a_status_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let headers = dir.path().join("headers.txt");
    // A fake that prints the body and nothing else: the status is missing.
    with_fake_curl(
        dir.path(),
        &curl_script_without_status(r#"{"usage":{}}"#, &headers),
        |dir| {
            let error = read_quota_in(QuotaReader::OpencodeGo, KEY, None, T13, dir)
                .err()
                .expect("an answer without a status is an error");
            assert!(error.contains("status"), "{error}");
        },
    );
}

#[test]
fn a_timeout_gives_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    with_fake_curl(
        dir.path(),
        &failing_curl(28, "Operation timed out after 15001 milliseconds"),
        |dir| {
            let error = read_quota_in(QuotaReader::OpencodeGo, KEY, None, T13, dir)
                .err()
                .expect("a timeout is an error");
            assert!(error.contains("timed out"), "{error}");
            assert!(error.contains("28"), "{error}");
        },
    );
}

#[test]
fn a_response_that_is_not_the_one_of_the_provider_gives_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let headers = dir.path().join("headers.txt");
    with_fake_curl(
        dir.path(),
        &curl_script("{\"error\": \"no key\"}", &headers),
        |dir| {
            let error = read_quota_in(QuotaReader::Minimax, KEY, None, T13, dir)
                .err()
                .expect("a body without model_remains is an error");
            assert!(error.contains("model_remains"), "{error}");
        },
    );
    with_fake_curl(
        dir.path(),
        &curl_script("not json at all", &headers),
        |dir| {
            let error = read_quota_in(QuotaReader::OpencodeGo, KEY, None, T13, dir)
                .err()
                .expect("a body that is not JSON is an error");
            assert!(error.contains("JSON"), "{error}");
        },
    );
}

#[test]
fn the_codex_windows_are_read_from_the_newest_count_of_the_rollout() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let thread = "0199aaaa-bbbb-cccc-dddd-eeeeffff0000";
    let count = |time: i64, used: f64, resets: i64| {
        json!({
            "timestamp": iso(time),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 10}},
                "rate_limits": {
                    "primary": {"used_percent": used, "window_minutes": 300, "resets_at": resets},
                    "secondary": {"used_percent": used + 30.0, "window_minutes": 10080, "resets_at": resets + 100}
                }
            }
        })
    };
    let path = write_lines(
        &codex_day_dir(&home, now()).join(format!("rollout-2026-10-26T12-00-00-{thread}.jsonl")),
        &[
            count(T13 - 3600, 5.0, 1_793_016_000),
            count(T13, 12.5, 1_793_020_000),
        ],
    );
    assert!(path.is_file());

    let reading = read_quota_in(
        QuotaReader::Codex,
        "",
        Some(thread),
        T13,
        &home,
    )
    .unwrap();
    assert_eq!(reading.provider, "codex");
    assert_eq!(reading.read_at, T13);
    assert_eq!(reading.windows.len(), 2, "{reading:?}");
    assert_eq!(reading.windows[0].name, "primary");
    assert!((reading.windows[0].used_percent - 12.5).abs() < 1e-9);
    assert_eq!(reading.windows[0].resets_at, Some(1_793_020_000));
    assert_eq!(reading.windows[1].name, "secondary");
    assert!((reading.windows[1].used_percent - 42.5).abs() < 1e-9);
    assert_eq!(reading.windows[1].resets_at, Some(1_793_020_100));
}

#[test]
fn a_codex_session_without_a_rollout_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = read_quota_in(
        QuotaReader::Codex,
        "",
        Some("0199not-a-thread"),
        T13,
        dir.path(),
    )
    .err()
    .expect("a session without a rollout is an error");
    assert!(error.contains("rollout"), "{error}");
}

#[test]
fn the_names_of_the_readers_are_the_names_of_the_config() {
    for (name, reader) in [
        ("minimax", QuotaReader::Minimax),
        ("opencode-go", QuotaReader::OpencodeGo),
        ("codex", QuotaReader::Codex),
    ] {
        assert_eq!(QuotaReader::from_name(name), Some(reader), "{name}");
        assert_eq!(reader.name(), name);
    }
    assert_eq!(QuotaReader::from_name("nothing"), None);
}
