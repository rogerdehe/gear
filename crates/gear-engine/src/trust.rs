//! Grok's trust store (T-32).
//!
//! Grok asks "Do you trust the contents of this directory?" the first time it
//! starts in a worktree — while Herdr reports the agent idle, so a prompt sent
//! then is lost (the SOP `gear --skill agent:grok`). The engine trusts the
//! worktree before it starts a grok agent: one entry in
//! `$HOME/.grok/trusted_folders.toml`, appended only when it is not there yet.

use std::path::Path;

/// The agent kind whose CLI keeps a trust store.
pub const GROK: &str = "grok";

/// Trust a grok worktree before its agent starts; other kinds need nothing.
pub fn trust_worktree(kind: &str, path: &str, now: i64) -> Result<(), String> {
    if kind != GROK {
        return Ok(());
    }
    let Some(home) = std::env::var_os("HOME") else {
        return Err("HOME is not set, so grok's trust store is unknown".to_string());
    };
    trust_grok_worktree_in(Path::new(&home), path, now)
}

/// Trust `path` in the grok store under `home`, once: an entry that is there is
/// left alone and nothing else in the file is touched (append only). A store
/// that cannot be written is an error — the engine never starts an untrusted
/// grok.
pub fn trust_grok_worktree_in(home: &Path, path: &str, now: i64) -> Result<(), String> {
    let file = home.join(".grok").join("trusted_folders.toml");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let header = format!("[folders.\"{path}\"]");
    if existing.lines().any(|line| line.trim() == header) {
        // Already trusted: append nothing.
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut body = existing;
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&format!("{header}\ntrusted = true\ndecided_at = {now}\n"));
    std::fs::write(&file, body).map_err(|e| format!("{}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch HOME.
    fn home(name: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!(
            "gear-trust-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn the_trust_entry_is_written_once_and_appended() {
        let home = home("once");
        trust_grok_worktree_in(&home, "/worktrees/gear-t32", 1_800_000_000).unwrap();
        let file = home.join(".grok/trusted_folders.toml");
        let once = std::fs::read_to_string(&file).unwrap();
        assert!(once.contains("[folders.\"/worktrees/gear-t32\"]"), "{once}");
        assert!(once.contains("trusted = true"), "{once}");
        assert!(once.contains("decided_at = 1800000000"), "{once}");

        // The second start appends nothing: one entry, byte for byte.
        trust_grok_worktree_in(&home, "/worktrees/gear-t32", 1_800_000_100).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), once, "written once");

        // Another worktree is appended, and the first entry stays.
        trust_grok_worktree_in(&home, "/worktrees/gear-t33", 1_800_000_200).unwrap();
        let two = std::fs::read_to_string(&file).unwrap();
        assert!(two.contains("[folders.\"/worktrees/gear-t33\"]"), "{two}");
        assert!(two.starts_with(&once), "append only: {two}");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn only_grok_has_a_trust_store() {
        let home = home("kinds");
        trust_worktree("opencode", "/worktrees/gear-t32", 1).unwrap();
        assert!(
            !home.join(".grok").exists(),
            "a kind without a trust store writes nothing"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
