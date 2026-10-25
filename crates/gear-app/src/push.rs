//! Where a project may be pushed (T-73, trial G12).
//!
//! `gear admin init` freezes, for the project root and for every `[[repos]]`
//! entry, the destinations the repo's own `origin` remote resolves to:
//! `git remote get-url --push --all origin`, after git's own
//! `url.*.insteadOf` and `url.*.pushInsteadOf` rewrites. The list is stored
//! normalised as host + path and read again on every re-init, so a project
//! that moves its remote moves with it.
//!
//! [`check`] answers the one question a push asks: may this repo be pushed
//! to this destination? The engine's merge push (the steady lane) and the
//! pre-push hook `gear push-check` (the base lane) ask it before they push.
//!
//! The rules:
//!
//! - a repo the project does not have is refused;
//! - a repo with no frozen destination is refused (nothing may be pushed
//!   to it until an init reads its remote again);
//! - a destination that is not one of the repo's frozen destinations is
//!   refused, whatever transport it is named in.
//!
//! The destinations are compared as git resolved them: the URL a caller
//! hands to [`check`] is the one the push would go to — the pre-push hook's
//! second argument, or `git remote get-url --push origin` — never the raw
//! address the remote is written as. A remote written as
//! `git@github.com:owner/repo.git` that a rewrite sends to
//! `https://ext.example.com/owner/repo.git` is registered as the place the
//! push goes, and the raw address is refused.
//!
//! A refusal names the repo and the normalised destination, and is never
//! retried or routed around: it is one item in the captain's inbox.

use std::path::Path;
use std::process::Command;

use gear_model::Ts;
use gear_store::{Store, Tx};

use crate::config::{ProjectConfig, Repo};

/// Why a push was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The project is not registered, or its config cannot be read.
    UnknownProject,
    /// The repo is not one of the project's repos.
    UnknownRepo,
    /// The repo has no frozen destination: nothing may be pushed to it.
    NoDestinations,
    /// The destination is not one of the repo's frozen destinations.
    NotRegistered,
    /// The store could not be read: nothing can be said about the push, so
    /// it is refused.
    Unreadable,
}

impl Reason {
    /// The reason in a few words, for the captain's item.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::UnknownProject => "the project is not registered",
            Reason::UnknownRepo => "the project has no such repo",
            Reason::NoDestinations => "the repo has no registered destination",
            Reason::NotRegistered => "the destination is not registered",
            Reason::Unreadable => "the store could not be read",
        }
    }
}

/// A push the project does not allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The repo the push was for: the `[[repos]]` name, `""` for the
    /// project root.
    pub repo: String,
    /// The normalised destination the push would have gone to: host + path.
    pub destination: String,
    /// Why the push was refused.
    pub reason: Reason,
}

impl Refusal {
    /// The host of the refused destination (`github.com`); `""` for a local
    /// path, which has no host.
    pub fn host(&self) -> &str {
        match self.destination.split_once('/') {
            Some((host, _)) => host,
            None => "",
        }
    }

    /// The line the captain reads: the repo, the destination, the reason.
    pub fn message(&self) -> String {
        let destination = if self.destination.is_empty() {
            "an unnamed destination"
        } else {
            self.destination.as_str()
        };
        format!(
            "push refused: {} may not be pushed to {} ({})",
            repo_label(&self.repo),
            destination,
            self.reason.as_str()
        )
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// How a repo is named in a message: the project root has no name.
fn repo_label(repo: &str) -> &str {
    if repo.is_empty() {
        "the project root"
    } else {
        repo
    }
}

/// The destinations frozen for one repo.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Frozen {
    /// The repo's name: `""` for the project root, else the `[[repos]]` name.
    pub repo: String,
    /// The normalised destinations the repo may be pushed to.
    pub destinations: Vec<String>,
}

/// May the project push `repo` to `url`?
///
/// `repo` is the repo of the project: the `[[repos]]` entry by its name, or
/// `""` for the project root. A repo's own path is accepted as well, so a
/// caller that knows where the push happens need not know the name.
///
/// `url` is the destination as git resolved it — the pre-push hook's second
/// argument, or `git remote get-url --push origin` — and it is compared as
/// it stands: the frozen destinations were read after git's URL rewrites, so
/// a raw (unrewritten) URL is not the place the push would go.
///
/// `Ok(())` = the push may go ahead. `Err(refusal)` = it may not; the
/// refusal names the repo and the normalised destination.
pub fn check(store: &mut Store, project: &str, repo: &str, url: &str) -> Result<(), Refusal> {
    let destination = normalise(url).unwrap_or_default();
    let refuse = |name: &str, reason: Reason| Refusal {
        repo: name.to_string(),
        destination: destination.clone(),
        reason,
    };

    // The project must be registered: its config says which repos it has.
    let config = match config_of(store, project) {
        Ok(Some(config)) => config,
        Ok(None) => return Err(refuse(repo, Reason::UnknownProject)),
        Err(()) => return Err(refuse(repo, Reason::Unreadable)),
    };

    // The repo must be one of the project's repos: the root, or a
    // `[[repos]]` entry.
    let Some(name) = repo_name_of(&config, repo) else {
        return Err(refuse(repo, Reason::UnknownRepo));
    };

    // The destinations frozen for the repo when the project was registered.
    let destinations = match store.read(|tx| tx.push_destinations(project, &name)) {
        Ok(destinations) => destinations,
        Err(_) => return Err(refuse(&name, Reason::Unreadable)),
    };
    if destinations.is_empty() {
        return Err(refuse(&name, Reason::NoDestinations));
    }
    if destinations.iter().any(|allowed| *allowed == destination) {
        return Ok(());
    }
    Err(refuse(&name, Reason::NotRegistered))
}

/// The destination a URL names, normalised: host + path.
///
/// - `https://github.com/owner/repo.git` → `github.com/owner/repo`
/// - `git@github.com:owner/repo.git` → `github.com/owner/repo`
/// - `ssh://git@github.com:22/owner/repo` → `github.com/owner/repo`
/// - `file:///srv/git/repo.git` → `/srv/git/repo`
/// - `/srv/git/repo.git` → `/srv/git/repo`
///
/// The host is lowercased (a host is not case sensitive); the path is not (a
/// path that differs in case is another place). The scheme, the user and the
/// port are transport, not destination: one repo reached two ways is one
/// destination. `None` when the URL names nothing.
pub fn normalise(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let (host, path) = split(url);
    let host = host.to_lowercase();
    let path = clean_path(path);
    if host.is_empty() {
        if path.is_empty() {
            None
        } else {
            Some(path)
        }
    } else if path.is_empty() {
        Some(host)
    } else {
        Some(format!("{host}/{path}"))
    }
}

/// The host and the path of a URL.
fn split(url: &str) -> (&str, &str) {
    // A scheme: `scheme://authority/path`. The user (`git@`) and the port
    // are not the host.
    if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (rest, ""),
        };
        // `file:///srv/git/repo` has no authority: its path keeps its slash.
        if authority.is_empty() {
            return ("", rest);
        }
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        return (host, path);
    }
    // The scp form: `user@host:path`.
    if let Some((head, path)) = url.split_once(':') {
        let host = head.rsplit('@').next().unwrap_or(head);
        if !host.is_empty() && !host.contains('/') {
            return (host, path);
        }
    }
    // A local path.
    ("", url)
}

/// The path of a destination: no trailing slash, no `.git` suffix.
fn clean_path(path: &str) -> String {
    let path = path.trim().trim_end_matches('/');
    if path.len() > 4 && path[path.len() - 4..].eq_ignore_ascii_case(".git") {
        path[..path.len() - 4].to_string()
    } else {
        path.to_string()
    }
}

/// Read the push destinations of every repo of the project: the project root
/// and each `[[repos]]` entry, in config order.
///
/// Git runs here, outside any transaction: the store is never held while a
/// subprocess runs. A repo that is not a git repository, that has no
/// `origin` remote, or whose remote names no URL has no destination —
/// nothing may be pushed to it until an init reads it again.
pub fn read_destinations(config: &ProjectConfig) -> Vec<Frozen> {
    config
        .repos()
        .iter()
        .map(|repo: &Repo| Frozen {
            repo: repo.name.clone(),
            destinations: destinations_of(repo),
        })
        .collect()
}

/// The normalised destinations of one repo: every URL its `origin` remote
/// pushes to, without duplicates.
fn destinations_of(repo: &Repo) -> Vec<String> {
    let mut destinations: Vec<String> = push_urls(Path::new(&repo.path))
        .iter()
        .filter_map(|url| normalise(url))
        .collect();
    destinations.sort();
    destinations.dedup();
    destinations
}

/// The URLs the repo's `origin` remote pushes to, as git resolves them:
/// `git remote get-url --push --all origin`, after the `url.*.insteadOf` and
/// `url.*.pushInsteadOf` rewrites. Empty when the repo has no such remote.
pub fn push_urls(dir: &Path) -> Vec<String> {
    let dir = dir.to_string_lossy().into_owned();
    let Ok(output) = Command::new("git")
        .args([
            "-C",
            dir.as_str(),
            "remote",
            "get-url",
            "--push",
            "--all",
            "origin",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Freeze the destinations: the stored list of every repo is what was read
/// now, and a repo that is gone leaves nothing behind.
///
/// Called inside the transaction that registers or re-registers the project,
/// so a project never exists without the destinations it was registered with.
pub fn freeze(tx: &Tx<'_>, project: &str, frozen: &[Frozen], now: Ts) -> gear_model::GearResult<()> {
    tx.clear_push_destinations(project)?;
    for repo in frozen {
        tx.set_push_destinations(project, &repo.repo, &repo.destinations, now)?;
    }
    Ok(())
}

/// The config the project is registered with, parsed. `Ok(None)` = the
/// project is not registered; `Err(())` = the store or the config cannot be
/// read.
fn config_of(store: &mut Store, project: &str) -> Result<Option<ProjectConfig>, ()> {
    let row = store.read(|tx| tx.get_project(project)).map_err(|_| ())?;
    let Some(row) = row else {
        return Ok(None);
    };
    crate::config::parse(&row.config_toml).map(Some).map_err(|_| ())
}

/// The repo of the project that `repo` names: the project root (`""`), a
/// `[[repos]]` entry by its name, or a repo by the path it lives in. `None`
/// when the project has no such repo.
fn repo_name_of(config: &ProjectConfig, repo: &str) -> Option<String> {
    if repo.is_empty() {
        return Some(String::new());
    }
    if config.repos.iter().any(|entry| entry.name == repo) {
        return Some(repo.to_string());
    }
    // The path of a repo: the caller may know where the push happens without
    // knowing the name.
    config
        .repos()
        .into_iter()
        .find(|resolved| same_path(&resolved.path, repo))
        .map(|resolved| resolved.name)
}

/// Are two paths the same place? The strings first, then the paths the file
/// system knows (a symlinked temp dir is one place under two names).
fn same_path(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_names_a_host_and_a_path() {
        assert_eq!(
            normalise("https://github.com/owner/repo.git").as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(
            normalise("https://GitHub.com/owner/repo").as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(
            normalise("git@github.com:owner/repo.git").as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(
            normalise("ssh://git@github.com:22/owner/repo").as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(
            normalise("https://user:token@github.com/owner/repo.git").as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(
            normalise("file:///srv/git/repo.git").as_deref(),
            Some("/srv/git/repo")
        );
        assert_eq!(normalise("/srv/git/repo.git").as_deref(), Some("/srv/git/repo"));
        assert_eq!(normalise("/srv/git/repo/").as_deref(), Some("/srv/git/repo"));
    }

    #[test]
    fn the_path_keeps_its_case() {
        // Two paths that differ in case are two places: refusing one of them
        // is the safe side.
        assert_eq!(
            normalise("https://github.com/Owner/Repo").as_deref(),
            Some("github.com/Owner/Repo")
        );
        assert_ne!(
            normalise("https://github.com/owner/repo"),
            normalise("https://github.com/Owner/repo")
        );
    }

    #[test]
    fn a_url_that_names_nothing_is_none() {
        assert_eq!(normalise(""), None);
        assert_eq!(normalise("   "), None);
        assert_eq!(normalise("https://github.com").as_deref(), Some("github.com"));
    }
}
