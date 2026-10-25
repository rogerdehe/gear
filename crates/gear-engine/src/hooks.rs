//! GEAR's pre-push check in every worktree GEAR makes (T-72).
//!
//! A GEAR worktree pushes only to the destinations the project registered with
//! `gear admin init` (T-73). The check has two halves: a hook in the worktree,
//! so a lane's or a reviewer's own `git push` obeys it, and the same check
//! in-process before the engine's merge push (in `merge.rs`).
//!
//! The hook is installed for **this worktree only**: `git config --worktree
//! core.hooksPath` points at a directory inside the worktree's own git dir, so
//! the main checkout and every other worktree keep their hooks. The script runs
//! `gear push-check "$1" "$2"` first, then the repository's own `pre-push` hook
//! if there is one, so a project's own checks keep running.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Install GEAR's pre-push check in the worktree at `worktree`.
///
/// Idempotent: a worktree GEAR reopens gets the hook rewritten, with the same
/// repository hook chained. A worktree whose hook cannot be installed is an
/// error — an unprotected worktree may not run.
pub fn install(worktree: &Path) -> Result<(), String> {
    let git_dir = match git(worktree, &["rev-parse", "--absolute-git-dir"]) {
        Ok(dir) => PathBuf::from(dir.trim()),
        // A worktree that is not a git repository has nothing to protect: the
        // engine's own merge check still guards the landing (T-72), and a
        // stand-in worktree (a test's fake) must not stop a lane.
        Err(_) => return Ok(()),
    };
    let hooks = git_dir.join("gear-hooks");
    std::fs::create_dir_all(&hooks)
        .map_err(|e| format!("cannot create {}: {e}", hooks.display()))?;

    // The repository's own pre-push hook: its `core.hooksPath` when the repo
    // sets one, else the shared hooks directory. Read from the repository's own
    // config (`--local`), so GEAR's own worktree override never hides it.
    let own = match git(worktree, &["config", "--local", "--get", "core.hooksPath"]) {
        Ok(path) if !path.trim().is_empty() => {
            let path = PathBuf::from(path.trim());
            let path = if path.is_absolute() { path } else { worktree.join(path) };
            path.join("pre-push")
        }
        _ => {
            let common = git(worktree, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
            PathBuf::from(common.trim()).join("hooks").join("pre-push")
        }
    };

    let script = format!(
        "#!/bin/sh\n\
         # GEAR's pre-push check (T-72): this worktree pushes only to the\n\
         # destinations the project registered (`gear admin init` froze them).\n\
         # The repository's own pre-push hook runs after it.\n\
         gear push-check \"$1\" \"$2\" || exit 1\n\
         own='{own}'\n\
         if [ -x \"$own\" ]; then exec \"$own\" \"$1\" \"$2\"; fi\n\
         exit 0\n",
        own = own.display()
    );
    let path = hooks.join("pre-push");
    std::fs::write(&path, script).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut perms = std::fs::metadata(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms)
        .map_err(|e| format!("cannot make {} executable: {e}", path.display()))?;

    // The worktree-only override needs the extension the git way of setting one
    // per worktree depends on; it is a repository setting, and setting it twice
    // is harmless.
    git(worktree, &["config", "extensions.worktreeConfig", "true"])?;
    git(
        worktree,
        &["config", "--worktree", "core.hooksPath", &hooks.to_string_lossy()],
    )?;
    Ok(())
}

/// Is the pre-push check in place in this worktree? (The test reads it; a lane
/// never needs to.)
pub fn installed(worktree: &Path) -> bool {
    let Ok(hooks) = git(worktree, &["config", "--worktree", "--get", "core.hooksPath"]) else {
        return false;
    };
    let hooks = PathBuf::from(hooks.trim());
    hooks.join("pre-push").is_file()
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
