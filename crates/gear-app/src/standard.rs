//! The project standard (T-84): the one line every packet carries when the
//! project has one.
//!
//! `gear --skill adopt` (T-85) produces `gear/standard.md` in the project
//! root: the exemplars, the no-go patterns and the rule that new code meets
//! the standard even where its neighbours do not. Every task and review
//! packet says so, with the file's absolute path, so the writer and the
//! reviewer judge against the same text.

use std::path::{Path, PathBuf};

/// The line a packet carries, when `gear/standard.md` exists under
/// `project_root` (T-84): the file's absolute path and what it is for.
/// `None` when the project has no standard — the packet says nothing.
pub fn line(project_root: &str) -> Option<String> {
    let path = standard_path(project_root)?;
    Some(format!(
        "Project standard: {} — new and changed code follows it, even where the code around it does not; the reviewer judges against it.",
        path.display()
    ))
}

/// The line for a registered project (T-84): its config's root is where
/// `gear/standard.md` lives. `None` when the project has none.
pub fn line_for(tx: &gear_store::Tx<'_>, project: &str) -> gear_model::GearResult<Option<String>> {
    let Some(row) = tx.get_project(project)? else {
        return Ok(None);
    };
    let Ok(config) = crate::config::parse(&row.config_toml) else {
        return Ok(None);
    };
    Ok(line(&config.project.repo))
}

/// The absolute path of the project's standard, when the file exists.
pub fn standard_path(project_root: &str) -> Option<PathBuf> {
    if project_root.trim().is_empty() {
        return None;
    }
    let path = Path::new(project_root).join("gear").join("standard.md");
    if !path.is_file() {
        return None;
    }
    // The packet carries an absolute path; a symlinked root resolves to the
    // place the file really is.
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}
