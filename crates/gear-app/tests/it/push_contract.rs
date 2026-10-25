//! T-73: `init` freezes where each repo may be pushed (trial G12).
//!
//! The destinations are read from the repo's own `origin` remote with real
//! git — a pushurl and an `insteadOf` rewrite included — and the check
//! passes the registered one and refuses every other.

use std::{path::Path, path::PathBuf, process::Command};

use gear_app::admin::init_with_home;
use gear_app::push::{self, Reason};
use gear_app::Env;
use gear_store::Store;

const T0: i64 = 1_800_000_000;

/// Run a shell command in `dir`; it must succeed.
fn sh(dir: &Path, command: &str) -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{command} in {}: {}{}",
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The real Git world of the test:
///
/// - the project root: a local origin to fetch from and a pushurl that an
///   `insteadOf` rewrite sends to another host;
/// - a child repo with its own local origin;
/// - a repo with no remote at all.
struct World {
    _root: tempfile::TempDir,
    parent: PathBuf,
    child_origin: PathBuf,
}

impl World {
    fn new() -> World {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("parent");
        let parent_origin = root.path().join("parent-origin.git");
        let child = parent.join("child");
        let child_origin = root.path().join("child-origin.git");
        let orphan = parent.join("orphan");

        // The project root.
        sh(root.path(), &format!("git init -q --bare {}", parent_origin.display()));
        sh(root.path(), &format!("git init -q -b dev {}", parent.display()));
        sh(&parent, "printf 'child/\\norphan/\\n' > .gitignore");
        sh(
            &parent,
            &format!("git remote add origin {}", parent_origin.display()),
        );
        // The pushurl is a GitHub address that a rewrite sends elsewhere:
        // what the project may push to is where the push *goes*, not what
        // the remote is written as.
        sh(
            &parent,
            "git remote set-url --push origin git@github.com:owner/parent.git",
        );
        sh(
            &parent,
            "git config --local url.\"https://ext.example.com/\".insteadOf \"git@github.com:\"",
        );

        // The child repo: its own local origin.
        sh(root.path(), &format!("git init -q --bare {}", child_origin.display()));
        sh(root.path(), &format!("git init -q -b feat {}", child.display()));
        sh(
            &child,
            &format!("git remote add origin {}", child_origin.display()),
        );

        // A repo with no remote: nothing may be pushed to it.
        sh(root.path(), &format!("git init -q -b main {}", orphan.display()));

        World {
            _root: root,
            parent,
            child_origin,
        }
    }

    /// The project config: the parent is the project, the child and the
    /// orphan are repos of it.
    fn config(&self) -> String {
        format!(
            r#"
[project]
id = "p"
herdr_session = "sess"
repo = "{parent}"

[[repos]]
name = "child"
path = "child"

[[repos]]
name = "orphan"
path = "orphan"

[quota]
lanes = 1
temps = 0

[captain]
pane = "w1:p1"
kind = "opencode"
profile = "opencode"

[[lanes]]
name = "heat"
duty = "everything"
owns = ["src/"]
kind = "opencode"
profile = "opencode"
repo = "child"
"#,
            parent = self.parent.display(),
        )
    }

    /// The child's origin, as a URL.
    fn child_origin(&self) -> String {
        self.child_origin.display().to_string()
    }

    /// The path of the child repo.
    fn child_path(&self) -> PathBuf {
        self.parent.join("child")
    }
}

fn admin() -> Env {
    Env { ctx: None, pane_id: None, admin: true }
}

/// The frozen destinations of one repo.
fn destinations_of(store: &mut Store, repo: &str) -> Vec<String> {
    store.read(|tx| tx.push_destinations("p", repo)).unwrap()
}

/// The frozen destinations of the project: (repo, destination).
fn all_destinations(store: &mut Store) -> Vec<(String, String)> {
    store
        .read(|tx| tx.push_destinations_of_project("p"))
        .unwrap()
        .into_iter()
        .map(|row| (row.repo, row.destination))
        .collect()
}

#[test]
fn init_freezes_the_destinations_of_every_repo() {
    let world = World::new();
    let mut store = Store::open_in_memory().unwrap();

    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0).unwrap();

    // The project root: the pushurl, after the rewrite — the place the push
    // goes, not the address the remote is written as.
    assert_eq!(
        destinations_of(&mut store, ""),
        vec!["ext.example.com/owner/parent".to_string()]
    );
    // The child repo: its own local origin.
    assert_eq!(
        destinations_of(&mut store, "child"),
        vec![world.child_origin()]
            .into_iter()
            .map(|url| push::normalise(&url).unwrap())
            .collect::<Vec<_>>()
    );
    // The repo with no remote: nothing may be pushed to it.
    assert!(destinations_of(&mut store, "orphan").is_empty());

    // Every row of the project, by repo.
    assert_eq!(all_destinations(&mut store).len(), 2, "{:?}", all_destinations(&mut store));
}

#[test]
fn the_check_passes_the_registered_destination_and_refuses_the_others() {
    let world = World::new();
    let mut store = Store::open_in_memory().unwrap();
    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0).unwrap();

    // The registered one: where the pushurl resolves to.
    assert!(push::check(
        &mut store,
        "p",
        "",
        "https://ext.example.com/owner/parent.git"
    )
    .is_ok());

    // Another host.
    let refusal = push::check(&mut store, "p", "", "https://gitlab.com/owner/parent.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::NotRegistered);
    assert_eq!(refusal.repo, "");
    assert_eq!(refusal.destination, "gitlab.com/owner/parent");
    assert_eq!(refusal.host(), "gitlab.com");
    assert!(refusal.message().contains("gitlab.com/owner/parent"), "{}", refusal.message());

    // The rewritten external URL: the address the remote is written as is
    // not the place the push goes, so it is refused.
    let refusal = push::check(&mut store, "p", "", "git@github.com:owner/parent.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::NotRegistered);
    assert_eq!(refusal.destination, "github.com/owner/parent");

    // Another external URL of the rewrite's host, never registered.
    let refusal = push::check(&mut store, "p", "", "https://ext.example.com/owner/other.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::NotRegistered);

    // An unknown repo.
    let refusal =
        push::check(&mut store, "p", "nosuch", "https://ext.example.com/owner/parent.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::UnknownRepo);
    assert_eq!(refusal.repo, "nosuch");

    // A repo with no destination: nothing may be pushed to it.
    let refusal =
        push::check(&mut store, "p", "orphan", "https://ext.example.com/owner/parent.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::NoDestinations);

    // A project that is not registered.
    let refusal =
        push::check(&mut store, "nosuch", "", "https://ext.example.com/owner/parent.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::UnknownProject);

    // The child repo: its own origin, and nothing else.
    assert!(push::check(&mut store, "p", "child", &world.child_origin()).is_ok());
    let refusal = push::check(
        &mut store,
        "p",
        "child",
        &world.child_origin().replace("child-origin", "other-origin"),
    )
    .unwrap_err();
    assert_eq!(refusal.reason, Reason::NotRegistered);
}

#[test]
fn a_repo_may_be_named_by_its_path() {
    // A caller that knows where the push happens need not know the name: the
    // path of a repo names it too.
    let world = World::new();
    let mut store = Store::open_in_memory().unwrap();
    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0).unwrap();

    // The project root by its path.
    assert!(push::check(
        &mut store,
        "p",
        &world.parent.display().to_string(),
        "https://ext.example.com/owner/parent.git"
    )
    .is_ok());
    // A child repo by its path.
    assert!(push::check(
        &mut store,
        "p",
        &world.child_path().display().to_string(),
        &world.child_origin()
    )
    .is_ok());
    // A path that is not a repo of the project is refused.
    let refusal = push::check(&mut store, "p", &world.child_origin(), &world.child_origin())
        .unwrap_err();
    assert_eq!(refusal.reason, Reason::UnknownRepo);
}

#[test]
fn every_init_reads_the_destinations_again() {
    let world = World::new();
    let mut store = Store::open_in_memory().unwrap();
    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0).unwrap();
    assert_eq!(
        destinations_of(&mut store, ""),
        vec!["ext.example.com/owner/parent".to_string()]
    );

    // The project moves its pushurl: the next init reads the new place.
    sh(
        &world.parent,
        "git remote set-url --push origin git@github.com:owner/moved.git",
    );
    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0 + 1).unwrap();
    assert_eq!(
        destinations_of(&mut store, ""),
        vec!["ext.example.com/owner/moved".to_string()]
    );

    // A repo that leaves the config leaves its destinations behind: the
    // check refuses it as an unknown repo anyway.
    let config = world
        .config()
        .replace("[[repos]]\nname = \"child\"\npath = \"child\"\n\n", "")
        .replace("repo = \"child\"\n", "");
    init_with_home(&mut store, &admin(), &config,Some(crate::skill_home()), T0 + 2).unwrap();
    assert!(destinations_of(&mut store, "child").is_empty(), "the child's destinations are gone");
    let refusal = push::check(&mut store, "p", "child", &world.child_origin()).unwrap_err();
    assert_eq!(refusal.reason, Reason::UnknownRepo);
}

/// T-73 F1 (rework): an unregistered IPv6 host must not pass the allow-list
/// because the registered one shares its path. The authority is not cut at
/// its first colon: the whole bracketed host is the destination.
#[test]
fn two_ipv6_hosts_of_one_path_are_two_destinations() {
    let world = World::new();
    // The project root pushes to a bracketed IPv6 host. No network is
    // touched: `init` only reads the remote's configuration.
    sh(
        &world.parent,
        "git remote set-url --push origin 'ssh://git@[2001:db8::1]/team/repo.git'",
    );
    let mut store = Store::open_in_memory().unwrap();
    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0).unwrap();

    // The frozen destination carries the whole host — not `[2001/team/repo`,
    // which two different hosts would share.
    assert_eq!(
        destinations_of(&mut store, ""),
        vec!["[2001:db8::1]/team/repo".to_string()]
    );

    // The registered host passes, in either form.
    assert!(push::check(&mut store, "p", "", "ssh://git@[2001:db8::1]/team/repo.git").is_ok());
    assert!(push::check(&mut store, "p", "", "git@[2001:db8::1]:team/repo.git").is_ok());
    // A port is not the host: the same host on another port is the same place.
    assert!(push::check(&mut store, "p", "", "ssh://git@[2001:db8::1]:2222/team/repo.git").is_ok());

    // Another IPv6 host of the same path is refused — the bypass F1 found.
    let refusal =
        push::check(&mut store, "p", "", "ssh://git@[2001:db8::2]/team/repo.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::NotRegistered);
    assert_eq!(refusal.destination, "[2001:db8::2]/team/repo");
    assert!(refusal.message().contains("[2001:db8::2]/team/repo"), "{}", refusal.message());
}

/// T-73 F2 (rework): a Unicode path is a destination, not a panic — in
/// `init` and in `check` alike.
#[test]
fn a_unicode_destination_is_frozen_and_checked() {
    let world = World::new();
    sh(
        &world.parent,
        "git remote set-url --push origin 'https://git.example.com/团队/repo.git'",
    );
    let mut store = Store::open_in_memory().unwrap();
    init_with_home(&mut store, &admin(), &world.config(),Some(crate::skill_home()), T0).unwrap();

    assert_eq!(
        destinations_of(&mut store, ""),
        vec!["git.example.com/团队/repo".to_string()]
    );
    assert!(push::check(&mut store, "p", "", "https://git.example.com/团队/repo.git").is_ok());
    let refusal =
        push::check(&mut store, "p", "", "https://git.example.com/团队/other.git").unwrap_err();
    assert_eq!(refusal.reason, Reason::NotRegistered);
}
