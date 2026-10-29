//! Contract for `gear plan import-issues` (T-81): the open issues a
//! GitHub-mode project has not turned into tasks yet, as `## NEW` blocks for the
//! captain to paste into the voyage file.
//!
//! A fake `gh` on the child's `PATH` answers the issue list; the store holds the
//! project's `[github]` mode and the tasks that already name an issue.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gear_model::SeatRole;
use gear_store::{testing, Store, TaskRow};

const T0: i64 = 1_800_000_000;

/// A fake `gh` for `dir`: it records its argv and prints `out`.
fn fake_gh(dir: &Path, out: &str) {
    std::fs::create_dir_all(dir).expect("the fake dir");
    std::fs::write(dir.join("out"), out).expect("the answer");
    let script = format!(
        r#"#!/bin/sh
dir={dir}
for arg in "$@"; do
  printf '%s\037' "$arg"
done > "$dir/argv"
while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$dir/out"
exit 0
"#,
        dir = dir.display()
    );
    let gh = dir.join("gh");
    std::fs::write(&gh, script).expect("the fake gh");
    let mut perms = std::fs::metadata(&gh).expect("the fake gh").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&gh, perms).expect("an executable fake gh");
}

/// The argv the fake was given.
fn argv(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("argv"))
        .unwrap_or_default()
        .split('\u{1f}')
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect()
}

/// A store with project `p` in GitHub mode, its captain and one lane, and task
/// `T-1` already carrying `issue: #12`.
fn store_with_plan(db: &Path, github: bool) -> Store {
    let mut store = Store::open(db).expect("the store");
    store
        .write(|tx| {
            testing::seed_project(tx, "p", "sess", T0)?;
            testing::seed_active_seat(tx, "p", "captain", SeatRole::Captain, "w1:p1", "codex", T0)?;
            testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", T0)?;
            let mut project = tx.get_project("p")?.expect("the project");
            let base = "[project]\nid = \"p\"\nherdr_session = \"sess\"\nrepo = \"/tmp/gear-issues\"\n\
                        \n[quota]\nlanes = 1\ntemps = 0\n\
                        \n[captain]\npane = \"w1:p1\"\nkind = \"codex\"\nprofile = \"codex\"\n\
                        \n[conventions]\nintegration = \"dev\"\n";
            project.config_toml = if github {
                format!(
                    "{base}\
                     \n[github]\nrepo = \"owner/name\"\n\
                     \n[github.issues]\nlabels = [\"gear\"]\n"
                )
            } else {
                base.to_string()
            };
            tx.update_project(&project)?;
            tx.insert_task(&TaskRow {
                repo: None,
                base: None,
                gate: None,
                rework_estimate_min: None,
                project: "p".into(),
                id: "T-1".into(),
                rank: 1,
                state: gear_model::TaskState::Running,
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
                updated: T0,
                issue: Some(12),
            })?;
            Ok(())
        })
        .expect("the world");
    store
}

fn gear(home: &Path, db: &Path, path: Option<&Path>, args: &[&str], path_env: &Path) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    cmd.args(["plan", "import-issues"]);
    if let Some(path) = path {
        cmd.arg(path);
    }
    cmd.args(args)
        .env("HOME", home)
        .env("GEAR_DB", db)
        .env("GEAR_CTX", "p:captain:1")
        .env("PATH", path_env)
        .env("GEAR_GH", path_env.join("gh"))
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .output()
        .expect("run gear")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn scratch(tag: &str) -> PathBuf {
    crate::scratch::temp(&format!("gear-issues-{tag}"))
}

/// The JSON `gh issue list --json …` answers: #12 is in the plan, #13 and #14
/// are new.
const ISSUES: &str = r#"[
{"number":12,"title":"Already planned","body":"it is in the plan","labels":[{"name":"gear"}]},
{"number":13,"title":"A bug in the gate","body":"the gate is red on main\nsteps to reproduce","labels":[{"name":"gear"},{"name":"priority/should"}]},
{"number":14,"title":"A new idea","body":"","labels":[]}
]"#;


/// A fake `gh` that behaves like the real one: without `--limit` it answers the
/// default first page, with it every issue. The argv is still recorded.
fn fake_gh_paged(dir: &Path, all: &str, page: &str) {
    std::fs::create_dir_all(dir).expect("the fake dir");
    std::fs::write(dir.join("all"), all).expect("the full answer");
    std::fs::write(dir.join("page"), page).expect("the default page");
    let script = format!(
        r#"#!/bin/sh
dir={dir}
for arg in "$@"; do
  printf '%s\037' "$arg"
done > "$dir/argv"
limited=0
for arg in "$@"; do
  if [ "$arg" = "--limit" ]; then limited=1; fi
done
if [ "$limited" = "1" ]; then src="$dir/all"; else src="$dir/page"; fi
while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$src"
exit 0
"#,
        dir = dir.display()
    );
    let gh = dir.join("gh");
    std::fs::write(&gh, script).expect("the fake gh");
    let mut perms = std::fs::metadata(&gh).expect("the fake gh").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&gh, perms).expect("an executable fake gh");
}

/// One JSON array of issues, `numbers` in the order given.
fn issues_json(numbers: &[u32]) -> String {
    let items: Vec<String> = numbers
        .iter()
        .map(|n| {
            format!(
                "{{\"number\":{n},\"title\":\"issue {n}\",\"body\":\"\",\"labels\":[]}}"
            )
        })
        .collect();
    format!("[{}]", items.join(",\n"))
}

/// T-81, F1: the import must not silently stop at `gh`'s default first page.
/// With 31 open issues, the newest 30 already planned and #1 still unplanned,
/// #1 has to be printed — an older issue is exactly what a long-lived plan is
/// missing.
#[test]
fn an_issue_older_than_the_first_page_is_still_imported() {
    let dir = scratch("page");
    let db = dir.join("gear.db");
    drop(store_with_plan(&db, true));
    // The newest 30 are #31..#2; #1 is older than the default page.
    let all: Vec<u32> = (1..=31).rev().collect();
    let page: Vec<u32> = (2..=31).rev().collect();
    fake_gh_paged(&dir.join("bin"), &issues_json(&all), &issues_json(&page));
    // The voyage file already names the newest 30.
    let voyage = dir.join("M2.md");
    let mut planned = String::from("# Queue\n");
    for n in (2..=31).rev() {
        planned.push_str(&format!("## T-{n} [must] lane=heat milestone=M2\ngoal: x\nissue: #{n}\n\n"));
    }
    std::fs::write(&voyage, planned).expect("the voyage file");

    let o = gear(&dir, &db, Some(&voyage), &[], &dir.join("bin"));

    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = stdout(&o);
    assert!(out.contains("issue: #1"), "the older unplanned issue is missing:\n{out}");
    assert!(out.contains("goal: issue 1"), "{out}");
    assert!(!out.contains("issue: #2\n"), "a planned issue came back:\n{out}");
    // The query asked for everything, not the default page.
    let args = argv(&dir.join("bin"));
    assert!(args.windows(2).any(|pair| pair[0] == "--limit"), "{args:?}");
}

#[test]
fn it_prints_blocks_for_the_issues_the_plan_does_not_carry() {
    let dir = scratch("new");
    let db = dir.join("gear.db");
    drop(store_with_plan(&db, true));
    fake_gh(&dir.join("bin"), ISSUES);

    let o = gear(&dir, &db, None, &[], &dir.join("bin"));

    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = stdout(&o);
    assert!(!out.contains("Already planned"), "issue #12 is in the plan:\n{out}");
    assert!(out.contains("goal: A bug in the gate"), "{out}");
    assert!(out.contains("issue: #13"), "{out}");
    assert!(out.contains("## NEW [should] lane=<lane> milestone=<M>"), "{out}");
    assert!(out.contains("accept: the gate is red on main"), "{out}");
    assert!(out.contains("goal: A new idea"), "{out}");
    assert!(out.contains("issue: #14"), "{out}");
    // The config's own label narrowed the query, with the repo named.
    let args = argv(&dir.join("bin"));
    assert_eq!(args.first().map(String::as_str), Some("issue"), "{args:?}");
    assert!(args.windows(2).any(|pair| pair == ["--repo", "owner/name"]), "{args:?}");
    assert!(args.windows(2).any(|pair| pair == ["--label", "gear"]), "{args:?}");
}

#[test]
fn a_label_flag_narrows_the_query_and_a_voyage_file_narrows_the_plan() {
    let dir = scratch("label");
    let db = dir.join("gear.db");
    drop(store_with_plan(&db, true));
    fake_gh(&dir.join("bin"), ISSUES);
    // The plan being edited already names #13.
    let voyage = dir.join("M1.md");
    std::fs::write(
        &voyage,
        "# Queue\n## T-1 [must] lane=heat milestone=M1\ngoal: x\nissue: #12\n\n## NEW\nissue: #13\n",
    )
    .expect("the voyage file");

    let o = gear(&dir, &db, Some(&voyage), &["--label", "bug"], &dir.join("bin"));

    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let out = stdout(&o);
    assert!(!out.contains("issue: #12"), "{out}");
    assert!(!out.contains("issue: #13"), "the file names it:\n{out}");
    assert!(out.contains("issue: #14"), "{out}");
    let args = argv(&dir.join("bin"));
    assert!(args.windows(2).any(|pair| pair == ["--label", "gear"]), "{args:?}");
    assert!(args.windows(2).any(|pair| pair == ["--label", "bug"]), "{args:?}");
}

#[test]
fn a_project_without_github_mode_says_so() {
    let dir = scratch("local");
    let db = dir.join("gear.db");
    drop(store_with_plan(&db, false));
    fake_gh(&dir.join("bin"), ISSUES);

    let o = gear(&dir, &db, None, &[], &dir.join("bin"));

    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("not in GitHub mode"), "{err}");
    assert!(argv(&dir.join("bin")).is_empty(), "the forge was called");
}

#[test]
fn a_lane_runs_no_import() {
    let dir = scratch("lane");
    let db = dir.join("gear.db");
    drop(store_with_plan(&db, true));
    fake_gh(&dir.join("bin"), ISSUES);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gear"));
    let o = cmd
        .args(["plan", "import-issues"])
        .env("HOME", &dir)
        .env("GEAR_DB", &db)
        .env("GEAR_CTX", "p:heat:1")
        .env("PATH", dir.join("bin"))
        .env("GEAR_GH", dir.join("bin").join("gh"))
        .env_remove("HERDR_PANE_ID")
        .output()
        .expect("run gear");
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("the captain's"), "{err}");
    assert!(argv(&dir.join("bin")).is_empty(), "the forge was called");
}
