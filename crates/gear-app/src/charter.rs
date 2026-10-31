//! Role charters (T-469): what a seat's agent is told, per role, from the
//! config the user confirmed at kickoff.
//!
//! At kickoff the captain proposes, and the user confirms with the staffing,
//! one charter per role — for each lane and for the reviewers — written for the
//! agent and model that role runs: a lane on a fast, simple agent implements
//! its packet without designing, a lane on a strong agent designs inside its
//! own task and owned paths but never decides for the whole project, and the
//! captain always holds the whole picture.
//!
//! The charter generalises the fixed rules files
//! (`skills/gear/references/rules/lane.md` and `review.md`). Each holds a
//! stance — who the seat is and what it may decide — and the GEAR working rules
//! (asking, pushing, reporting) that every seat keeps. A project's charter
//! replaces the stance; the working rules are always rendered, charter or not;
//! a role that names no charter renders as the fixed file does today. The
//! rendered text is written into the seat's rules file and given to the agent
//! once per conversation: at a new session, never again in a resumed one.
//!
//! The captain's guide (`skills/gear/references/captain.md`) stays as it is:
//! the captain reads it through the skill, and it carries the captain's own
//! hand-fix and urgent-fix text.

use crate::config::{self, ProjectConfig};
use gear_herdr::Profile;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole};
use gear_store::Store;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The fixed lane rules: the default stance, then the GEAR working rules every
/// lane keeps.
pub const LANE_RULES: &str = include_str!("../../../skills/gear/references/rules/lane.md");
/// The fixed reviewer rules, the same way.
pub const REVIEW_RULES: &str = include_str!("../../../skills/gear/references/rules/review.md");

/// The line that ends a role's stance in its fixed rules file. What follows are
/// the working rules every charter keeps; the line itself is never rendered.
pub const STANCE_END: &str =
    "<!-- stance end: what follows are the GEAR working rules every charter keeps -->";

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// The fixed rules of a role.
fn fixed_rules(role: SeatRole) -> Option<&'static str> {
    match role {
        SeatRole::Lane => Some(LANE_RULES),
        SeatRole::Temp => Some(REVIEW_RULES),
        SeatRole::Captain => None,
    }
}

/// The stance and the working rules of a fixed rules file, the marker line
/// removed. `None` when the file has no marker (it is never rendered then).
fn split_rules(full: &str) -> Option<(&str, &str)> {
    let at = full.find(STANCE_END)?;
    let stance = full[..at].trim_end();
    let working = full[at + STANCE_END.len()..].trim_start_matches('\n');
    Some((stance, working))
}

/// The text of a role's rules file (T-469): its charter — the config's own
/// stance when it names one, else the fixed stance — followed by the working
/// rules every seat keeps, then the voyage's done and bar. `None` for the
/// captain, whose guide is a skill topic, not a rules file.
pub fn text(
    config: Option<&ProjectConfig>,
    role: SeatRole,
    seat: &str,
    done: &str,
    bar: &str,
) -> Option<String> {
    let (stance, working) = split_rules(fixed_rules(role)?)?;
    let stance = match role {
        SeatRole::Lane => config
            .and_then(|config| config.lanes.iter().find(|lane| lane.name == seat))
            .and_then(|lane| lane.charter.as_deref()),
        SeatRole::Temp => config
            .and_then(|config| config.temps.as_ref())
            .and_then(|temps| temps.charter.as_deref()),
        SeatRole::Captain => None,
    }
    .map(str::trim)
    .filter(|charter| !charter.is_empty())
    .unwrap_or(stance);
    Some(format!(
        "{stance}\n\n{}\nVoyage done: {}\nBar: {}\n",
        working.trim_end(),
        done.trim(),
        bar.trim()
    ))
}

/// The file name of a role's rules under the seat's `rules` folder (T-469):
/// one per lane, and one for the reviewers. `None` for the captain.
pub fn file_name(role: SeatRole, seat: &str) -> Option<String> {
    match role {
        SeatRole::Lane => Some(format!("lanes/{seat}.md")),
        SeatRole::Temp => Some("review.md".to_string()),
        SeatRole::Captain => None,
    }
}

/// The arguments that give a seat's agent its charter (T-469), with the rules
/// file written first.
///
/// `resumed` is the conversation the agent comes back to: a resumed session
/// already holds the charter it was given when it started, so it is given
/// none again. A captain, a profile whose CLI takes no instructions argument
/// and a seat without a database path get no arguments.
pub fn arguments(
    store: &mut Store,
    project: &str,
    role: SeatRole,
    seat: &str,
    db_path: &str,
    profile: &Profile,
    resumed: bool,
) -> GearResult<Vec<String>> {
    let Some(file) = file_name(role, seat) else {
        return Ok(Vec::new());
    };
    if db_path.is_empty() {
        return Ok(Vec::new());
    }
    let dir = Path::new(db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let voyage = store.read(|tx| {
        let id = crate::plan::current_voyage(tx, project)?;
        if id.is_empty() {
            Ok(None)
        } else {
            tx.get_voyage(project, &id)
        }
    })?;
    let done = voyage
        .as_ref()
        .and_then(|v| v.done.as_deref())
        .unwrap_or("-");
    let bar = voyage
        .as_ref()
        .and_then(|v| v.bar.as_deref())
        .unwrap_or("-");
    // A project without a stored config (a store written before one) keeps
    // the fixed charters; a config that does not parse is the engine's
    // finding elsewhere, not a start's failure.
    let config = store.read(|tx| tx.get_project(project))?;
    let config = config
        .filter(|row| !row.config_toml.trim().is_empty())
        .and_then(|row| config::parse(&row.config_toml).ok());
    let Some(text) = text(config.as_ref(), role, seat, done, bar) else {
        return Ok(Vec::new());
    };
    let path = write_rules(dir, &file, &text)?;
    if resumed {
        return Ok(Vec::new());
    }
    Ok(gear_herdr::profile::render_rules(
        &profile.instructions_args,
        &path.to_string_lossy(),
        &text,
    ))
}

/// Write `text` as the rules file `file` under `dir`, atomically, and only when
/// it changed. Returns its path.
fn write_rules(dir: &Path, file: &str, text: &str) -> GearResult<PathBuf> {
    let path = dir.join("rules").join(file);
    if std::fs::read(&path).ok().as_deref() == Some(text.as_bytes()) {
        return Ok(path);
    }
    let io_error = |e: std::io::Error| {
        GearError::new(
            ErrorCode::BadFormat,
            format!("cannot write role rules {}: {e}", path.display()),
        )
    };
    let parent = path.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent).map_err(io_error)?;
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temp, text).map_err(io_error)?;
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(io_error(error));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_store::{testing, VoyageRow};

    const TWO_CHARTERS: &str = r#"
[project]
id = "p"
herdr_session = "s"
repo = "/repo"

[quota]
lanes = 2
temps = 1

[captain]
pane = "w1:p1"
kind = "pi"
profile = "pi"

[[lanes]]
name = "heat"
duty = "heat"
owns = ["src/heat/"]
kind = "pi"
profile = "pi"
charter = """
You implement your packet without designing; the captain decides.
"""

[[lanes]]
name = "pricing"
duty = "pricing"
owns = ["src/pricing/"]
kind = "pi"
profile = "pi"
charter = """
You are a strong lane: design inside your task and owned paths, never for the project.
"""

[temps]
kind = "pi"
profile = "pi"
charter = """
You judge one candidate against its contract and the bar.
"""

[conventions]
gate = "make test"
"#;

    fn world(config_text: &str) -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                let mut row = testing::seed_project(tx, "p", "s", 100)?;
                row.config_toml = config_text.to_string();
                tx.update_project(&row)?;
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: "M1".into(),
                    title: "current".into(),
                    done: Some("land the fleet".into()),
                    bar: Some("stable".into()),
                    eta: None,
                    departure: Some(2),
                    departure_tasks: None,
                    departure_estimate_min: None,
                    updated: 2,
                })
            })
            .unwrap();
        (store, dir)
    }

    fn pi() -> Profile {
        gear_herdr::builtin("pi").unwrap()
    }

    /// A working rule of every lane: it is rendered under a charter too.
    const LANE_WORKING: &str = "- Search only inside your worktree";
    /// A working rule of every reviewer.
    const REVIEW_WORKING: &str = "- Read the voyage's goal and bar first";

    #[test]
    fn two_charters_render_two_rules_files() {
        let (mut store, dir) = world(TWO_CHARTERS);
        let db = dir.path().join("gear.db");
        let db = db.to_str().unwrap();
        let heat = arguments(&mut store, "p", SeatRole::Lane, "heat", db, &pi(), false).unwrap();
        let pricing =
            arguments(&mut store, "p", SeatRole::Lane, "pricing", db, &pi(), false).unwrap();
        let heat_path = dir.path().join("rules/lanes/heat.md");
        let pricing_path = dir.path().join("rules/lanes/pricing.md");
        assert_eq!(
            heat,
            ["--append-system-prompt", heat_path.to_str().unwrap()]
        );
        assert_eq!(
            pricing,
            ["--append-system-prompt", pricing_path.to_str().unwrap()]
        );
        let heat_text = std::fs::read_to_string(&heat_path).unwrap();
        let pricing_text = std::fs::read_to_string(&pricing_path).unwrap();
        assert_ne!(heat_text, pricing_text);
        assert!(heat_text.starts_with("You implement your packet without designing"));
        assert!(pricing_text.starts_with("You are a strong lane"));
        // The stance is the charter's; the working rules are always there.
        assert!(heat_text.contains(LANE_WORKING), "{heat_text}");
        assert!(pricing_text.contains(LANE_WORKING), "{pricing_text}");
        assert!(!heat_text.contains("You never design."), "{heat_text}");
        assert!(heat_text.ends_with("Voyage done: land the fleet\nBar: stable\n"));
        assert!(pricing_text.ends_with("Voyage done: land the fleet\nBar: stable\n"));
        let review = arguments(
            &mut store,
            "p",
            SeatRole::Temp,
            "review-1",
            db,
            &pi(),
            false,
        )
        .unwrap();
        let review_path = dir.path().join("rules/review.md");
        assert_eq!(
            review,
            ["--append-system-prompt", review_path.to_str().unwrap()]
        );
        let review_text = std::fs::read_to_string(&review_path).unwrap();
        assert!(review_text.starts_with("You judge one candidate"));
        assert!(review_text.contains(REVIEW_WORKING), "{review_text}");
        assert!(review_text.ends_with("Voyage done: land the fleet\nBar: stable\n"));
        // The captain has no rules file: its guide is a skill topic.
        assert!(arguments(
            &mut store,
            "p",
            SeatRole::Captain,
            "captain",
            db,
            &pi(),
            false
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn a_resumed_session_gets_no_charter_again() {
        let (mut store, dir) = world(TWO_CHARTERS);
        let db = dir.path().join("gear.db");
        let db = db.to_str().unwrap();
        assert!(
            !arguments(&mut store, "p", SeatRole::Lane, "heat", db, &pi(), false)
                .unwrap()
                .is_empty(),
            "a new session is given the charter"
        );
        assert!(
            arguments(&mut store, "p", SeatRole::Lane, "heat", db, &pi(), true)
                .unwrap()
                .is_empty(),
            "a resumed session is given no charter again"
        );
        // The rules file stays for the next new session.
        assert!(dir.path().join("rules/lanes/heat.md").is_file());
        assert!(
            arguments(&mut store, "p", SeatRole::Temp, "review-1", db, &pi(), true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_role_without_a_charter_renders_as_today() {
        let config = config::parse(
            r#"
[project]
id = "p"
herdr_session = "s"
repo = "/repo"

[quota]
lanes = 1
temps = 1

[captain]
pane = "w1:p1"
kind = "pi"
profile = "pi"

[[lanes]]
name = "heat"
duty = "heat"
owns = ["src/heat/"]
kind = "pi"
profile = "pi"

[temps]
kind = "pi"
profile = "pi"

[conventions]
gate = "make test"
"#,
        )
        .unwrap();
        let lane = text(Some(&config), SeatRole::Lane, "heat", "-", "-").unwrap();
        // The fixed file with its marker line removed: today's rules.
        let fixed = LANE_RULES.replace(&format!("{STANCE_END}\n\n"), "");
        assert_eq!(
            lane,
            format!("{}\nVoyage done: -\nBar: -\n", fixed.trim_end())
        );
        let review = text(Some(&config), SeatRole::Temp, "review-1", "-", "-").unwrap();
        let fixed = REVIEW_RULES.replace(&format!("{STANCE_END}\n\n"), "");
        assert_eq!(
            review,
            format!("{}\nVoyage done: -\nBar: -\n", fixed.trim_end())
        );
        // Without a stored config the fixed charters are rendered too.
        assert_eq!(text(None, SeatRole::Lane, "heat", "-", "-").unwrap(), lane);
        assert_eq!(
            text(Some(&config), SeatRole::Captain, "captain", "-", "-"),
            None
        );
    }

    #[test]
    fn the_fixed_rules_hold_a_stance_and_the_working_rules() {
        for rules in [LANE_RULES, REVIEW_RULES] {
            let (stance, working) = split_rules(rules).unwrap();
            assert!(!stance.trim().is_empty());
            assert!(!working.trim().is_empty());
            assert!(!stance.contains(STANCE_END));
        }
    }

    #[test]
    fn a_profile_without_instructions_gets_no_arguments() {
        let (mut store, dir) = world(TWO_CHARTERS);
        let db = dir.path().join("gear.db");
        let opencode = gear_herdr::builtin("opencode").unwrap();
        assert!(arguments(
            &mut store,
            "p",
            SeatRole::Lane,
            "heat",
            db.to_str().unwrap(),
            &opencode,
            false
        )
        .unwrap()
        .is_empty());
        // The rules file is still written: the next new session reads it.
        assert!(dir.path().join("rules/lanes/heat.md").is_file());
    }

    #[test]
    fn an_unchanged_rules_file_is_not_rewritten() {
        let (mut store, dir) = world(TWO_CHARTERS);
        let db = dir.path().join("gear.db");
        let db = db.to_str().unwrap();
        arguments(&mut store, "p", SeatRole::Lane, "heat", db, &pi(), false).unwrap();
        let path = dir.path().join("rules/lanes/heat.md");
        let earlier = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(earlier))
            .unwrap();
        arguments(&mut store, "p", SeatRole::Lane, "heat", db, &pi(), false).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            earlier
        );
    }
}
