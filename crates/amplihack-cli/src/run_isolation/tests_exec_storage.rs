//! Unit tests for `exec_storage.rs` (issue #2785): a worktree registered in
//! the storage's private repository that lies inside the storage keeps it
//! while it holds work.
//!
//! Contract:
//! - `pub(crate) type HoldsWork = fn(&Path) -> Option<String>`, passed in by
//!   the caller as `OwnerEnded` is; this module does not depend on cleanup;
//! - `fn live_dependant_with(stable: &Path, holds_work: HoldsWork, limit:
//!   usize) -> Result<Option<String>>` gives the reason the storage is kept,
//!   or `None`:
//!   - a registered worktree outside `stable` that still exists gives
//!     `durable worktree <path> still uses its repository`, as before;
//!   - one inside `stable`, other than `<stable>/checkout`, gives
//!     `worktree "<path>" inside it <reason>` when `holds_work` returns
//!     `Some(reason)`;
//!   - past `limit` calls of `holds_work` the storage is kept with
//!     `more than <limit> worktrees inside it to check`;
//!   - `<stable>/checkout` is left out by an exact comparison of canonical
//!     paths, so a worktree nested below it is still checked.
//!
//! The registrations are written by hand, as `git worktree add` writes them,
//! so no git runs here.

use super::*;

fn never(_: &Path) -> Option<String> {
    None
}

fn always(_: &Path) -> Option<String> {
    Some("has uncommitted changes".to_string())
}

/// A canonical storage folder with an empty `repository.git/worktrees/`.
fn storage() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let stable = fs::canonicalize(tmp.path()).unwrap().join("identity");
    fs::create_dir_all(stable.join("repository.git/worktrees")).unwrap();
    (tmp, stable)
}

/// Registers a worktree at `tree` under `name`: its `.git` file names the
/// admin folder, and the admin folder's `gitdir` names the `.git` file.
fn register(stable: &Path, name: &str, tree: &Path) {
    let admin = stable.join("repository.git/worktrees").join(name);
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(tree).unwrap();
    fs::write(tree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", tree.join(".git").display()),
    )
    .unwrap();
}

#[test]
fn a_worktree_inside_the_storage_that_holds_work_keeps_it() {
    let (_tmp, stable) = storage();
    let feat = stable.join("worktrees/feat");
    register(&stable, "feat", &feat);

    assert_eq!(
        live_dependant_with(&stable, always, 64).unwrap(),
        Some(format!(
            "worktree {feat:?} inside it has uncommitted changes"
        ))
    );
    assert_eq!(
        live_dependant_with(&stable, never, 64).unwrap(),
        None,
        "a worktree inside the storage with no work does not keep it"
    );
}

#[test]
fn the_private_checkout_is_not_checked() {
    let (_tmp, stable) = storage();
    register(&stable, "checkout", &stable.join("checkout"));

    assert_eq!(live_dependant_with(&stable, always, 64).unwrap(), None);
}

#[test]
fn a_worktree_nested_below_the_checkout_is_checked() {
    let (_tmp, stable) = storage();
    register(&stable, "checkout", &stable.join("checkout"));
    let nested = stable.join("checkout/nested");
    register(&stable, "nested", &nested);

    assert_eq!(
        live_dependant_with(&stable, always, 64).unwrap(),
        Some(format!(
            "worktree {nested:?} inside it has uncommitted changes"
        ))
    );
}

#[test]
fn more_worktrees_inside_than_the_limit_keep_the_storage() {
    let (_tmp, stable) = storage();
    register(&stable, "a", &stable.join("worktrees/a"));
    register(&stable, "b", &stable.join("worktrees/b"));

    assert_eq!(
        live_dependant_with(&stable, never, 1).unwrap(),
        Some("more than 1 worktrees inside it to check".to_string())
    );
    assert_eq!(live_dependant_with(&stable, never, 2).unwrap(), None);
}

#[test]
fn a_worktree_outside_the_storage_keeps_it_whatever_it_holds() {
    let (tmp, stable) = storage();
    let durable = fs::canonicalize(tmp.path()).unwrap().join("durable");
    register(&stable, "durable", &durable);

    assert_eq!(
        live_dependant_with(&stable, never, 64).unwrap(),
        Some(format!(
            "durable worktree {} still uses its repository",
            durable.display()
        ))
    );
}

#[test]
fn a_registered_worktree_that_is_gone_is_ignored() {
    let (_tmp, stable) = storage();
    let gone = stable.join("worktrees/gone");
    register(&stable, "gone", &gone);
    fs::remove_dir_all(&gone).unwrap();

    assert_eq!(live_dependant_with(&stable, always, 64).unwrap(), None);
}
