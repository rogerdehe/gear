//! Materialize role instructions beside the store and render profile arguments.
use gear_herdr::Profile;
use gear_model::{ErrorCode, GearError, GearResult, SeatRole};
use gear_store::Store;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const LANE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../skills/gear/references/rules/lane.md"
));
const REVIEW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../skills/gear/references/rules/review.md"
));
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn arguments(
    store: &mut Store,
    project: &str,
    role: SeatRole,
    db_path: &str,
    profile: &Profile,
) -> GearResult<Vec<String>> {
    let (name, base) = match role {
        SeatRole::Lane => ("lane", LANE),
        SeatRole::Temp => ("review", REVIEW),
        SeatRole::Captain => return Ok(Vec::new()),
    };
    if db_path.is_empty() {
        return Ok(Vec::new());
    }
    let dir = Path::new(db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let voyage = store.read(|tx| {
        let id = gear_app::plan::current_voyage(tx, project)?;
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
    let text = format!(
        "{}\nVoyage done: {}\nBar: {}\n",
        base.trim_end(),
        done.trim(),
        bar.trim()
    );
    let rules_dir = dir.join("rules");
    let path = rules_dir.join(format!("{name}.md"));
    if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
        let io_error = |e: std::io::Error| {
            GearError::new(
                ErrorCode::BadFormat,
                format!("cannot write role rules {}: {e}", path.display()),
            )
        };
        std::fs::create_dir_all(&rules_dir).map_err(io_error)?;
        let temp = rules_dir.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&temp, &text).map_err(io_error)?;
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(io_error(error));
        }
    }
    Ok(gear_herdr::profile::render_rules(
        &profile.instructions_args,
        &path.to_string_lossy(),
        &text,
    ))
}

/// Refresh a persisted busy-start's inline text while retaining model and
/// resume arguments. A pending start from an older engine may have no rules.
pub(crate) fn refresh_args(args: &mut Vec<String>, profile: &Profile, rendered: Vec<String>) {
    if rendered.is_empty() {
        return;
    }
    let offset = profile.unattended_args.len();
    let count = profile.instructions_args.len();
    let looks_like_rules = args
        .get(offset..offset.saturating_add(count))
        .is_some_and(|old| {
            old.iter()
                .zip(&profile.instructions_args)
                .all(|(value, template)| {
                    let first = template
                        .find("{rules_file}")
                        .or_else(|| template.find("{rules_toml}"));
                    match first {
                        Some(i) => value.starts_with(&template[..i]),
                        None => value == template,
                    }
                })
        });
    let end = if looks_like_rules {
        offset + count
    } else {
        offset
    };
    if offset <= args.len() {
        args.splice(offset..end, rendered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gear_store::{testing, VoyageRow};

    #[test]
    fn roles_render_current_voyage_and_do_not_rewrite_unchanged_files() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("gear.db");
        let mut store = Store::open_in_memory().unwrap();
        store
            .write(|tx| {
                testing::seed_project(tx, "p", "sess", 100)?;
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: "M1".into(),
                    title: "old".into(),
                    done: Some("old goal".into()),
                    bar: Some("old bar".into()),
                    eta: None,
                    departure: Some(1),
                    departure_tasks: None,
                    departure_estimate_min: None,
                    updated: 1,
                })?;
                tx.upsert_voyage(&VoyageRow {
                    project: "p".into(),
                    milestone: "M2".into(),
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
        let pi = gear_herdr::builtin("pi").unwrap();
        let args = arguments(&mut store, "p", SeatRole::Lane, db.to_str().unwrap(), &pi).unwrap();
        let path = dir.path().join("rules/lane.md");
        assert_eq!(args, ["--append-system-prompt", path.to_str().unwrap()]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(LANE.trim_end()));
        assert!(text.ends_with("Voyage done: land the fleet\nBar: stable\n"));
        let earlier = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(earlier))
            .unwrap();
        arguments(&mut store, "p", SeatRole::Lane, db.to_str().unwrap(), &pi).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            earlier
        );
        let codex = gear_herdr::builtin("codex").unwrap();
        let review = arguments(
            &mut store,
            "p",
            SeatRole::Temp,
            db.to_str().unwrap(),
            &codex,
        )
        .unwrap();
        assert_eq!(review[0], "-c");
        let value: toml::Table = toml::from_str(&review[1]).unwrap();
        let text = value
            .get("developer_instructions")
            .and_then(toml::Value::as_str)
            .unwrap();
        let review_path = dir.path().join("rules/review.md");
        assert!(text.contains(review_path.to_str().unwrap()), "{text}");
        let written = std::fs::read_to_string(&review_path).unwrap();
        assert!(written.starts_with(REVIEW.trim_end()));
        assert!(written.ends_with("Voyage done: land the fleet\nBar: stable\n"));
        assert!(arguments(
            &mut store,
            "p",
            SeatRole::Captain,
            db.to_str().unwrap(),
            &codex
        )
        .unwrap()
        .is_empty());
        let opencode = gear_herdr::builtin("opencode").unwrap();
        assert!(arguments(
            &mut store,
            "p",
            SeatRole::Lane,
            db.to_str().unwrap(),
            &opencode
        )
        .unwrap()
        .is_empty());
    }
}
