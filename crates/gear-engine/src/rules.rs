//! A seat's role charter (T-469): the text comes from `gear_app::charter`, so
//! the config the user confirmed at kickoff decides what a lane or a reviewer
//! is told; this module adds the refresh of a persisted busy start's inline
//! arguments.
use gear_herdr::Profile;
use gear_model::{GearResult, SeatRole};
use gear_store::Store;

/// The arguments that give `seat` its charter, with its rules file written
/// first. A resumed session is given none: it already holds the charter it
/// started with (T-469).
pub(crate) fn arguments(
    store: &mut Store,
    project: &str,
    role: SeatRole,
    seat: &str,
    db_path: &str,
    profile: &Profile,
    resumed: bool,
) -> GearResult<Vec<String>> {
    gear_app::charter::arguments(store, project, role, seat, db_path, profile, resumed)
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
