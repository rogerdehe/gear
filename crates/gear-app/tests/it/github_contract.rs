//! T-79: the config and the plan carry the GitHub mode.
//!
//! - the config's `[github]` (project level and per repo) parses, resolves and
//!   is validated; absent = local-only;
//! - a task's `issue: #<n>` round-trips through apply and is kept when a later
//!   draft does not name it;
//! - `gear_app::github::issue_block` renders one valid `## NEW` block from an
//!   issue, with the priority the issue's labels name.

use gear_app::config;
use gear_app::draft::{parse, Priority};
use gear_app::github::issue_block;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

const BASE: &str = r#"
[project]
id = "toy"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"

[conventions]
# T-255: the gate has no default; every project names its own.
gate = "make test"
"#;

#[test]
fn a_project_github_section_parses_and_resolves_on_the_root_repo() {
    let text = format!(
        "{BASE}\n[github]\nrepo = \"rogerdehe/gear\"\nmerge = \"push\"\nmerge_method = \"merge\"\n\n[github.issues]\nlabels = [\"gear\", \"triage\"]\n"
    );
    let cfg = config::parse(&text).unwrap();
    assert!(
        config::validate(&cfg).is_empty(),
        "{:?}",
        config::validate(&cfg)
    );
    let root = cfg.root_repo();
    let github = root
        .github
        .expect("the project's mode reaches the root repo");
    assert_eq!(github.repo, "rogerdehe/gear");
    assert_eq!(
        github.issues_labels,
        vec!["gear".to_string(), "triage".to_string()]
    );
    assert_eq!(github.merge, config::MergeMode::Push);
    assert_eq!(github.merge_method, config::MergeMethod::Merge);
    // The root's own repo works the same way.
    assert_eq!(cfg.repo_of_lane("heat").github, Some(github));
}

#[test]
fn a_repo_github_section_replaces_the_projects_and_absent_means_local_only() {
    let text = format!(
        "{BASE}\n[github]\nrepo = \"rogerdehe/gear\"\n\n[[repos]]\nname = \"api\"\npath = \"services/api\"\n\n[repos.github]\nrepo = \"rogerdehe/api\"\nmerge_method = \"squash\"\n"
    );
    let cfg = config::parse(&text).unwrap();
    assert!(
        config::validate(&cfg).is_empty(),
        "{:?}",
        config::validate(&cfg)
    );
    let repos = cfg.repos();
    assert_eq!(
        repos[0].github.as_ref().map(|g| g.repo.as_str()),
        Some("rogerdehe/gear")
    );
    let api = repos.iter().find(|repo| repo.name == "api").unwrap();
    assert_eq!(
        api.github.as_ref().map(|g| g.repo.as_str()),
        Some("rogerdehe/api")
    );
    // A repo without its own section follows the project's, and the default
    // merge mode is `push` with `squash`.
    let api = api.github.as_ref().unwrap();
    assert_eq!(api.merge, config::MergeMode::Push);
    assert_eq!(api.merge_method, config::MergeMethod::Squash);

    // No `[github]` anywhere: local-only.
    let cfg = config::parse(BASE).unwrap();
    assert_eq!(cfg.root_repo().github, None);
    assert_eq!(cfg.repo_of_lane("heat").github, None);
}

#[test]
fn a_project_can_land_through_a_pull_request() {
    let cfg = config::parse(&format!(
        "{BASE}\n[github]\nrepo = \"a/b\"\nmerge = \"pr\"\n"
    ))
    .unwrap();
    assert!(
        config::validate(&cfg).is_empty(),
        "{:?}",
        config::validate(&cfg)
    );
    assert_eq!(cfg.root_repo().github.unwrap().merge, config::MergeMode::Pr);
}

#[test]
fn a_child_repo_can_land_through_a_pull_request() {
    let cfg = config::parse(&format!(
        "{BASE}\n[[repos]]\nname = \"api\"\npath = \"api\"\n\n[repos.github]\nrepo = \"a/b\"\nmerge = \"pr\"\n"
    )).unwrap();
    assert!(
        config::validate(&cfg).is_empty(),
        "{:?}",
        config::validate(&cfg)
    );
    let api = cfg
        .repos()
        .into_iter()
        .find(|repo| repo.name == "api")
        .unwrap();
    assert_eq!(api.github.unwrap().merge, config::MergeMode::Pr);
}

/// A child without its own GitHub section remains local-only.
#[test]
fn a_child_repo_without_a_section_is_local_only() {
    let text = format!(
        "{BASE}\n[github]\nrepo = \"root/project\"\nmerge = \"push\"\n\n[[repos]]\nname = \"child\"\npath = \"child\"\n\n[[repos]]\nname = \"api\"\npath = \"services/api\"\n\n[repos.github]\nrepo = \"root/api\"\n"
    );
    let cfg = config::parse(&text).unwrap();
    assert!(
        config::validate(&cfg).is_empty(),
        "{:?}",
        config::validate(&cfg)
    );

    // The root keeps the project's identity.
    let root = cfg.root_repo();
    let github = root.github.as_ref().expect("the root's mode");
    assert_eq!(github.repo, "root/project");
    assert_eq!(github.merge, config::MergeMode::Push);

    // The child has none — through both ways to reach it.
    let repos = cfg.repos();
    let child = repos.iter().find(|repo| repo.name == "child").unwrap();
    assert_eq!(
        child.github, None,
        "a child does not inherit the root's mode"
    );
    let lane = cfg.lanes.iter().find(|lane| lane.name == "heat").unwrap();
    assert!(lane.repo.is_none());
    // The configured child keeps its own identity.
    let api = repos.iter().find(|repo| repo.name == "api").unwrap();
    assert_eq!(
        api.github.as_ref().map(|g| g.repo.as_str()),
        Some("root/api")
    );
}

#[test]
fn a_github_section_without_a_repo_or_with_a_bad_repo_is_refused() {
    let no_repo = format!("{BASE}\n[github]\nmerge = \"push\"\n");
    let cfg = config::parse(&no_repo).unwrap();
    let issues = config::validate(&cfg);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].field, "github.repo");
    assert!(
        issues[0].problem.contains("owner/name"),
        "{}",
        issues[0].problem
    );

    for bad in [
        "https://github.com/rogerdehe/gear",
        "gear",
        "a/b/c",
        "owner /name",
    ] {
        let text = format!("{BASE}\n[github]\nrepo = \"{bad}\"\n");
        let cfg = config::parse(&text).unwrap();
        let issues = config::validate(&cfg);
        assert!(
            issues.iter().any(|issue| issue.field == "github.repo"),
            "{bad}: {issues:?}"
        );
    }

    // A repo's section is checked the same way, under its own field name.
    let text = format!(
        "{BASE}\n[[repos]]\nname = \"api\"\npath = \"api\"\n\n[repos.github]\nrepo = \"nope\"\n"
    );
    let cfg = config::parse(&text).unwrap();
    let issues = config::validate(&cfg);
    assert!(
        issues
            .iter()
            .any(|issue| issue.field == "repos[0].github.repo"),
        "{issues:?}"
    );
}

#[test]
fn an_unknown_merge_mode_or_method_is_a_parse_error() {
    for bad in [
        "[github]\nrepo = \"a/b\"\nmerge = \"fast-forward\"\n",
        "[github]\nrepo = \"a/b\"\nmerge_method = \"rebase\"\n",
    ] {
        let text = format!("{BASE}\n{bad}");
        assert_eq!(
            config::parse(&text).unwrap_err().code,
            ErrorCode::BadFormat,
            "{bad}"
        );
    }
}

#[test]
fn a_blank_issue_label_is_refused() {
    let text = format!(
        "{BASE}\n[github]\nrepo = \"a/b\"\n\n[github.issues]\nlabels = [\"gear\", \"  \"]\n"
    );
    let cfg = config::parse(&text).unwrap();
    let issues = config::validate(&cfg);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].field, "github.issues.labels[1]");
}

#[test]
fn the_priority_labels_name_the_queues_priorities() {
    for (label, priority) in [
        ("priority/now", Priority::Now),
        ("priority/must", Priority::Must),
        ("priority/should", Priority::Should),
        ("priority/could", Priority::Could),
        ("priority/wont", Priority::Wont),
    ] {
        assert_eq!(Priority::from_label(label), Some(priority), "{label}");
        assert_eq!(priority.label(), label);
    }
    assert_eq!(Priority::from_label("bug"), None);
    assert_eq!(Priority::from_label("priority/urgent"), None);
}

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        testing::seed_project(tx, "p", "sess", NOW)?;
        testing::seed_active_seat(
            tx,
            "p",
            "captain",
            SeatRole::Captain,
            CAP_PANE,
            "codex",
            NOW,
        )?;
        testing::seed_active_seat(tx, "p", "heat", SeatRole::Lane, "w1:p2", "opencode", NOW)?;
        Ok(())
    })
    .unwrap();
    s
}

fn cap() -> (Env, Fingerprint) {
    (
        Env {
            ctx: Some(GearCtx {
                project: "p".into(),
                seat: "captain".into(),
                generation: 1,
            }),
            pane_id: Some(CAP_PANE.into()),
            admin: false,
        },
        Fingerprint {
            pane_id: CAP_PANE.into(),
            kind: Some("codex".into()),
            session_ref: None,
        },
    )
}

fn run(
    s: &mut Store,
    queue: &str,
    base: i64,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let text = format!("<!-- gear-plan project=p base={base} -->\n# Queue\n{queue}");
    let (env, fp) = cap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(&text).unwrap(),
        &text,
        rid,
        NOW,
    )
}

fn issue_of(s: &mut Store, task: &str) -> Option<i64> {
    s.read(|tx| tx.get_task("p", task)).unwrap().unwrap().issue
}

#[test]
fn an_issue_round_trips_through_apply_and_is_kept_when_a_draft_omits_it() {
    let mut s = world();
    let r = run(
        &mut s,
        "## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nissue: #12\n",
        0,
        "v1",
    )
    .unwrap();
    assert_eq!(r.created, vec!["T-1".to_string()]);
    assert_eq!(issue_of(&mut s, "T-1"), Some(12));

    // The next draft does not name the issue: the task keeps it.
    run(
        &mut s,
        "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n",
        1,
        "v2",
    )
    .unwrap();
    assert_eq!(issue_of(&mut s, "T-1"), Some(12), "kept across applies");

    // A draft that names another issue moves it.
    run(
        &mut s,
        "## T-1 [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nissue: #34\n",
        2,
        "v3",
    )
    .unwrap();
    assert_eq!(issue_of(&mut s, "T-1"), Some(34));
}

#[test]
fn an_issue_that_is_not_hash_number_is_refused_at_its_line() {
    for bad in [
        "12",
        "#x",
        "#-1",
        "#",
        "#0",
        "#+12",
        "#9223372036854775808",
        "#18446744073709551615",
    ] {
        let text = format!(
            "<!-- gear-plan project=toy base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\nissue: {bad}\n"
        );
        let error = parse(&text).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadFormat, "{bad}");
        assert!(
            error.detail.starts_with("line 8:"),
            "{bad}: {}",
            error.detail
        );
        assert!(error.detail.contains("issue"), "{bad}: {}", error.detail);
    }
}

fn issue(number: u64, title: &str, body: &str, labels: &[&str]) -> gear_forge::Issue {
    gear_forge::Issue {
        number,
        title: title.to_string(),
        body: body.to_string(),
        labels: labels.iter().map(|label| label.to_string()).collect(),
    }
}

#[test]
fn issue_block_renders_a_valid_new_block() {
    let rendered = issue_block(&issue(
        42,
        "The board shows the lane   twice",
        "When two lanes share a name the board lists it twice.\n\nSteps: …",
        &["bug", "priority/should"],
    ));

    // It parses as a plan queue block, with the issue's facts.
    let draft = parse(&format!(
        "<!-- gear-plan project=toy base=0 -->\n# Queue\n{rendered}"
    ))
    .unwrap();
    assert_eq!(draft.queue.len(), 1, "{rendered}");
    let item = &draft.queue[0];
    assert_eq!(item.id, None, "a NEW block");
    assert_eq!(
        item.priority,
        Priority::Should,
        "the label names the priority"
    );
    assert_eq!(
        item.goal.as_deref(),
        Some("The board shows the lane twice"),
        "the title, on one line"
    );
    assert_eq!(
        item.accept.as_deref(),
        Some("When two lanes share a name the board lists it twice."),
        "the body's first line"
    );
    assert_eq!(item.issue, Some(42), "the link to the issue");
    assert_eq!(item.estimate, Some(30), "the captain's to change");
    assert!(item.deps.is_empty());

    // The rendered text says all of it the way the format wants it.
    assert!(rendered.starts_with("## NEW [should] lane="), "{rendered}");
    assert!(rendered.contains("\nissue: #42\n"), "{rendered}");
}

#[test]
fn issue_block_defaults_the_priority_and_handles_an_empty_body() {
    let rendered = issue_block(&issue(7, "Do the thing", "", &["bug"]));
    let draft = parse(&format!(
        "<!-- gear-plan project=toy base=0 -->\n# Queue\n{rendered}"
    ))
    .unwrap();
    let item = &draft.queue[0];
    assert_eq!(item.priority, Priority::Must, "no priority label: must");
    assert_eq!(
        item.accept.as_deref(),
        Some("the request of issue #7 is met"),
        "{rendered}"
    );
    assert_eq!(item.issue, Some(7));
}
