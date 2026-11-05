//! The `review-model:` of a task (T-500): the captain sends one task to a
//! review entry of their choice. The field names a model of `[models] review`
//! and differs from the task's lane model; a block that names it sets it, a
//! block that names none keeps it, and `gear plan show` prints it back.

use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx, SeatRole};
use gear_store::{testing, Store};

const NOW: i64 = 1_800_000_000;
const CAP_PANE: &str = "w1:p1";

/// The lane's primary model is `review-a`; the review chain offers `review-a`
/// (the same model, for the equal-model check), `review-b` and `review-c`.
const CONFIG: &str = r#"
[project]
id = "p"
herdr_session = "sess"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w1:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "wire"
duty = "price engine"
owns = ["src/wire/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "codex"
profile = "codex"

[models]
lanes = ["review-a"]
review = [
  { model = "review-a", cli = "codex", args = ["-c", "model_reasoning_effort=high"] },
  { model = "review-b", cli = "claude", args = ["--effort", "high"] },
  { model = "review-c", cli = "codex", args = ["-c", "model_reasoning_effort=low"] },
]

[conventions]
integration = "dev"
release = "main"
gate = "make test"
"#;

fn world() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.write(|tx| {
        let mut project = testing::seed_project(tx, "p", "sess", NOW)?;
        project.config_toml = CONFIG.to_string();
        tx.update_project(&project)?;
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
        testing::seed_active_seat(tx, "p", "wire", SeatRole::Lane, "w1:p3", "opencode", NOW)?;
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
    text: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let (env, fp) = cap();
    apply(
        s,
        &env,
        Some(&fp),
        None,
        &parse(text).unwrap(),
        text,
        rid,
        NOW + 10,
    )
}

fn run_at(
    s: &mut Store,
    text: &str,
    rid: &str,
) -> gear_model::GearResult<gear_app::plan::ApplyResult> {
    let base = s.read(|tx| tx.plan_version("p")).unwrap();
    run(
        s,
        &format!("<!-- gear-plan project=p base={base} -->\n{text}"),
        rid,
    )
}

/// T-500: a NEW block's `review-model:` is stored and shown back; an existing
/// task's block sets it, and a block without the field keeps it.
#[test]
fn a_review_model_is_stored_shown_and_kept() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nreview-model: review-b\ndeps: -\n",
        "r1",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.task_review_model("p", "T-1"))
            .unwrap()
            .as_deref(),
        Some("review-b")
    );
    let shown = s.read(|tx| gear_app::plan::show(tx, "p", "T-1")).unwrap();
    assert!(shown.contains("review-model: review-b"), "{shown}");

    // A block without `review-model:` keeps the model the task has.
    run_at(&mut s, "# Queue\n## T-1 [must] lane=heat\ndeps: -\n", "r2").unwrap();
    assert_eq!(
        s.read(|tx| tx.task_review_model("p", "T-1"))
            .unwrap()
            .as_deref(),
        Some("review-b")
    );

    // A block that names another entry sets it.
    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\nreview-model: review-b\ndeps: -\n",
        "r3",
    )
    .unwrap();
    assert_eq!(
        s.read(|tx| tx.task_review_model("p", "T-1"))
            .unwrap()
            .as_deref(),
        Some("review-b")
    );
}

/// T-500: `review-model:` must name a model of `[models] review`.
#[test]
fn a_review_model_outside_the_review_chain_is_refused() {
    let mut s = world();
    let error = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nreview-model: review-z\ndeps: -\n",
        "r1",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("review-a, review-b, review-c"),
        "the entry list: {}",
        error.detail
    );
}

/// T-500: `review-model:` must differ from the task's lane model.
#[test]
fn a_review_model_equal_to_the_lanes_model_is_refused() {
    let mut s = world();
    let error = run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nreview-model: review-a\ndeps: -\n",
        "r1",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("is the model of lane heat"),
        "{}",
        error.detail
    );
}

/// T-500: the field needs a value.
#[test]
fn an_empty_review_model_is_refused() {
    let error = parse(
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\nreview-model:\ndeps: -\n",
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BadFormat, "{}", error.detail);
    assert!(
        error.detail.contains("`review-model` needs the model"),
        "{}",
        error.detail
    );
}

/// `carried-by: T-m` (Lawrence T-31, 2026-11-03): stored, shown back, kept by
/// a block without it, cleared by `-`; a task that does not exist, or the task
/// itself, is refused.
#[test]
fn carried_by_is_stored_shown_cleared_and_checked() {
    let mut s = world();
    run(
        &mut s,
        "<!-- gear-plan project=p base=0 -->\n# Queue\n## NEW [must] lane=heat\ngoal: A\naccept: a\nestimate: 30\ndeps: -\n\n## NEW [must] lane=heat\ngoal: B\naccept: b\nestimate: 30\ndeps: -\n",
        "r1",
    )
    .unwrap();
    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\ncarried-by: T-2\ndeps: -\n",
        "r2",
    )
    .unwrap();
    let by = |s: &mut Store| s.read(|tx| tx.task_carried_by("p", "T-1")).unwrap();
    assert_eq!(by(&mut s).as_deref(), Some("T-2"));
    let shown = s.read(|tx| gear_app::plan::show(tx, "p", "T-1")).unwrap();
    assert!(shown.contains("carried-by: T-2"), "{shown}");

    run_at(&mut s, "# Queue\n## T-1 [must] lane=heat\ndeps: -\n", "r3").unwrap();
    assert_eq!(
        by(&mut s).as_deref(),
        Some("T-2"),
        "a block without it keeps it"
    );

    run_at(
        &mut s,
        "# Queue\n## T-1 [must] lane=heat\ncarried-by: -\ndeps: -\n",
        "r4",
    )
    .unwrap();
    assert_eq!(by(&mut s), None, "`-` clears it");

    let bad = parse("<!-- gear-plan project=p base=0 -->\n# Queue\n## T-1 [must] lane=heat\ncarried-by: soon\ndeps: -\n")
        .unwrap_err();
    assert_eq!(bad.code, ErrorCode::BadFormat, "{}", bad.detail);
    for (value, rid) in [("T-9", "r5"), ("T-1", "r6")] {
        let error = run_at(
            &mut s,
            &format!("# Queue\n## T-1 [must] lane=heat\ncarried-by: {value}\ndeps: -\n"),
            rid,
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            ErrorCode::BadFormat,
            "{value}: {}",
            error.detail
        );
    }
}
