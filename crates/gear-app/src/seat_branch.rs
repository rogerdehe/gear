//! The branch of a seat's own worktree (T-252, T-559, T-565, T-584).
//!
//! One place that moves it, for both callers: a config apply that changes
//! `conventions.seat_branch` (T-559), and the engine when it starts or onboards
//! a seat whose branch still carries the name the **previous default** gave it
//! (T-565: `{project}/{seat}`, before v0.6.55's `{project}/{seat}/home`; T-584:
//! `{project}/{seat}/home`, before `{project}/{seat}/{voyage}`). The second case
//! needed the first: a project that never wrote a `seat_branch` line has no
//! config change to apply, so no apply ran the rename, and git refused the new
//! name while the old ref stood in its way.
//!
//! [`settle`] is also what a seat branch is created through: a seat's own old
//! branch is renamed to the new name — git cannot hold `p/hull` and
//! `p/hull/home` at once, and a name beside the new one (`p/hull/home` beside
//! `p/hull/M2`) is never left behind — and a ref of another name that is a
//! folder of the new one is named, never touched.

/// The name the default seat branch had before `{project}/{seat}/{voyage}`
/// (T-584): v0.6.55's `{project}/{seat}/home`, so a lane's `p/hull/home` is
/// renamed instead of being left beside `p/hull/M2`.
///
/// A seat branch named by this template is the seat's own: it is moved to the
/// current template's name when the seat is next onboarded.
pub const PREVIOUS_DEFAULT: &str = "{project}/{seat}/home";

/// The old default's expansion for one seat: `p/hull/home`.
pub fn previous_default(project: &str, seat: &str) -> String {
    gear_model::branch::expand_seat(PREVIOUS_DEFAULT, project, seat, "")
}

/// What [`settle`] did or found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// Nothing stood in the way: the name is free.
    Free,
    /// The seat's own old branch was renamed to the new name.
    Renamed {
        /// The name the branch had.
        old: String,
        /// The name it has now.
        new: String,
        /// What happened on the remote, in words (one line for the captain).
        remote: String,
    },
}

/// Move the seat's own branch to `new` before anything is created on it, and
/// fail naming a ref that stands in the way (T-565, T-584).
///
/// `dir` is a worktree or a repository of the seat's repo; `old` is the name the
/// previous default gave the seat's branch; `new` is the branch the current
/// `conventions.seat_branch` names. The seat's own old branch is renamed to
/// `new` — whether it stands in the way (git cannot hold `p/hull/home` and
/// `p/hull/home/x` at once) or merely beside it (`p/hull/home` beside
/// `p/hull/M2`, which would otherwise be left behind). A ref of another name
/// that is a folder of `new` is refused with its name, and nothing is touched.
pub fn settle(dir: &str, old: &str, new: &str, remote: &str) -> Result<Settled, String> {
    if old == new {
        return Ok(Settled::Free);
    }
    match blocking_ref(dir, new).as_deref() {
        Some(blocking) if blocking != old => {
            return Err(format!(
                "the branch {new} cannot be created: the ref {blocking} stands in its way (git cannot hold a ref and a folder of the same name); rename or delete {blocking}, or give the seat branches another name"
            ));
        }
        // The seat's own old branch is in the way, and is renamed below.
        Some(_) => {}
        // Nothing is in the way: only an old branch of the seat's own to move.
        None if !local_branch(dir, old) => return Ok(Settled::Free),
        None => {}
    }
    rename(dir, old, new)?;
    Ok(Settled::Renamed {
        old: old.to_string(),
        new: new.to_string(),
        remote: move_remote(dir, remote, old, new),
    })
}

/// Whether `dir` has the local branch `branch`.
fn local_branch(dir: &str, branch: &str) -> bool {
    git(
        dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// The ref (short name) that is a folder of `branch`, if there is one.
///
/// `p/hull` for `p/hull/home`: creating the second needs the first to go.
pub fn blocking_ref(dir: &str, branch: &str) -> Option<String> {
    let refs = git(
        dir,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )
    .ok()?;
    refs.lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .find(|name| branch.starts_with(&format!("{name}/")))
        .map(str::to_string)
}

/// Rename the local branch `old` to `new`, wherever it is checked out.
pub fn rename(dir: &str, old: &str, new: &str) -> Result<(), String> {
    let moved = git(dir, &["branch", "-m", old, new]);
    if moved.is_ok() {
        return Ok(());
    }
    // A branch checked out in a worktree that git refuses to rename from here is
    // renamed inside that worktree: the seat's own checkout is the one that
    // stands on the old name.
    let where_checked_out = git(dir, &["worktree", "list", "--porcelain"])
        .ok()
        .and_then(|listed| {
            let mut path = None;
            for line in listed.lines() {
                if let Some(rest) = line.strip_prefix("worktree ") {
                    path = Some(rest.trim().to_string());
                }
                if line.trim() == format!("branch refs/heads/{old}") {
                    return path;
                }
            }
            None
        });
    let Some(path) = where_checked_out else {
        return Err(format!(
            "the branch {old} could not be renamed to {new}: {}",
            moved.unwrap_or_default()
        ));
    };
    git(&path, &["branch", "-m", old, new]).map_err(|error| {
        format!("the branch {old} could not be renamed to {new} in {path}: {error}")
    })?;
    Ok(())
}

/// Move the branch on the remote as well, when it is there: one line saying
/// what happened, never a reason to fail.
pub fn move_remote(dir: &str, remote: &str, old: &str, new: &str) -> String {
    let remote = remote.trim();
    if remote.is_empty() || !has_branch(dir, remote, old) {
        return String::new();
    }
    let push = |dir: &str, name: &str| {
        git(
            dir,
            &[
                "push",
                remote,
                &format!("refs/heads/{name}:refs/heads/{name}"),
            ],
        )
    };
    match push(dir, new) {
        Ok(_) => match git(dir, &["push", remote, "--delete", old]) {
            Ok(_) => format!("; {remote} has {new} now, and {old} is gone there"),
            Err(error) => {
                format!("; {remote} has {new} now, but deleting {old} there failed ({error})")
            }
        },
        // A remote cannot hold the old name and a folder of it at once either
        // (T-565: `p/hull` blocked `p/hull/home` there as it did here): the old
        // ref goes first, then the new one arrives.
        Err(first) if new.starts_with(&format!("{old}/")) => {
            match git(dir, &["push", remote, "--delete", old]) {
                Ok(_) => match push(dir, new) {
                    Ok(_) => format!("; {remote} has {new} now, and {old} is gone there"),
                    Err(second) => format!(
                        "; {old} is gone from {remote} but pushing {new} failed ({second}), so {remote} has neither"
                    ),
                },
                Err(error) => format!(
                    "; pushing {new} failed ({first}) and deleting {old} there failed ({error}), so {remote} still has {old}"
                ),
            }
        }
        Err(error) => {
            format!("; pushing {new} to {remote} failed ({error}), so {remote} still has {old}")
        }
    }
}

/// Whether `remote` has the branch `branch` of the repository in `dir`.
pub fn has_branch(dir: &str, remote: &str, branch: &str) -> bool {
    git(dir, &["ls-remote", "--heads", remote, branch])
        .map(|found| !found.is_empty())
        .unwrap_or(false)
}

/// Run `git <args>` in `dir`; the trimmed stdout, or the error's words.
pub fn git(dir: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|error| format!("git could not run: {error}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if said.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            said
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with a branch, in a scratch directory of the test.
    fn repo(name: &str) -> (std::path::PathBuf, String) {
        let dir =
            std::env::temp_dir().join(format!("gear-seat-branch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();
        git(&path, &["init", "-q", "-b", "dev"]).unwrap();
        git(&path, &["commit", "-q", "--allow-empty", "-m", "one"]).unwrap();
        (dir, path)
    }

    #[test]
    fn the_previous_default_is_project_seat_home() {
        assert_eq!(previous_default("p", "hull"), "p/hull/home");
        assert_ne!(
            previous_default("p", "hull"),
            gear_model::branch::expand_seat(gear_model::branch::SEAT_BRANCH, "p", "hull", "M2")
        );
    }

    #[test]
    fn a_seat_on_the_old_default_is_renamed_to_the_new_one() {
        let (_dir, path) = repo("rename");
        git(&path, &["branch", "p/hull/home"]).unwrap();

        let settled = settle(&path, "p/hull/home", "p/hull/M2", "origin").unwrap();
        assert!(
            matches!(&settled, Settled::Renamed { old, new, .. } if old == "p/hull/home" && new == "p/hull/M2"),
            "{settled:?}"
        );
        assert_eq!(
            git(&path, &["rev-parse", "--verify", "refs/heads/p/hull/M2"]).is_ok(),
            true
        );
        assert_eq!(
            git(&path, &["rev-parse", "--verify", "refs/heads/p/hull/home"]).is_ok(),
            false,
            "the old name is gone: it is never left beside the new one"
        );
    }

    #[test]
    fn a_foreign_ref_in_the_way_is_named_and_left_alone() {
        let (_dir, path) = repo("foreign");
        git(&path, &["branch", "p/hull"]).unwrap();
        // The seat's branch is another one: `p/hull` is not its old name.
        let error = settle(&path, "p/hull/home", "p/hull/M2", "origin").unwrap_err();
        assert!(error.contains("p/hull"), "{error}");
        assert_eq!(
            git(&path, &["rev-parse", "--verify", "refs/heads/p/hull"]).is_ok(),
            true,
            "the foreign ref is untouched"
        );
    }

    #[test]
    fn a_branch_checked_out_in_a_worktree_is_renamed_from_there() {
        let (dir, path) = repo("checked-out");
        let worktree = dir.join("seat");
        git(
            &path,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "p/hull",
                worktree.to_str().unwrap(),
                "dev",
            ],
        )
        .unwrap();

        let settled = settle(worktree.to_str().unwrap(), "p/hull", "p/hull/M2", "origin").unwrap();
        assert!(matches!(settled, Settled::Renamed { .. }), "{settled:?}");
        assert_eq!(
            git(&path, &["rev-parse", "--abbrev-ref", "HEAD"]),
            Ok("dev".to_string())
        );
        assert_eq!(
            git(
                worktree.to_str().unwrap(),
                &["rev-parse", "--abbrev-ref", "HEAD"]
            ),
            Ok("p/hull/M2".to_string())
        );
    }

    #[test]
    fn a_free_name_settles_nothing() {
        let (_dir, path) = repo("free");
        assert_eq!(
            settle(&path, "p/hull/home", "p/hull/M2", "origin").unwrap(),
            Settled::Free
        );
        assert_eq!(
            settle(&path, "p/hull/home", "p/hull/home", "origin").unwrap(),
            Settled::Free
        );
    }
}
