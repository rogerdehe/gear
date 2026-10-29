//! Contract for a departed voyage's staffing (T-392): `[quota]`, `[[lanes]]`,
//! `[temps]` and `[models]` are agreed at its kickoff, like the goal (T-331).
//! While the voyage is under way, a re-init that changes one of them is
//! refused with `StaffLocked` and the changed fields, and writes nothing;
//! with the user's own words (`--user`) it applies and leaves one starred
//! `staff` decision in the voyage. Anything outside those four sections, and
//! any change of a project with no voyage under way, applies as before.

use gear_app::admin::init_with_home;
use gear_app::draft::parse;
use gear_app::identity::Fingerprint;
use gear_app::plan::apply;
use gear_app::Env;
use gear_model::{ErrorCode, GearCtx};
use gear_store::Store;

const T0: i64 = 1_800_000_000;
const CAP_PANE: &str = "w2:p1";

/// A config for `harbor`: one lane, a reviewer quota of `temps`, and the
/// merge message `merge_message`.
fn config(temps: u32, merge_message: &str) -> String {
    format!(
        r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 3
temps = {temps}

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
duty = "thermal calculation"
owns = ["src/heat/"]
kind = "opencode"
profile = "opencode"

[temps]
kind = "opencode"
profile = "opencode"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = ["gpt-6.1-sol"]
on_quota = {{ lanes = "wait", review = "wait" }}

[conventions]
gate = "make test"
merge_message = "{merge_message}"
"#
    )
}

fn admin() -> Env {
    Env { ctx: None, pane_id: None, admin: true }
}

/// The captain's plan with one milestone and one open task on it: the voyage
/// M1 departs, and it is still under way.
fn depart(s: &mut Store) {
    let text = "<!-- gear-plan project=harbor base=0 -->\n\
                # Milestone M1: delivery v0.1.0\n\
                done: users install it\n\
                bar: block only loop breaks\n\
                eta: 8h\n\
                # Queue\n\
                ## NEW [must] lane=heat milestone=M1\n\
                goal: G\naccept: a\nestimate: 30\ndeps: -\n";
    let draft = parse(text).unwrap();
    let env = Env {
        ctx: Some(GearCtx { project: "harbor".into(), seat: "captain".into(), generation: 1 }),
        pane_id: Some(CAP_PANE.into()),
        admin: false,
    };
    let fp = Fingerprint { pane_id: CAP_PANE.into(), kind: Some("codex".into()), session_ref: None };
    apply(s, &env, Some(&fp), None, &draft, text, "r1", T0 + 1).unwrap();
}

/// A registered project whose voyage M1 has departed with an open task.
fn departed() -> Store {
    departed_with(&config(6, "merge({task}): {source}"))
}

/// The same, from `config_text` instead of the helper's own config.
fn departed_with(config_text: &str) -> Store {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), config_text, Some(crate::skill_home()), None, T0).unwrap();
    depart(&mut s);
    let voyage = s.read(|tx| tx.get_voyage("harbor", "M1")).unwrap().unwrap();
    assert_eq!(voyage.departure, Some(T0 + 1), "the voyage departed");
    s
}

/// `config(temps, merge_message)` with the lane models as entries: the same
/// model, the CLI and a reasoning effort (T-392 rework F2).
fn config_with_effort(effort: &str, context_window_k: Option<u64>) -> String {
    let window = match context_window_k {
        Some(k) => format!(", context_window_k = {k}"),
        None => String::new(),
    };
    config(6, "merge({task}): {source}").replace(
        "lanes = [\"minimax-cn/MiniMax-M3.1-Flash\"]",
        &format!(
            "lanes = [{{ model = \"minimax-cn/MiniMax-M3.1-Flash\", cli = \"opencode\", args = [\"-c\", \"model_reasoning_effort={effort}\"]{window} }}]"
        ),
    )
}

/// The config version the store holds.
fn version(s: &mut Store) -> i64 {
    s.read(|tx| tx.get_project("harbor")).unwrap().unwrap().config_version
}

/// The starred `staff` decisions of voyage M1.
fn staff_decisions(s: &mut Store) -> Vec<gear_store::DecisionRow> {
    s.read(|tx| tx.decisions_of("harbor", "M1"))
        .unwrap()
        .into_iter()
        .filter(|decision| decision.kind == "staff" && decision.star)
        .collect()
}

/// A change to `[quota] temps` while the voyage is under way is refused with
/// `StaffLocked`, names the field, and writes nothing.
#[test]
fn a_staffing_change_mid_voyage_is_refused_without_the_users_words() {
    let mut s = departed();
    let stored = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();

    let changed = config(3, "merge({task}): {source}");
    let err = init_with_home(&mut s, &admin(), &changed, Some(crate::skill_home()), None, T0 + 2)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaffLocked);
    assert!(
        err.detail.contains("quota.temps: 6 → 3"),
        "the changed field is named: {}",
        err.detail
    );
    assert!(
        err.detail.contains("the staffing of voyage M1 was agreed at its kickoff"),
        "{}",
        err.detail
    );
    assert!(
        err.detail.contains("--user"),
        "the refusal says how to apply it: {}",
        err.detail
    );

    // Nothing was written: the config, its version and the decisions are as
    // they were.
    let after = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(after.config_version, stored.config_version);
    assert_eq!(after.config_toml, stored.config_toml);
    assert!(staff_decisions(&mut s).is_empty());
}

/// With the user's own words the change applies, and the voyage keeps one
/// starred `staff` decision whose why holds them.
#[test]
fn the_users_words_apply_the_change_and_leave_one_starred_decision() {
    let mut s = departed();
    let before = version(&mut s);

    let changed = config(3, "merge({task}): {source}");
    let r = init_with_home(
        &mut s,
        &admin(),
        &changed,
        Some(crate::skill_home()),
        Some("lower the reviewers to three, I asked for it"),
        T0 + 2,
    )
    .unwrap();
    assert_eq!(r.config_version, before + 1);
    assert_eq!(version(&mut s), before + 1);

    let decisions = staff_decisions(&mut s);
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    let decision = &decisions[0];
    assert_eq!(decision.voyage, "M1");
    assert_eq!(decision.what, "quota.temps: 6 → 3");
    let why = decision.why.as_deref().unwrap_or_default();
    assert!(
        why.contains("lower the reviewers to three, I asked for it"),
        "the user's words are kept: {why}"
    );
}

/// A change outside the four staffing sections applies without the user's
/// words, even mid-voyage.
#[test]
fn a_change_outside_the_staffing_sections_applies_without_words() {
    let mut s = departed();
    let before = version(&mut s);

    let changed = config(6, "merged({task}): {source}");
    let r = init_with_home(&mut s, &admin(), &changed, Some(crate::skill_home()), None, T0 + 2)
        .unwrap();
    assert_eq!(r.config_version, before + 1);
    assert!(staff_decisions(&mut s).is_empty());
}

/// A project with no voyage under way takes a staffing change as before.
#[test]
fn without_a_departed_voyage_every_change_applies_as_before() {
    let mut s = Store::open_in_memory().unwrap();
    init_with_home(&mut s, &admin(), &config(6, "merge({task}): {source}"), Some(crate::skill_home()), None, T0)
        .unwrap();

    let changed = config(3, "merge({task}): {source}");
    let r = init_with_home(&mut s, &admin(), &changed, Some(crate::skill_home()), None, T0 + 1)
        .unwrap();
    assert_eq!(r.config_version, 2);
    assert_eq!(
        s.read(|tx| tx.get_project("harbor")).unwrap().unwrap().quota_temps,
        3
    );
    assert!(staff_decisions(&mut s).is_empty());
}

/// Rework F1: `--user ""` and `--user "   "` carry no words, so the refusal
/// stands, nothing is written and no staff decision is recorded.
#[test]
fn blank_user_words_are_no_words() {
    let mut s = departed();
    let stored = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    let changed = config(3, "merge({task}): {source}");

    for blank in ["", "   ", "\t\n "] {
        let err = init_with_home(
            &mut s,
            &admin(),
            &changed,
            Some(crate::skill_home()),
            Some(blank),
            T0 + 2,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::StaffLocked, "for {blank:?}: {}", err.detail);
        assert!(
            err.detail.contains("quota.temps: 6 → 3"),
            "for {blank:?}: {}",
            err.detail
        );
    }

    let after = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(after.config_version, stored.config_version);
    assert_eq!(after.config_toml, stored.config_toml);
    assert!(staff_decisions(&mut s).is_empty(), "no staff decision");
}

/// Rework F2: a changed model argument is a changed field — the refusal and
/// the starred decision name the argument, and the context window when the
/// entry sets it.
#[test]
fn a_changed_model_argument_is_named_in_the_refusal_and_the_decision() {
    let mut s = departed_with(&config_with_effort("medium", Some(200)));
    let before = version(&mut s);

    let changed = config_with_effort("high", Some(300));
    let err =
        init_with_home(&mut s, &admin(), &changed, Some(crate::skill_home()), None, T0 + 2)
            .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaffLocked);
    for expected in [
        "models.lanes:",
        "model_reasoning_effort=medium",
        "model_reasoning_effort=high",
        "context_window_k 200",
        "context_window_k 300",
    ] {
        assert!(
            err.detail.contains(expected),
            "the refusal names {expected:?}: {}",
            err.detail
        );
    }
    assert_eq!(version(&mut s), before, "the refusal wrote nothing");

    let r = init_with_home(
        &mut s,
        &admin(),
        &changed,
        Some(crate::skill_home()),
        Some("raise the lane's effort, the user said so"),
        T0 + 2,
    )
    .unwrap();
    assert_eq!(r.config_version, before + 1);
    let decisions = staff_decisions(&mut s);
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    let what = &decisions[0].what;
    assert!(
        what.contains("model_reasoning_effort=medium")
            && what.contains("model_reasoning_effort=high")
            && what.contains("context_window_k 200")
            && what.contains("context_window_k 300"),
        "the decision names the change: {what}"
    );
}

/// A config for `harbor` with two lanes, in the given order (T-392 direction
/// check: the order is staffing too).
fn two_lane_config(order: &[&str; 2]) -> String {
    let lane = |name: &str| {
        format!(
            "[[lanes]]\nname = \"{name}\"\nduty = \"the {name} duty\"\nowns = [\"src/{name}/\"]\n\
             kind = \"opencode\"\nprofile = \"opencode\"\n\n"
        )
    };
    format!(
        r#"
[project]
id = "harbor"
herdr_session = "marina"
repo = "/repo"

[quota]
lanes = 3
temps = 6

[captain]
pane = "w2:p1"
kind = "codex"
profile = "codex"

{first}{second}[temps]
kind = "opencode"
profile = "opencode"

[models]
lanes = ["minimax-cn/MiniMax-M3.1-Flash"]
review = ["gpt-6.1-sol"]
on_quota = {{ lanes = "wait", review = "wait" }}

[conventions]
gate = "make test"
merge_message = "merge({{task}}): {{source}}"
"#,
        first = lane(order[0]),
        second = lane(order[1]),
    )
}

/// The direction check: the four sections are compared as whole sections, so
/// putting the same lanes in another order is a staffing change and is refused
/// without the user's words.
#[test]
fn a_reordered_lanes_section_is_a_staffing_change() {
    let mut s = departed_with(&two_lane_config(&["heat", "keel"]));
    let stored = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();

    let reordered = two_lane_config(&["keel", "heat"]);
    let err = init_with_home(&mut s, &admin(), &reordered, Some(crate::skill_home()), None, T0 + 2)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaffLocked, "{}", err.detail);
    assert!(
        err.detail.contains("lanes: heat, keel → keel, heat"),
        "the new order is named: {}",
        err.detail
    );
    let after = s.read(|tx| tx.get_project("harbor")).unwrap().unwrap();
    assert_eq!(after.config_version, stored.config_version);
    assert_eq!(after.config_toml, stored.config_toml);
    assert!(staff_decisions(&mut s).is_empty());

    // With the user's words the order applies, and the decision names it.
    init_with_home(
        &mut s,
        &admin(),
        &reordered,
        Some(crate::skill_home()),
        Some("run keel first, the user asked"),
        T0 + 2,
    )
    .unwrap();
    let decisions = staff_decisions(&mut s);
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert!(
        decisions[0].what.contains("lanes: heat, keel → keel, heat"),
        "{}",
        decisions[0].what
    );
}
