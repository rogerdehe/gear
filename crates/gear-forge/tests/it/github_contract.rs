//! Contract for [`GitHub`], the forge over the `gh` CLI (T-78).
//!
//! Every test runs a fake `gh` — a shell script in a scratch directory, put
//! on the child's `PATH` — that records the argv it was called with and
//! answers recorded JSON. That is what the engine's forge calls look like
//! from `gh`'s side: the command line, the parsing of the answer, and a typed
//! error when the tool is missing, fails, answers nonsense or runs too long.

use std::fs;
use std::path::{Path, PathBuf};

use gear_forge::{marker, Error, Filter, Forge, GitHub, MergeMethod, PrRef};

/// A fake `gh` for `dir`: it writes its argv to `argv` (each argument closed
/// by a `\037`, so an argument may hold newlines), prints `out`, prints `err`
/// on stderr and exits with `code`; every file is optional. Shell builtins
/// only — it runs with a `PATH` that holds nothing but itself.
fn fake_script(dir: &Path) -> String {
    let dir = dir.display();
    format!(
        r#"#!/bin/sh
dir={dir}
for arg in "$@"; do
  printf '%s\037' "$arg"
done > "$dir/argv"
if [ -f "$dir/out" ]; then
  while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$dir/out"
fi
if [ -f "$dir/err" ]; then
  while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line" >&2; done < "$dir/err"
fi
if [ -f "$dir/code" ]; then
  read -r code < "$dir/code"
  exit "$code"
fi
exit 0
"#
    )
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
/// looks for `gh` only there.
fn forge_with(tag: &str, script: Option<&str>) -> (PathBuf, GitHub) {
    let dir = std::env::temp_dir().join(format!("gear-forge-{}-{tag}", std::process::id()));
    fs::remove_dir_all(&dir).ok();
    fs::create_dir_all(&dir).expect("a scratch directory");
    match script {
        Some(text) => executable(&dir.join("gh"), text),
        None => executable(&dir.join("gh"), &fake_script(&dir)),
    }
    let forge = GitHub::new().with_path(&dir);
    (dir, forge)
}

/// What the fake was called with.
fn argv(dir: &Path) -> Vec<String> {
    let recorded = fs::read_to_string(dir.join("argv")).expect("the fake gh recorded nothing");
    recorded
        .split('\u{1f}')
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect()
}

/// What the fake answers on stdout.
fn answer(dir: &Path, text: &str) {
    fs::write(dir.join("out"), text).expect("the answer");
}

/// What the fake exits with.
fn code(dir: &Path, code: i32) {
    fs::write(dir.join("code"), code.to_string()).expect("the exit code");
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
    answer(&dir, "✔ Merged pull request #7\n");
    let pr = PrRef {
        repo: "owner/name".to_string(),
        number: 7,
        url: String::new(),
    };
    let result = forge.merge(&pr, MergeMethod::Squash).expect("the merge");
    assert_eq!(
        argv(&dir),
        vec!["pr", "merge", "7", "--repo", "owner/name", "--squash"]
    );
    assert!(result.merged, "{result:?}");
    assert!(result.message.contains("Merged pull request #7"), "{result:?}");

    let (dir, forge) = forge_with("merge-commit", None);
    forge.merge(&pr, MergeMethod::Merge).expect("the merge");
    assert_eq!(
        argv(&dir),
        vec!["pr", "merge", "7", "--repo", "owner/name", "--merge"]
    );
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
    let merged = forge.merge(&none, MergeMethod::Squash).expect("no call");
    assert!(!merged.merged, "{merged:?}");
    assert!(!dir.join("argv").exists(), "the forge was called");
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
    let forge = forge.with_timeout(std::time::Duration::from_millis(250));
    let started = std::time::Instant::now();
    match forge.issues("owner/name", &Filter::default()).unwrap_err() {
        Error::Timeout(bound) => assert_eq!(bound, std::time::Duration::from_millis(250)),
        other => panic!("expected a timeout, got {other:?}"),
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the bound was not respected: {:?}",
        started.elapsed()
    );
}
