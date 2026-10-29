//! Contract for [`GitHub`], the forge over the `gh` CLI (T-78).
//!
//! Every test runs a fake `gh` — a shell script in a scratch directory, named
//! as the executable ([`GitHub::with_path`]) — that records the argv and the
//! sequence of calls it was given and answers recorded JSON. That is what the
//! engine's forge calls look like from `gh`'s side: the command line, the
//! parsing of the answer, the draft-to-ready step before a merge (T-78, F1),
//! and a typed error when the tool is missing, fails, answers nonsense or runs
//! too long.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gear_forge::{marker, Error, Filter, Forge, GitHub, MergeMethod, PrRef};

/// A fake `gh` for `dir`: it records its argv (each argument closed by a
/// `\037`, so an argument may hold newlines) and one line per call
/// (`pr create`, `pr view`, …). Its answer is chosen by the first two
/// arguments: `out-<cmd>-<sub>`, `err-…`, `code-…` when present, else the
/// generic `out`, `err`, `code`. Every file is optional. Shell builtins only —
/// it runs with a `PATH` that holds nothing but itself.
///
/// The script is **one per test file** (T-128): it reads its own directory from
/// `$0`, and every test's `gh` is a link to this one file. A fresh script per
/// test meant a fresh executable image per test on macOS — the whole cost of
/// this file, seconds for a handful of tests; one image is warm after the first
/// call, while the answers stay per test, in that test's own directory.
const FAKE_GH: &str = r#"#!/bin/sh
dir=$(dirname "$0")
for arg in "$@"; do
  printf '%s\037' "$arg"
done > "$dir/argv"
printf '%s %s\n' "$1" "$2" >> "$dir/calls"
key="$1-$2"
src=""
if [ -f "$dir/out-$key" ]; then src="$dir/out-$key"; elif [ -f "$dir/out" ]; then src="$dir/out"; fi
if [ -n "$src" ]; then
  while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$src"
fi
src=""
if [ -f "$dir/err-$key" ]; then src="$dir/err-$key"; elif [ -f "$dir/err" ]; then src="$dir/err"; fi
if [ -n "$src" ]; then
  while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line" >&2; done < "$src"
fi
code=0
if [ -f "$dir/code-$key" ]; then read -r code < "$dir/code-$key"; elif [ -f "$dir/code" ]; then read -r code < "$dir/code"; fi
exit "$code"
"#;

/// The one `gh` file every test's fake links to, made once per test file.
fn shared_fake() -> &'static Path {
    static SHARED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("gear-forge-{}-gh", std::process::id()));
        executable(&path, FAKE_GH);
        path
    })
}

/// A fake `gh` that never answers: it burns time until it is killed.
const HANG: &str = r#"#!/bin/sh
while :; do :; done
"#;

/// Make `path` an executable file.
fn executable(path: &Path, text: &str) {
    fs::write(path, text).expect("the fake gh");
    let mut perms = fs::metadata(path).expect("the fake gh").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(path, perms).expect("an executable fake gh");
}

/// A scratch directory with an executable `gh` in it, and the forge that
/// looks for `gh` only there. The bound is wide on purpose: the 30 s the
/// forge ships with is a production setting, and a loaded machine would turn
/// it into a failure these tests say nothing about. A test that checks the
/// bound sets its own.
fn forge_with(tag: &str, script: Option<&str>) -> (PathBuf, GitHub) {
    let dir = std::env::temp_dir().join(format!("gear-forge-{}-{tag}", std::process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).expect("a scratch directory");
    match script {
        // A test's own script (the hanging fake): written where it is used.
        Some(text) => executable(&dir.join("gh"), text),
        // The ordinary fake: a link to the one script of this test file.
        None => fs::hard_link(shared_fake(), dir.join("gh")).expect("link the fake gh"),
    }
    let forge = GitHub::new()
        .with_path(&dir)
        .with_timeout(Duration::from_secs(300));
    (dir, forge)
}

/// What the fake was called with, last call only.
fn argv(dir: &Path) -> Vec<String> {
    let recorded = fs::read_to_string(dir.join("argv")).expect("the fake gh recorded nothing");
    recorded
        .split('\u{1f}')
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every call the fake was given, in order, as `<command> <subcommand>`.
fn calls(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// What the fake answers, for every call.
fn answer(dir: &Path, text: &str) {
    fs::write(dir.join("out"), text).expect("the answer");
}

/// What the fake answers one call (`out-<cmd>-<sub>`).
fn answer_for(dir: &Path, key: &str, text: &str) {
    fs::write(dir.join(format!("out-{key}")), text).expect("the answer");
}

/// What the fake exits with, for every call.
fn code(dir: &Path, code: i32) {
    fs::write(dir.join("code"), code.to_string()).expect("the exit code");
}

/// What the fake exits with, for one call (`code-<cmd>-<sub>`).
fn code_for(dir: &Path, key: &str, code: i32) {
    fs::write(dir.join(format!("code-{key}")), code.to_string()).expect("the exit code");
}

/// What the fake says on stderr, for one call (`err-<cmd>-<sub>`).
fn error_for(dir: &Path, key: &str, text: &str) {
    fs::write(dir.join(format!("err-{key}")), text).expect("the error text");
}

/// A pull request that is not a draft: the merge asks and goes on.
fn ready_pr(dir: &Path) {
    answer_for(dir, "pr-view", r#"{"isDraft":false}"#);
    answer_for(dir, "pr-merge", "✔ Merged pull request #7\n");
}

#[test]
fn issues_asks_for_json_and_reads_the_labels() {
    let (dir, forge) = forge_with("issues", None);
    answer(
        &dir,
        r#"[{"number":12,"title":"A bug","body":"the body","labels":[{"name":"bug"},{"name":"now"}]},
            {"number":13,"title":"An idea"}]"#,
    );
    let filter = Filter {
        labels: vec!["must".to_string()],
        limit: 5,
    };
    let issues = forge.issues("owner/name", &filter).expect("the issue list");
    assert_eq!(
        argv(&dir),
        vec![
            "issue",
            "list",
            "--repo",
            "owner/name",
            "--state",
            "open",
            "--json",
            "number,title,body,labels",
            "--limit",
            "5",
            "--label",
            "must",
        ]
    );
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].number, 12);
    assert_eq!(issues[0].title, "A bug");
    assert_eq!(issues[0].body, "the body");
    assert_eq!(issues[0].labels, vec!["bug", "now"]);
    // A missing body and missing labels are empty, not an error.
    assert_eq!(issues[1].number, 13);
    assert_eq!(issues[1].body, "");
    assert!(issues[1].labels.is_empty());
}

#[test]
fn open_claim_opens_a_draft_and_names_the_pull_request() {
    let (dir, forge) = forge_with("claim", None);
    // `gh pr create` prints the new pull request's URL, nothing else.
    answer(&dir, "https://github.com/owner/name/pull/7\n");
    let body = format!("Closes #12\n\n{}", marker::claim("base", 12));
    let pr = forge
        .open_claim("owner/name", "base/t78", "dev", "The forge layer (#12)", &body)
        .expect("the claim");
    assert_eq!(
        argv(&dir),
        vec![
            "pr",
            "create",
            "--repo",
            "owner/name",
            "--head",
            "base/t78",
            "--base",
            "dev",
            "--title",
            "The forge layer (#12)",
            "--body",
            &body,
            "--draft",
        ]
    );
    assert_eq!(
        pr,
        PrRef {
            repo: "owner/name".to_string(),
            number: 7,
            url: "https://github.com/owner/name/pull/7".to_string(),
        }
    );
    assert!(pr.is_open());
}

#[test]
fn comment_names_the_repo_and_the_number() {
    let (dir, forge) = forge_with("comment", None);
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    forge.comment(&pr, "handed in").expect("the comment");
    assert_eq!(
        argv(&dir),
        vec!["pr", "comment", "7", "--repo", "owner/name", "--body", "handed in"]
    );
}

#[test]
fn merge_takes_the_method() {
    let (dir, forge) = forge_with("merge", None);
    ready_pr(&dir);
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    let result = forge.merge(&pr, MergeMethod::Squash, "").expect("the merge");
    assert_eq!(
        argv(&dir),
        vec!["pr", "merge", "7", "--repo", "owner/name", "--squash"]
    );
    assert_eq!(calls(&dir), vec!["pr view", "pr merge"], "a ready pull request is not re-readied");
    assert!(result.merged, "{result:?}");
    assert!(result.message.contains("Merged pull request #7"), "{result:?}");

    let (dir, forge) = forge_with("merge-commit", None);
    ready_pr(&dir);
    forge.merge(&pr, MergeMethod::Merge, "").expect("the merge");
    assert_eq!(
        argv(&dir),
        vec!["pr", "merge", "7", "--repo", "owner/name", "--merge"]
    );
}

/// T-78, F1: a claim is opened as a draft, and GitHub refuses to merge a
/// draft. The ordinary path — claim, hand-in, verdict, merge — must need no
/// hand, so the merge turns the claim ready first.
#[test]
fn a_draft_claim_is_made_ready_before_it_merges() {
    let (dir, forge) = forge_with("draft", None);
    answer_for(&dir, "pr-create", "https://github.com/owner/name/pull/7\n");
    answer_for(&dir, "pr-view", r#"{"isDraft":true}"#);
    answer_for(&dir, "pr-ready", "");
    answer_for(&dir, "pr-merge", "✔ Merged pull request #7\n");

    let pr = forge
        .open_claim("owner/name", "base/t78", "dev", "The forge layer", "Closes #12")
        .expect("the claim");
    assert!(pr.is_open());
    let result = forge.merge(&pr, MergeMethod::Squash, "").expect("the merge");

    assert_eq!(
        calls(&dir),
        vec!["pr create", "pr view", "pr ready", "pr merge"],
        "a draft is turned ready before the merge"
    );
    assert_eq!(
        argv(&dir),
        vec!["pr", "merge", "7", "--repo", "owner/name", "--squash"]
    );
    assert!(result.merged, "{result:?}");
}

/// T-78, F1: readiness is asked, never assumed. A pull request that is already
/// ready (a retry, or one opened by hand) is not turned ready again.
#[test]
fn a_ready_pull_request_is_not_made_ready_again() {
    let (dir, forge) = forge_with("ready", None);
    ready_pr(&dir);
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    forge.merge(&pr, MergeMethod::Squash, "").expect("the merge");
    assert_eq!(calls(&dir), vec!["pr view", "pr merge"]);
}

/// T-78, F1: a readiness step that fails is a typed error, and nothing is
/// merged afterwards.
#[test]
fn a_failed_readiness_is_a_typed_error() {
    let (dir, forge) = forge_with("ready-failed", None);
    answer_for(&dir, "pr-view", r#"{"isDraft":true}"#);
    code_for(&dir, "pr-ready", 1);
    error_for(&dir, "pr-ready", "HTTP 403: not allowed\n");
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    match forge.merge(&pr, MergeMethod::Squash, "").unwrap_err() {
        Error::Failed(said) => {
            assert!(said.contains("pr ready"), "{said}");
            assert!(said.contains("not allowed"), "{said}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(calls(&dir), vec!["pr view", "pr ready"], "no merge after a failed readiness");
}

#[test]
fn close_issue_comments_and_closes() {
    let (dir, forge) = forge_with("close", None);
    forge
        .close_issue("owner/name", 12, "landed in dev")
        .expect("the close");
    assert_eq!(
        argv(&dir),
        vec![
            "issue",
            "close",
            "12",
            "--repo",
            "owner/name",
            "--comment",
            "landed in dev",
        ]
    );
}

#[test]
fn a_closed_pull_request_is_never_asked_about() {
    // Local's ref has no number: nothing to comment on, nothing to merge —
    // and no call is made at all.
    let (dir, forge) = forge_with("none", None);
    let none = PrRef::none("owner/name");
    forge.comment(&none, "x").expect("no call");
    let merged = forge.merge(&none, MergeMethod::Squash, "").expect("no call");
    assert!(!merged.merged, "{merged:?}");
    assert!(calls(&dir).is_empty(), "the forge was called: {:?}", calls(&dir));
}

#[test]
fn a_failing_gh_is_a_typed_error() {
    let (dir, forge) = forge_with("failed", None);
    fs::write(dir.join("err"), "HTTP 404: Not Found\n").expect("the error text");
    code(&dir, 1);
    let err = forge.issues("owner/name", &Filter::default()).unwrap_err();
    match err {
        Error::Failed(said) => {
            assert!(said.contains("HTTP 404"), "{said}");
            assert!(said.contains("issue list"), "{said}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn an_unreadable_answer_is_a_typed_error() {
    let (dir, forge) = forge_with("garbage", None);
    answer(&dir, "not json at all\n");
    match forge.issues("owner/name", &Filter::default()).unwrap_err() {
        Error::Unreadable(said) => assert!(said.contains("the issue list"), "{said}"),
        other => panic!("expected an unreadable answer, got {other:?}"),
    }
    match forge
        .open_claim("owner/name", "b", "dev", "t", "b")
        .unwrap_err()
    {
        Error::Unreadable(said) => assert!(said.contains("the opened pull request"), "{said}"),
        other => panic!("expected an unreadable answer, got {other:?}"),
    }
    // The draft state is read, not guessed: nonsense there is unreadable too.
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    match forge.merge(&pr, MergeMethod::Squash, "").unwrap_err() {
        Error::Unreadable(said) => assert!(said.contains("draft state"), "{said}"),
        other => panic!("expected an unreadable answer, got {other:?}"),
    }
}

#[test]
fn a_missing_tool_is_a_typed_error() {
    // A directory without a `gh`: the tool cannot be started.
    let (dir, _) = forge_with("missing", None);
    fs::remove_file(dir.join("gh")).expect("remove the fake");
    let forge = GitHub::new().with_path(&dir);
    match forge.issues("owner/name", &Filter::default()).unwrap_err() {
        Error::Tool(said) => assert!(said.contains("gh issue list"), "{said}"),
        other => panic!("expected a missing tool, got {other:?}"),
    }
}

#[test]
fn a_call_past_its_bound_is_a_timeout() {
    let (_dir, forge) = forge_with("hang", Some(HANG));
    let forge = forge.with_timeout(Duration::from_millis(250));
    let started = std::time::Instant::now();
    match forge.issues("owner/name", &Filter::default()).unwrap_err() {
        Error::Timeout(bound) => assert_eq!(bound, Duration::from_millis(250)),
        other => panic!("expected a timeout, got {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the bound was not respected: {:?}",
        started.elapsed()
    );
}

/// T-111: under `cargo test` there is no `gh` to forget. `.cargo/config.toml`
/// names a path that does not exist, so a `GitHub::new()` with no fake fails
/// with the harness's own reason — never with a call to the real `gh`.
#[test]
fn without_a_fake_no_test_reaches_a_real_gh() {
    let gh = std::env::var_os(gear_forge::github::GH_ENV)
        .expect("run the tests with cargo: .cargo/config.toml sets GEAR_GH");
    assert!(
        !Path::new(&gh).exists(),
        "GEAR_GH = {} exists: the tests must name a gh that does not run",
        Path::new(&gh).display()
    );
    let err = GitHub::new()
        .issues("o/r", &Filter::default())
        .expect_err("no gh means no call");
    let said = err.to_string();
    assert!(said.contains("no gh in tests"), "{said}");
    assert!(said.contains("GEAR_GH"), "{said}");
}

/// Design §11.2: the merge carries the head the gate checked, so GitHub
/// merges exactly that commit and never a head that landed in between.
#[test]
fn the_merge_passes_the_head_it_must_merge() {
    let (dir, forge) = forge_with("merge-head", None);
    ready_pr(&dir);
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    let result = forge
        .merge(&pr, MergeMethod::Squash, "abc1234")
        .expect("the merge");
    assert_eq!(
        argv(&dir),
        vec![
            "pr",
            "merge",
            "7",
            "--repo",
            "owner/name",
            "--squash",
            "--match-head-commit",
            "abc1234"
        ]
    );
    assert!(result.merged, "{result:?}");
}

/// Design §11.3: the pull request's own state is what an unknown merge asks
/// the forge.
#[test]
fn the_state_of_a_pull_request_is_read() {
    let (dir, forge) = forge_with("pr-state", None);
    // `gh` names the merge commit as an object.
    answer_for(
        &dir,
        "pr-view",
        r#"{"state":"MERGED","mergeCommit":{"oid":"sha99"},"headRefOid":"abc1234"}"#,
    );
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    let state = forge.pr_state(&pr).expect("the state");
    assert_eq!(state.state, "MERGED");
    assert!(state.is_merged(), "{state:?}");
    assert_eq!(state.merge_commit.as_deref(), Some("sha99"));
    assert_eq!(state.head_ref_oid.as_deref(), Some("abc1234"));
    assert_eq!(
        argv(&dir),
        vec![
            "pr",
            "view",
            "7",
            "--repo",
            "owner/name",
            "--json",
            "state,mergeCommit,headRefOid"
        ]
    );
}

/// Design §11.3: a pull request that is not merged yet names no commit.
#[test]
fn an_open_pull_request_names_no_merge_commit() {
    let (dir, forge) = forge_with("pr-state-open", None);
    answer_for(
        &dir,
        "pr-view",
        r#"{"state":"OPEN","mergeCommit":null,"headRefOid":"abc1234"}"#,
    );
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    let state = forge.pr_state(&pr).expect("the state");
    assert_eq!(state.state, "OPEN");
    assert!(state.is_open(), "{state:?}");
    assert_eq!(state.merge_commit, None, "no commit yet");
    assert_eq!(state.head_ref_oid.as_deref(), Some("abc1234"));
}

/// Design §11.3: an interrupted claim is adopted by its branch.
#[test]
fn the_open_pull_request_of_a_branch_is_found() {
    let (dir, forge) = forge_with("pr-by-head", None);
    answer_for(
        &dir,
        "pr-list",
        r#"[{"number":7,"url":"https://github.com/owner/name/pull/7"}]"#,
    );
    let pr = forge
        .pr_by_head("owner/name", "heat/t1")
        .expect("the pull request")
        .expect("there is one");
    assert_eq!(pr.number, 7);
    assert_eq!(pr.repo, "owner/name");
    assert_eq!(
        argv(&dir),
        vec![
            "pr",
            "list",
            "--repo",
            "owner/name",
            "--head",
            "heat/t1",
            "--json",
            "number,url"
        ]
    );

    // A branch with no pull request is none, not an error.
    let (dir, forge) = forge_with("pr-by-head-none", None);
    answer_for(&dir, "pr-list", "[]");
    assert!(
        forge
            .pr_by_head("owner/name", "heat/t2")
            .expect("the lookup")
            .is_none(),
        "there is none"
    );
}
