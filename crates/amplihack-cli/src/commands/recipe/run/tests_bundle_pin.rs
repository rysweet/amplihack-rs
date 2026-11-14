//! Issue #2528: one bundle per recipe run. Unit tests for `bundle_pin.rs`.
//!
//! Wire with, in `run/mod.rs`:
//! `mod bundle_pin;` and
//! `#[cfg(test)] #[path = "tests_bundle_pin.rs"] mod tests_bundle_pin;`
//!
//! Contract (docs/reference/recipe-run-bundle.md):
//! - `BundleChoice { Installed, Checkout }` with `parse`, `from_inputs`, `as_str`;
//!   derives `Debug, Clone, Copy, PartialEq, Eq`.
//! - `SelectInputs<'a>` with the `pub(crate)` fields used by `inputs()` below,
//!   including `work_root: Option<&Path>`, the run's work root (the folder
//!   that holds `runs/` and `bundle-pins/`). A marker is accepted only when
//!   its canonical parent is `<work_root>/bundle-pins`; with no work root,
//!   every marker is ignored. `select_from` reads nothing from the process
//!   environment.
//! - `Selection` with `source()` (canonical `<home>/amplifier-bundle`),
//!   `recipes_dir()`, `choice()`, `version()`, `inherited()` and `notes()`
//!   (stderr notes, such as a skipped checkout or an ignored marker).
//! - `pin(&Selection, work_root, dry_run) -> PinnedBundle` with `home()`
//!   (`<work_root>/bundle-pins/<24 hex key>`), `bundle()`, `source()` and
//!   `record()`. Dropping the `PinnedBundle` releases its lease on
//!   `<key>.use`; the last launch lease to go removes the snapshot.
//! - `is_snapshot_path(path, work_root)`: true only for
//!   `<work_root>/bundle-pins/<24 lower-case hex>`.
//! - `check_state_home(state_home, &PinnedBundle, runtime_dir)`: E8.
//!
//! Every test builds its own temporary homes; none reads or changes the
//! process environment, the real `~/.amplihack` or the working directory.
//! No test waits on a clock: lock contention is created and checked with
//! non-blocking `flock` calls only.

#![cfg(unix)]

use super::bundle_pin::{
    BundleChoice, SelectInputs, check_state_home, is_snapshot_path, pin, select_from,
};
use super::correlation::BundleRecord;
use fs4::fs_std::FileExt;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- fixtures

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Writes `<home>/amplifier-bundle` with a recipe, an agent and a tool whose
/// output names `label`. Directories 0755, files 0644, the tool 0755.
fn write_bundle(home: &Path, label: &str) -> PathBuf {
    let bundle = home.join("amplifier-bundle");
    for dir in ["recipes", "agents/core", "tools"] {
        fs::create_dir_all(bundle.join(dir)).unwrap();
    }
    fs::write(
        bundle.join("recipes/probe.yaml"),
        format!("name: probe\ndescription: {label}\nsteps: []\n"),
    )
    .unwrap();
    fs::write(
        bundle.join("agents/core/builder.md"),
        format!("# {label}\n"),
    )
    .unwrap();
    let tool = bundle.join("tools/identity.sh");
    fs::write(&tool, format!("#!/bin/sh\necho {label}\n")).unwrap();
    for file in ["recipes/probe.yaml", "agents/core/builder.md"] {
        set_mode(&bundle.join(file), 0o644);
    }
    set_mode(&tool, 0o755);
    for dir in ["", "recipes", "agents", "agents/core", "tools"] {
        set_mode(&bundle.join(dir), 0o755);
    }
    set_mode(home, 0o755);
    bundle
}

/// A checkout root: `.git/` plus a bundle.
fn write_checkout(root: &Path, label: &str) -> PathBuf {
    fs::create_dir_all(root.join(".git")).unwrap();
    write_bundle(root, label)
}

/// A private temporary root. On drop, the owner write bit is restored on
/// every real directory first (symlinks are never followed), so read-only
/// snapshots left by a test do not stop the temporary root's removal.
struct Scratch(Option<tempfile::TempDir>);

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(tmp) = self.0.take() {
            restore_owner_write(tmp.path());
        }
    }
}

/// Moves the snapshot folder `from` to `to` in another folder, keeping its
/// mode. Moving a folder to a new parent needs write permission on the
/// folder itself, and a placed snapshot is read-only.
fn move_snapshot(from: &Path, to: &Path) {
    let mode = mode_of(from);
    set_mode(from, mode | 0o200);
    fs::rename(from, to).unwrap();
    set_mode(to, mode);
}

fn restore_owner_write(dir: &Path) {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let _ = fs::set_permissions(
        dir,
        fs::Permissions::from_mode(meta.permissions().mode() | 0o700),
    );
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            restore_owner_write(&entry.path());
        }
    }
}

fn scratch() -> (Scratch, PathBuf) {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let root = fs::canonicalize(tmp.path()).unwrap();
    (Scratch(Some(tmp)), root)
}

fn os(path: &Path) -> Option<&OsStr> {
    Some(path.as_os_str())
}

/// Inputs with no candidates, no marker, no choice and no work root.
fn inputs(working_dir: &Path) -> SelectInputs<'_> {
    SelectInputs {
        choice: None,
        working_dir,
        amplihack_home: None,
        configured_home: None,
        user_home: None,
        run_bundle: None,
        run_bundle_choice: None,
        work_root: None,
    }
}

fn err_text(result: anyhow::Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.expect_err("expected an error"))
}

// ---------------------------------------------------------- BundleChoice

#[test]
fn choice_parses_exact_lower_case_values_only() {
    assert_eq!(
        BundleChoice::parse("installed").unwrap(),
        BundleChoice::Installed
    );
    assert_eq!(
        BundleChoice::parse("checkout").unwrap(),
        BundleChoice::Checkout
    );
    for bad in [
        "Checkout",
        "INSTALLED",
        "",
        " checkout",
        "path",
        "checkout\n",
    ] {
        assert!(BundleChoice::parse(bad).is_err(), "{bad:?} must be refused");
    }
    assert_eq!(BundleChoice::Installed.as_str(), "installed");
    assert_eq!(BundleChoice::Checkout.as_str(), "checkout");
}

#[test]
fn from_inputs_matrix() {
    use BundleChoice::*;
    assert_eq!(BundleChoice::from_inputs(None, None).unwrap(), None);
    assert_eq!(
        BundleChoice::from_inputs(Some("installed"), None).unwrap(),
        Some(Installed)
    );
    assert_eq!(
        BundleChoice::from_inputs(None, Some("checkout")).unwrap(),
        Some(Checkout)
    );
    assert_eq!(
        BundleChoice::from_inputs(Some("checkout"), Some("checkout")).unwrap(),
        Some(Checkout)
    );
}

#[test]
fn flag_and_key_that_differ_are_refused_with_e1() {
    assert_eq!(
        err_text(BundleChoice::from_inputs(
            Some("installed"),
            Some("checkout")
        )),
        "--bundle installed conflicts with -c bundle=checkout; give one value"
    );
}

#[test]
fn key_with_another_value_is_refused_with_e2() {
    assert_eq!(
        err_text(BundleChoice::from_inputs(None, Some("Checkout"))),
        "-c bundle=Checkout is not valid; use installed or checkout"
    );
    // The key is validated even when the flag is given.
    assert!(BundleChoice::from_inputs(Some("installed"), Some("bogus")).is_err());
}

// ------------------------------------------------------ installed choice

#[test]
fn default_uses_the_installed_bundle_even_inside_a_checkout() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    write_checkout(&checkout, "checkout-B");
    let working_dir = checkout.join("crates/sub");
    fs::create_dir_all(&working_dir).unwrap();
    let installed = root.join("installed");
    let bundle = write_bundle(&installed, "installed-A");

    let mut i = inputs(&working_dir);
    i.amplihack_home = os(&installed);
    let selection = select_from(i).unwrap();

    assert_eq!(selection.source(), fs::canonicalize(&bundle).unwrap());
    assert_eq!(selection.recipes_dir(), selection.source().join("recipes"));
    assert_eq!(selection.choice(), BundleChoice::Installed);
    assert!(!selection.inherited());
}

#[test]
fn installed_never_walks_up_from_the_working_directory() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    write_checkout(&checkout, "checkout-B");
    let user_home = root.join("user");
    fs::create_dir_all(&user_home).unwrap();

    let selected = select_from(SelectInputs {
        user_home: os(&user_home),
        ..inputs(&checkout)
    });
    let text = err_text(selected);
    assert!(
        text.starts_with("no installed amplifier-bundle found; searched: "),
        "E4 expected, got: {text}"
    );
    assert!(
        text.ends_with("To use this checkout's bundle, pass --bundle checkout"),
        "{text}"
    );
    assert!(
        text.contains(&user_home.join(".amplihack").display().to_string()),
        "{text}"
    );
}

#[test]
fn amplihack_home_that_is_a_checkout_is_skipped_with_a_note() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    write_checkout(&checkout, "checkout-B");
    let user_home = root.join("user");
    let installed = write_bundle(&user_home.join(".amplihack"), "installed-A");

    let selection = select_from(SelectInputs {
        amplihack_home: os(&checkout),
        user_home: os(&user_home),
        ..inputs(&checkout)
    })
    .unwrap();

    assert_eq!(selection.source(), fs::canonicalize(&installed).unwrap());
    let notes = selection.notes().join("\n");
    assert!(
        notes.contains(&format!(
            "recipe bundle: skipping AMPLIHACK_HOME={}: it is a git checkout; \
             pass --bundle checkout to use its bundle",
            checkout.display()
        )),
        "notes: {notes}"
    );
}

#[test]
fn a_dot_git_file_also_marks_a_checkout_and_is_not_parsed() {
    let (_tmp, root) = scratch();
    let worktree = root.join("worktree");
    write_bundle(&worktree, "worktree-B");
    // A linked worktree: `.git` is a file. Its text points nowhere real.
    fs::write(worktree.join(".git"), "gitdir: /nonexistent/elsewhere\n").unwrap();
    let configured = root.join("configured");
    let installed = write_bundle(&configured, "installed-A");

    let selection = select_from(SelectInputs {
        amplihack_home: os(&worktree),
        configured_home: Some(&configured),
        ..inputs(&root)
    })
    .unwrap();
    assert_eq!(selection.source(), fs::canonicalize(&installed).unwrap());
}

#[test]
fn installed_candidates_are_tried_in_order() {
    let (_tmp, root) = scratch();
    let env_home = root.join("env-home");
    let configured = root.join("configured");
    let user_home = root.join("user");
    let a = write_bundle(&env_home, "A");
    let b = write_bundle(&configured, "B");
    let c = write_bundle(&user_home.join(".amplihack"), "C");

    let all = SelectInputs {
        amplihack_home: os(&env_home),
        configured_home: Some(&configured),
        user_home: os(&user_home),
        ..inputs(&root)
    };
    assert_eq!(
        select_from(all).unwrap().source(),
        fs::canonicalize(&a).unwrap()
    );

    let no_env = SelectInputs {
        configured_home: Some(&configured),
        user_home: os(&user_home),
        ..inputs(&root)
    };
    assert_eq!(
        select_from(no_env).unwrap().source(),
        fs::canonicalize(&b).unwrap()
    );

    // A candidate that does not exist is passed over silently.
    let missing = root.join("missing");
    let only_user = SelectInputs {
        amplihack_home: os(&missing),
        user_home: os(&user_home),
        ..inputs(&root)
    };
    let selection = select_from(only_user).unwrap();
    assert_eq!(selection.source(), fs::canonicalize(&c).unwrap());
    assert!(selection.notes().is_empty(), "{:?}", selection.notes());
}

#[test]
fn e4_lists_every_candidate_and_marks_skipped_checkouts() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    write_checkout(&checkout, "B");
    let configured = root.join("configured-empty");
    fs::create_dir_all(&configured).unwrap();
    let user_home = root.join("user");
    fs::create_dir_all(&user_home).unwrap();

    let text = err_text(select_from(SelectInputs {
        amplihack_home: os(&checkout),
        configured_home: Some(&configured),
        user_home: os(&user_home),
        ..inputs(&root)
    }));
    assert!(
        text.contains(&format!("{} (git checkout, skipped)", checkout.display())),
        "{text}"
    );
    assert!(text.contains(&configured.display().to_string()), "{text}");
    assert!(
        text.contains(&user_home.join(".amplihack").display().to_string()),
        "{text}"
    );
}

#[test]
fn a_release_link_is_canonicalised_once_at_selection() {
    let (_tmp, root) = scratch();
    let home = root.join("home");
    let release = home.join("releases/1.0.0");
    write_bundle(&release, "A");
    fs::write(release.join(".complete"), "").unwrap();
    fs::write(
        release.join(".installed-by"),
        "format=1\nversion=1.0.0\nexe=\npid=1\npost_update=0\n",
    )
    .unwrap();
    set_mode(&release, 0o755);
    set_mode(&home.join("releases"), 0o755);
    set_mode(&home, 0o755);
    symlink(
        release.join("amplifier-bundle"),
        home.join("amplifier-bundle"),
    )
    .unwrap();

    let selection = select_from(SelectInputs {
        amplihack_home: os(&home),
        ..inputs(&root)
    })
    .unwrap();
    assert_eq!(
        selection.source(),
        fs::canonicalize(release.join("amplifier-bundle")).unwrap()
    );
    assert_eq!(selection.version(), "1.0.0");
}

// ------------------------------------------------------- checkout choice

#[test]
fn checkout_uses_the_nearest_ancestor_with_git_and_a_bundle() {
    let (_tmp, root) = scratch();
    let outer = root.join("outer");
    write_checkout(&outer, "outer");
    let inner = outer.join("nested/inner");
    let inner_bundle = write_checkout(&inner, "inner");
    let working_dir = inner.join("deep/dir");
    fs::create_dir_all(&working_dir).unwrap();
    let installed = root.join("installed");
    write_bundle(&installed, "installed");

    let selection = select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        amplihack_home: os(&installed),
        ..inputs(&working_dir)
    })
    .unwrap();
    assert_eq!(selection.source(), fs::canonicalize(&inner_bundle).unwrap());
    assert_eq!(selection.choice(), BundleChoice::Checkout);
}

#[test]
fn checkout_passes_over_an_ancestor_with_only_one_of_the_two_entries() {
    let (_tmp, root) = scratch();
    let outer = root.join("outer");
    let outer_bundle = write_checkout(&outer, "outer");
    // A nested git repository without a bundle, then a bundle without .git.
    let no_bundle = outer.join("vendored");
    fs::create_dir_all(no_bundle.join(".git")).unwrap();
    let no_git = no_bundle.join("pkg");
    write_bundle(&no_git, "not-a-checkout");

    let selection = select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        ..inputs(&no_git)
    })
    .unwrap();
    assert_eq!(selection.source(), fs::canonicalize(&outer_bundle).unwrap());
}

#[test]
fn checkout_with_a_dot_git_file_is_accepted_without_parsing_it() {
    let (_tmp, root) = scratch();
    let worktree = root.join("wt");
    let bundle = write_bundle(&worktree, "wt");
    fs::write(worktree.join(".git"), "gitdir: /nonexistent/x\n").unwrap();
    let selection = select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        ..inputs(&worktree)
    })
    .unwrap();
    assert_eq!(selection.source(), fs::canonicalize(&bundle).unwrap());
}

#[test]
fn no_checkout_bundle_is_e5() {
    let (_tmp, root) = scratch();
    let working_dir = root.join("plain");
    fs::create_dir_all(&working_dir).unwrap();
    assert_eq!(
        err_text(select_from(SelectInputs {
            choice: Some(BundleChoice::Checkout),
            ..inputs(&working_dir)
        })),
        format!(
            "--bundle checkout: no directory containing both .git and amplifier-bundle/ above {}",
            working_dir.display()
        )
    );
}
#[test]
fn a_group_writable_file_is_refused_with_e6_naming_file_and_mode() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    let bundle = write_checkout(&checkout, "B");
    let bad = bundle.join("tools/identity.sh");
    set_mode(&bad, 0o775);
    let selection = select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        ..inputs(&checkout)
    })
    .unwrap();
    let work = work_root(&root);

    for dry_run in [true, false] {
        let text = err_text(pin(&selection, &work, dry_run));
        let canonical = fs::canonicalize(&bad).unwrap();
        assert!(
            text.contains(&format!(
                "untrusted bundle entry {}: mode 0775 (group or other writable); \
                 fix with chmod go-w {}",
                canonical.display(),
                canonical.display()
            )),
            "dry_run={dry_run}: {text}"
        );
    }
    // A refused launch leaves no snapshot, no lease and no copy in progress.
    let left: Vec<String> = pin_entries(&work)
        .into_iter()
        .filter(|name| name != ".lock")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

// ------------------------------------------------------------ version

fn version_of_installed(installed_by: Option<&[u8]>) -> String {
    let (_tmp, root) = scratch();
    let home = root.join("home");
    write_bundle(&home, "A");
    if let Some(body) = installed_by {
        fs::write(home.join(".installed-by"), body).unwrap();
    }
    select_from(SelectInputs {
        amplihack_home: os(&home),
        ..inputs(&root)
    })
    .unwrap()
    .version()
    .to_owned()
}

#[test]
fn version_comes_from_installed_by_next_to_the_bundle() {
    assert_eq!(
        version_of_installed(Some(b"format=1\nversion=1.2.151\npid=4\n")),
        "1.2.151"
    );
}

#[test]
fn version_is_unknown_without_a_record() {
    assert_eq!(version_of_installed(None), "unknown");
}

#[test]
fn version_with_control_or_unsafe_characters_becomes_unknown() {
    for body in [
        &b"version=1.2.3\x1b[31m\n"[..],
        b"version=1.2.3 ; rm -rf\n",
        b"version=../../etc\n",
        b"version=\n",
    ] {
        assert_eq!(version_of_installed(Some(body)), "unknown", "{body:?}");
    }
    let long = format!("version={}\n", "1".repeat(65));
    assert_eq!(version_of_installed(Some(long.as_bytes())), "unknown");
    let max = format!("version={}\n", "1".repeat(64));
    assert_eq!(version_of_installed(Some(max.as_bytes())), "1".repeat(64));
}

#[test]
fn version_reads_at_most_64_kib() {
    let mut body = vec![b'#'; 64 * 1024];
    body.extend_from_slice(b"\nversion=9.9.9\n");
    assert_eq!(version_of_installed(Some(&body)), "unknown");
}

#[test]
fn checkout_version_comes_from_workspace_cargo_toml_with_suffix() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    write_checkout(&checkout, "B");
    fs::write(
        checkout.join("Cargo.toml"),
        "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.2.151\"\n",
    )
    .unwrap();
    let selection = select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        ..inputs(&checkout)
    })
    .unwrap();
    assert_eq!(selection.version(), "1.2.151+checkout");
}

// ----------------------------------------------------------- nested runs
// ------------------------------------------------------- bundle-pins layout

/// A key-shaped name used for hand-built snapshots.
const KEY: &str = "0123456789abcdef01234567";
/// A second key-shaped name.
const KEY2: &str = "fedcba9876543210fedcba98";

fn is_key(name: &str) -> bool {
    name.len() == 24 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `<root>/work`, private, as `WorkRoot::ensure_created` leaves it.
fn work_root(root: &Path) -> PathBuf {
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    set_mode(&work, 0o700);
    work
}

fn pins_dir(work: &Path) -> PathBuf {
    work.join("bundle-pins")
}

/// `<work>/bundle-pins`, created private if missing.
fn private_pins(work: &Path) -> PathBuf {
    let pins = pins_dir(work);
    fs::create_dir_all(&pins).unwrap();
    set_mode(&pins, 0o700);
    pins
}

/// Sorted names in `<work>/bundle-pins`; empty when it does not exist.
fn pin_entries(work: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(pins_dir(work)) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// `<work>/bundle-pins/<key>.use` for the snapshot `home`.
fn lease_file(home: &Path) -> PathBuf {
    let name = home.file_name().unwrap().to_str().unwrap();
    home.with_file_name(format!("{name}.use"))
}

/// True when some open file description holds a `flock` on `path`: a
/// non-blocking exclusive lock from a fresh open is refused. The probe lock
/// is released when the file is closed on return.
fn held(path: &Path) -> bool {
    let file = fs::File::open(path).unwrap();
    !FileExt::try_lock_exclusive(&file).unwrap()
}

fn exists_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// A snapshot as `pin` lays it out, built by hand:
/// `<work>/bundle-pins/<KEY>/amplifier-bundle`, an empty `<KEY>.use` and the
/// store's `.lock`.
/// Returns the work root and the snapshot home (the marker).
fn parent_snapshot(root: &Path) -> (PathBuf, PathBuf) {
    let work = work_root(root);
    let pins = private_pins(&work);
    let marker = pins.join(KEY);
    fs::create_dir(&marker).unwrap();
    write_bundle(&marker, "parent-A");
    for name in [format!("{KEY}.use"), ".lock".to_string()] {
        let path = pins.join(name);
        fs::write(&path, "").unwrap();
        set_mode(&path, 0o600);
    }
    (work, marker)
}

/// Inputs of a run nested in the run that pinned `marker`.
fn nested_inputs<'a>(
    working_dir: &'a Path,
    work: &'a Path,
    marker: &'a Path,
    choice: &'static str,
) -> SelectInputs<'a> {
    SelectInputs {
        amplihack_home: os(marker),
        run_bundle: os(marker),
        run_bundle_choice: Some(OsStr::new(choice)),
        work_root: Some(work),
        ..inputs(working_dir)
    }
}

fn installed_selection(root: &Path) -> (PathBuf, super::bundle_pin::Selection) {
    let home = root.join("installed");
    let bundle = write_bundle(&home, "installed-A");
    (bundle, select_installed(root, &home))
}

fn select_installed(root: &Path, home: &Path) -> super::bundle_pin::Selection {
    select_from(SelectInputs {
        amplihack_home: os(home),
        ..inputs(root)
    })
    .unwrap()
}

/// Makes `file` in a read-only snapshot say `text`, keeping its mode.
fn tamper(file: &Path, text: &str) {
    let mode = mode_of(file);
    set_mode(file, 0o644);
    fs::write(file, text).unwrap();
    set_mode(file, mode);
}

#[test]
fn a_valid_marker_is_reused_without_selection() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let checkout = root.join("checkout");
    write_checkout(&checkout, "B");

    for choice in [None, Some(BundleChoice::Installed)] {
        let selection = select_from(SelectInputs {
            choice,
            ..nested_inputs(&checkout, &work, &marker, "installed")
        })
        .unwrap();
        assert!(
            selection.inherited(),
            "choice {choice:?}: {:?}",
            selection.notes()
        );
        assert_eq!(selection.choice(), BundleChoice::Installed);
        assert_eq!(
            selection.source(),
            fs::canonicalize(marker.join("amplifier-bundle")).unwrap()
        );
    }
}

#[test]
fn a_nested_run_asking_for_another_bundle_is_refused_with_e3() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let checkout = root.join("checkout");
    write_checkout(&checkout, "B");
    let text = err_text(select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        ..nested_inputs(&checkout, &work, &marker, "installed")
    }));
    assert_eq!(
        text,
        format!(
            "this run is nested in a run pinned to the installed bundle at {}; \
             --bundle checkout cannot change it",
            fs::canonicalize(&marker).unwrap().display()
        )
    );
}

#[test]
fn a_marker_failing_any_check_is_ignored_with_a_note() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let installed = root.join("installed");
    let installed_bundle = write_bundle(&installed, "A");
    // Key-shaped, but its parent is not a `bundle-pins` folder.
    let elsewhere = root.join("elsewhere").join(KEY);
    fs::create_dir_all(&elsewhere).unwrap();
    write_bundle(&elsewhere, "X");
    let other_work = root.join("other-work");
    fs::create_dir_all(&other_work).unwrap();
    set_mode(&other_work, 0o700);

    // (AMPLIHACK_HOME, marker, marker choice, work root) combinations that
    // each fail one check.
    let cases: Vec<(&Path, &Path, &str, Option<&Path>, &str)> = vec![
        (
            &installed,
            &marker,
            "installed",
            Some(&work),
            "marker != AMPLIHACK_HOME",
        ),
        (
            &elsewhere,
            &elsewhere,
            "installed",
            Some(&work),
            "not under bundle-pins",
        ),
        (&marker, &marker, "Installed", Some(&work), "bad choice"),
        (&marker, &marker, "", Some(&work), "empty choice"),
        (
            &marker,
            &marker,
            "installed",
            Some(&other_work),
            "another work root",
        ),
        (&marker, &marker, "installed", None, "no work root"),
    ];
    for (home, run_bundle, choice, work_root, why) in cases {
        let selection = select_from(SelectInputs {
            amplihack_home: os(home),
            run_bundle: os(run_bundle),
            run_bundle_choice: Some(OsStr::new(choice)),
            configured_home: Some(&installed),
            work_root,
            ..inputs(&root)
        })
        .unwrap_or_else(|e| panic!("{why}: {e:#}"));
        assert!(!selection.inherited(), "{why}");
        assert!(!selection.notes().is_empty(), "{why}: a note is printed");
        assert_eq!(
            selection.source(),
            fs::canonicalize(&installed_bundle).unwrap(),
            "{why}: the installed bundle is selected instead"
        );
    }
}

#[test]
fn a_marker_under_another_work_roots_bundle_pins_is_refused() {
    let (_tmp, root) = scratch();
    // A complete, trusted snapshot of some other work root.
    let (_other_work, foreign) = parent_snapshot(&root.join("other"));
    let work = work_root(&root);
    private_pins(&work);
    let installed = root.join("installed");
    let installed_bundle = write_bundle(&installed, "A");
    let selection = select_from(SelectInputs {
        configured_home: Some(&installed),
        ..nested_inputs(&root, &work, &foreign, "installed")
    })
    .unwrap();
    assert!(!selection.inherited(), "{:?}", selection.notes());
    assert!(
        selection
            .notes()
            .iter()
            .any(|n| n.contains(&foreign.display().to_string())),
        "the note names the refused marker: {:?}",
        selection.notes()
    );
    assert_eq!(
        selection.source(),
        fs::canonicalize(&installed_bundle).unwrap()
    );
}

#[test]
fn an_ignored_marker_and_amplihack_home_that_both_cannot_be_resolved_are_not_the_same() {
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    private_pins(&work);
    let installed = root.join("installed");
    let installed_bundle = write_bundle(&installed, "A");
    let marker = pins_dir(&work).join(KEY);
    let missing_home = root.join("no-such-home");
    let selection = select_from(SelectInputs {
        amplihack_home: os(&missing_home),
        run_bundle: os(&marker),
        run_bundle_choice: Some(OsStr::new("installed")),
        configured_home: Some(&installed),
        work_root: Some(&work),
        ..inputs(&root)
    })
    .unwrap();
    assert!(!selection.inherited());
    assert!(
        !selection
            .notes()
            .iter()
            .any(|n| n.contains("names the ignored")),
        "two paths that cannot be resolved are not one folder: {:?}",
        selection.notes()
    );
    assert_eq!(
        selection.source(),
        fs::canonicalize(&installed_bundle).unwrap()
    );
}

#[test]
fn a_marker_whose_name_is_not_a_key_is_ignored() {
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    let pins = private_pins(&work);
    let installed = root.join("installed");
    write_bundle(&installed, "A");
    for name in [
        "0123456789ABCDEF01234567",
        "0123456789abcdef0123456",
        "0123456789abcdef012345678",
        "g123456789abcdef01234567",
        ".tmp-0f8f4a2e-7c55-4a8e-9a7e-2b1d7c3e9f10",
        "bundle",
    ] {
        let marker = pins.join(name);
        fs::create_dir(&marker).unwrap();
        write_bundle(&marker, "X");
        let selection = select_from(SelectInputs {
            configured_home: Some(&installed),
            ..nested_inputs(&root, &work, &marker, "installed")
        })
        .unwrap();
        assert!(!selection.inherited(), "{name}");
    }
}

#[test]
fn the_old_runtime_bundle_layouts_are_no_longer_markers() {
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    let installed = root.join("installed");
    write_bundle(&installed, "A");
    for rel in ["runs/r1/runtime/bundle", "r/0123456789ab/bundle"] {
        let marker = work.join(rel);
        fs::create_dir_all(&marker).unwrap();
        write_bundle(&marker, "old-layout");
        let mut dir = marker.parent();
        while let Some(d) = dir.filter(|d| *d != work) {
            set_mode(d, 0o700);
            dir = d.parent();
        }
        let selection = select_from(SelectInputs {
            configured_home: Some(&installed),
            ..nested_inputs(&root, &work, &marker, "installed")
        })
        .unwrap();
        assert!(!selection.inherited(), "{rel}");
        assert!(!selection.notes().is_empty(), "{rel}");
    }
}

#[test]
fn is_snapshot_path_accepts_only_a_key_folder_in_this_work_roots_bundle_pins() {
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    let pins = private_pins(&work);
    let other_pins = private_pins(&work_root(&root.join("other")));
    let accepted = pins.join(KEY);
    fs::create_dir_all(&accepted).unwrap();
    assert!(is_snapshot_path(&accepted, &work));

    let refused = [
        pins.join("0123456789ABCDEF01234567"),
        pins.join("0123456789abcdef0123456"),
        pins.join("0123456789abcdef012345678"),
        pins.join(KEY).join("amplifier-bundle"),
        pins.join(".tmp-0f8f4a2e-7c55-4a8e-9a7e-2b1d7c3e9f10"),
        pins.clone(),
        other_pins.join(KEY),
        work.join("runs/r/runtime/bundle"),
        work.join("r/0123456789ab/bundle"),
        work.join(KEY),
    ];
    for path in refused {
        fs::create_dir_all(&path).unwrap();
        assert!(!is_snapshot_path(&path, &work), "{}", path.display());
    }
}

#[test]
fn inherited_marker_failing_trust_is_ignored() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    set_mode(&marker.join("amplifier-bundle"), 0o777);
    let installed = root.join("installed");
    let installed_bundle = write_bundle(&installed, "A");
    let selection = select_from(SelectInputs {
        configured_home: Some(&installed),
        ..nested_inputs(&root, &work, &marker, "installed")
    })
    .unwrap();
    assert!(!selection.inherited());
    assert_eq!(
        selection.source(),
        fs::canonicalize(&installed_bundle).unwrap()
    );
}

#[test]
fn an_unreadable_pin_record_is_noted_not_silently_dropped() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let select = || select_from(nested_inputs(&root, &work, &marker, "installed")).unwrap();
    let missing = select();
    assert!(missing.inherited());
    assert_eq!(missing.version(), "unknown");
    let note = missing.notes().join("\n");
    assert!(note.contains("pin.json"), "{note}");

    fs::write(marker.join("pin.json"), "{not json").unwrap();
    let malformed = select();
    assert!(malformed.inherited());
    assert!(
        malformed.notes().iter().any(|n| n.contains("pin record")),
        "{:?}",
        malformed.notes()
    );
}

#[test]
fn a_nested_run_takes_a_shared_lease_and_never_copies_or_removes() {
    let (_tmp, root) = scratch();
    let installed = root.join("installed");
    let source = write_bundle(&installed, "installed-A");
    fs::write(installed.join(".installed-by"), "version=1.2.151\n").unwrap();
    let work = work_root(&root);
    let outer = pin(&select_installed(&root, &installed), &work, false).unwrap();
    let home = outer.home().to_path_buf();

    let selection = select_from(nested_inputs(&root, &work, &home, "installed")).unwrap();
    assert!(selection.inherited(), "{:?}", selection.notes());
    let before = pin_entries(&work);
    let nested = pin(&selection, &work, false).unwrap();
    assert_eq!(nested.home(), home);
    assert_eq!(pin_entries(&work), before, "no second copy, no new file");
    let record = nested.record();
    assert!(record.inherited);
    assert_eq!(record.choice, "installed");
    assert_eq!(record.version, "1.2.151", "taken from pin.json");
    assert_eq!(
        Path::new(&record.source),
        fs::canonicalize(&source).unwrap(),
        "taken from pin.json"
    );

    // The outer run ends first: the nested lease keeps the snapshot.
    drop(outer);
    assert!(home.join("amplifier-bundle/recipes/probe.yaml").exists());
    assert!(held(&lease_file(&home)));

    // A nested lease never removes the snapshot, even as its last user.
    drop(nested);
    assert!(home.join("amplifier-bundle/recipes/probe.yaml").exists());
    assert!(!held(&lease_file(&home)));

    // The next launch of the same bundle reuses it, and removes it at its end.
    let later = pin(&select_installed(&root, &installed), &work, false).unwrap();
    assert_eq!(later.home(), home);
    drop(later);
    assert!(!exists_no_follow(&home));
}

#[test]
fn a_nested_run_without_a_lease_file_is_refused_with_e13() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let lease = lease_file(&marker);
    fs::remove_file(&lease).unwrap();
    let selection = select_from(nested_inputs(&root, &work, &marker, "installed")).unwrap();
    assert!(selection.inherited());
    let text = err_text(pin(&selection, &work, false));
    assert!(
        text.contains(&format!(
            "inherited bundle snapshot {} has no lease file {}; \
             the run that pinned it has ended",
            marker.display(),
            lease.display()
        )),
        "{text}"
    );
    assert!(
        !exists_no_follow(&lease),
        "a nested run never creates the lease file"
    );
    assert!(marker.join("amplifier-bundle/recipes/probe.yaml").exists());
}

#[test]
fn a_nested_run_without_the_store_lock_file_is_refused_with_e13() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let lock = pins_dir(&work).join(".lock");
    fs::remove_file(&lock).unwrap();
    let lease = lease_file(&marker);
    let selection = select_from(nested_inputs(&root, &work, &marker, "installed")).unwrap();
    assert!(selection.inherited());
    let text = err_text(pin(&selection, &work, false));
    assert!(
        text.contains(&format!(
            "inherited bundle snapshot {} has no lock file {}; \
             the run that pinned it has ended",
            marker.display(),
            lock.display()
        )),
        "{text}"
    );
    assert!(!exists_no_follow(&lock), "a nested run never creates .lock");
    assert!(!held(&lease), "no lease is taken");
    assert!(marker.join("amplifier-bundle/recipes/probe.yaml").exists());
}

// ------------------------------------------------- nested in auto-drive

/// An auto-drive invocation copy of `snapshot`, as `LaunchBundle::prepare`
/// lays it out: a private folder with `amplifier-bundle` and a source record.
fn launch_copy(root: &Path, name: &str, source: &Path) -> PathBuf {
    let launch = root.join(name);
    fs::create_dir_all(&launch).unwrap();
    set_mode(&launch, 0o700);
    write_bundle(&launch, "launch-copy");
    fs::write(
        launch.join(crate::commands::orch::autodrive::trusted::LAUNCH_SOURCE_FILE),
        fs::canonicalize(source).unwrap().to_str().unwrap(),
    )
    .unwrap();
    launch
}

#[test]
fn a_run_nested_in_an_auto_drive_step_reuses_the_outer_snapshot() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let launch = launch_copy(&root, "autodrive-invocation-a", &marker);
    let installed = root.join("installed");
    write_bundle(&installed, "A");
    let selection = select_from(SelectInputs {
        amplihack_home: os(&launch),
        configured_home: Some(&installed),
        ..nested_inputs(&root, &work, &marker, "checkout")
    })
    .unwrap();
    assert!(selection.inherited(), "{:?}", selection.notes());
    assert_eq!(selection.choice(), BundleChoice::Checkout);
    assert_eq!(
        selection.source(),
        fs::canonicalize(marker.join("amplifier-bundle")).unwrap()
    );
    let before = pin_entries(&work);
    let pinned = pin(&selection, &work, false).unwrap();
    assert_eq!(pin_entries(&work), before, "no second copy");
    assert_eq!(pinned.record().choice, "checkout");
}

#[test]
fn a_launch_copy_of_another_bundle_does_not_vouch_for_the_marker() {
    let (_tmp, root) = scratch();
    let (work, marker) = parent_snapshot(&root);
    let installed = root.join("installed");
    write_bundle(&installed, "A");
    let launch = launch_copy(&root, "autodrive-invocation-b", &installed);
    let selection = select_from(SelectInputs {
        amplihack_home: os(&launch),
        configured_home: Some(&installed),
        ..nested_inputs(&root, &work, &marker, "installed")
    })
    .unwrap();
    assert!(!selection.inherited());
    assert!(!selection.notes().is_empty());
}

// ------------------------------------------------------------------ pin

#[test]
fn pin_copies_into_a_content_keyed_folder_with_read_only_modes() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);

    let pinned = pin(&selection, &work, false).unwrap();
    let home = pinned.home().to_path_buf();
    let pins = pins_dir(&work);
    assert_eq!(home.parent(), Some(pins.as_path()));
    let key = home.file_name().unwrap().to_str().unwrap();
    assert!(is_key(key), "{key}");
    assert!(is_snapshot_path(&home, &work));
    assert_eq!(pinned.bundle(), home.join("amplifier-bundle"));
    assert_eq!(pinned.source(), fs::canonicalize(&source).unwrap());

    assert_eq!(mode_of(&work), 0o700, "the work root is not widened");
    assert_eq!(mode_of(&pins), 0o700);
    assert_eq!(
        mode_of(&home),
        0o555,
        "the shared snapshot home is read-only"
    );
    assert_eq!(mode_of(pinned.bundle()), 0o555);
    assert_eq!(mode_of(&pinned.bundle().join("recipes")), 0o555);
    assert_eq!(mode_of(&pinned.bundle().join("tools/identity.sh")), 0o555);
    assert_eq!(mode_of(&pinned.bundle().join("recipes/probe.yaml")), 0o444);
    assert_eq!(mode_of(&home.join("pin.json")), 0o444);
    let lease = lease_file(&home);
    let lease_meta = fs::symlink_metadata(&lease).unwrap();
    assert!(lease_meta.file_type().is_file());
    assert_eq!(mode_of(&lease), 0o600);
    assert!(held(&lease), "the run holds its lease");
    assert!(
        fs::read_to_string(pinned.bundle().join("tools/identity.sh"))
            .unwrap()
            .contains("installed-A")
    );

    let record = pinned.record();
    assert_eq!(record.choice, "installed");
    assert!(!record.inherited);
    assert_eq!(record.snapshot.as_deref(), Some(home.to_str().unwrap()));
    assert_eq!(record.source, pinned.source().to_str().unwrap());

    let shared: BundleRecord =
        serde_json::from_slice(&fs::read(home.join("pin.json")).unwrap()).unwrap();
    assert_eq!(shared.source, record.source);
    assert_eq!(shared.version, record.version);
    assert_eq!(shared.choice, "installed");

    let entries = pin_entries(&work);
    assert!(
        !entries.iter().any(|n| n.starts_with(".tmp-")),
        "no copy in progress is left: {entries:?}"
    );
}

#[test]
fn the_same_contents_give_the_same_key_and_path() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);

    let first = pin(&selection, &work, false).unwrap();
    let home = first.home().to_path_buf();
    drop(first);
    assert!(!exists_no_follow(&home), "removed by its last lease");

    // A relaunch, as a resumed run does: same path, so the same runner
    // arguments and recovery fingerprints (#1741).
    let again = pin(
        &select_installed(&root, &root.join("installed")),
        &work,
        false,
    )
    .unwrap();
    assert_eq!(again.home(), home);
    // A concurrent run of the same bundle shares the snapshot.
    let concurrent = pin(&selection, &work, false).unwrap();
    assert_eq!(concurrent.home(), home);
    let keys: Vec<String> = pin_entries(&work)
        .into_iter()
        .filter(|n| is_key(n))
        .collect();
    assert_eq!(keys.len(), 1, "{keys:?}");
}

#[test]
fn a_changed_file_gives_a_different_key() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    let before = first.home().to_path_buf();

    fs::write(source.join("agents/core/builder.md"), "# changed\n").unwrap();
    let second = pin(
        &select_installed(&root, &root.join("installed")),
        &work,
        false,
    )
    .unwrap();
    assert_ne!(second.home(), before);
    assert!(
        fs::read_to_string(first.bundle().join("agents/core/builder.md"))
            .unwrap()
            .contains("installed-A"),
        "the first run keeps its own snapshot"
    );
    assert_eq!(
        fs::read_to_string(second.bundle().join("agents/core/builder.md")).unwrap(),
        "# changed\n"
    );
}

#[test]
fn an_added_file_gives_a_different_key() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let before = pin(&selection, &work, false).unwrap().home().to_path_buf();
    fs::write(
        source.join("recipes/extra.yaml"),
        "name: extra\nsteps: []\n",
    )
    .unwrap();
    set_mode(&source.join("recipes/extra.yaml"), 0o644);
    let after = pin(
        &select_installed(&root, &root.join("installed")),
        &work,
        false,
    )
    .unwrap();
    assert_ne!(after.home(), before);
}

#[test]
fn an_execute_bit_change_gives_a_different_key_but_a_write_bit_change_does_not() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let base = pin(&selection, &work, false).unwrap().home().to_path_buf();

    // The copy normalises non-executable files to 0644, so a read-only
    // source file gives the same tree and the same key.
    set_mode(&source.join("recipes/probe.yaml"), 0o444);
    let same = pin(
        &select_installed(&root, &root.join("installed")),
        &work,
        false,
    )
    .unwrap();
    assert_eq!(same.home(), base);
    drop(same);

    set_mode(&source.join("tools/identity.sh"), 0o644);
    let other = pin(
        &select_installed(&root, &root.join("installed")),
        &work,
        false,
    )
    .unwrap();
    assert_ne!(other.home(), base);
}

#[test]
fn the_same_bytes_at_another_source_path_get_another_key() {
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    let a = root.join("a");
    let b = root.join("b");
    write_bundle(&a, "same");
    write_bundle(&b, "same");
    let pa = pin(&select_installed(&root, &a), &work, false).unwrap();
    let pb = pin(&select_installed(&root, &b), &work, false).unwrap();
    assert_ne!(pa.home(), pb.home());
}

#[test]
fn installed_and_checkout_choices_never_share_a_snapshot() {
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    let installed = root.join("installed");
    write_bundle(&installed, "same");
    let checkout = root.join("checkout");
    write_checkout(&checkout, "same");
    let pi = pin(&select_installed(&checkout, &installed), &work, false).unwrap();
    let pc = pin(
        &select_from(SelectInputs {
            choice: Some(BundleChoice::Checkout),
            ..inputs(&checkout)
        })
        .unwrap(),
        &work,
        false,
    )
    .unwrap();
    assert_ne!(pi.home(), pc.home());
    assert_eq!(pc.record().choice, "checkout");
}

#[test]
fn the_snapshot_survives_an_upgrade_of_the_source() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pinned = pin(&selection, &work, false).unwrap();

    // Replace the installed bundle the way an upgrade does (rename over).
    let staged = root.join("staged");
    let new_bundle = write_bundle(&staged, "installed-B");
    fs::remove_dir_all(&source).unwrap();
    fs::rename(&new_bundle, &source).unwrap();

    let tool = fs::read_to_string(pinned.bundle().join("tools/identity.sh")).unwrap();
    assert!(tool.contains("installed-A"), "{tool}");
}

#[test]
fn a_dry_run_checks_every_entry_and_copies_nothing() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pinned = pin(&selection, &work, true).unwrap();
    assert!(!exists_no_follow(&pins_dir(&work)));
    assert_eq!(pinned.bundle(), selection.source());
    assert_eq!(pinned.record().snapshot, None);
}

// --------------------------------------------------------- lease and cleanup

#[test]
fn the_last_lease_removes_the_snapshot_and_its_lease_file() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pinned = pin(&selection, &work, false).unwrap();
    let home = pinned.home().to_path_buf();
    drop(pinned);
    assert!(!exists_no_follow(&home));
    assert!(!exists_no_follow(&lease_file(&home)));
    let left: Vec<String> = pin_entries(&work)
        .into_iter()
        .filter(|n| n != ".lock")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_second_lease_keeps_the_snapshot_until_it_ends() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    let second = pin(&selection, &work, false).unwrap();
    let home = first.home().to_path_buf();
    assert_eq!(second.home(), home);

    drop(first);
    assert!(home.join("amplifier-bundle/tools/identity.sh").exists());
    assert!(
        held(&lease_file(&home)),
        "the second run still holds its lease"
    );

    drop(second);
    assert!(!exists_no_follow(&home));
    assert!(!exists_no_follow(&lease_file(&home)));
}

/// Pins, records the home, then fails the way a later step's `?` does.
fn pin_then_fail(
    selection: &super::bundle_pin::Selection,
    work: &Path,
    home: &mut Option<PathBuf>,
) -> anyhow::Result<()> {
    let pinned = pin(selection, work, false)?;
    *home = Some(pinned.home().to_path_buf());
    std::fs::read(work.join("no-such-file"))?;
    unreachable!("the read above fails");
}

#[test]
fn the_lease_is_released_on_an_early_return() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let mut home = None;
    assert!(pin_then_fail(&selection, &work, &mut home).is_err());
    let home = home.expect("pinned before the failure");
    assert!(!exists_no_follow(&home));
    assert!(!exists_no_follow(&lease_file(&home)));
}

#[test]
fn the_lease_is_released_on_panic_unwind() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let mut home = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let pinned = pin(&selection, &work, false).unwrap();
        home = Some(pinned.home().to_path_buf());
        panic!("a step panicked while the bundle was pinned");
    }));
    assert!(outcome.is_err());
    let home = home.expect("pinned before the panic");
    assert!(!exists_no_follow(&home));
    assert!(!exists_no_follow(&lease_file(&home)));
}

// ------------------------------------------------------ leftovers and sweep

#[test]
fn a_stale_copy_in_progress_is_removed_and_unknown_names_are_left() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = private_pins(&work);
    let stale = pins.join(format!(".tmp-{}", uuid::Uuid::new_v4().hyphenated()));
    fs::create_dir_all(stale.join("amplifier-bundle/recipes")).unwrap();
    fs::write(stale.join("amplifier-bundle/recipes/half.yaml"), "x").unwrap();
    set_mode(&stale.join("amplifier-bundle/recipes"), 0o555);
    set_mode(&stale.join("amplifier-bundle"), 0o555);
    let unknown = [".tmp-not-a-uuid", "ABCDEF0123456789ABCDEF01", "notes.txt"];
    for name in unknown {
        if name.ends_with(".txt") {
            fs::write(pins.join(name), "keep").unwrap();
        } else {
            fs::create_dir(pins.join(name)).unwrap();
        }
    }

    let _pinned = pin(&selection, &work, false).unwrap();
    assert!(!exists_no_follow(&stale));
    for name in unknown {
        assert!(exists_no_follow(&pins.join(name)), "{name} is left alone");
    }
}

/// A hand-built leftover snapshot `<pins>/<key>` with a read-only tree.
fn leftover(pins: &Path, key: &str) -> PathBuf {
    let home = pins.join(key);
    fs::create_dir(&home).unwrap();
    let bundle = write_bundle(&home, "leftover");
    for dir in ["recipes", "agents/core", "agents", "tools", ""] {
        set_mode(&bundle.join(dir), 0o555);
    }
    home
}

#[test]
fn an_unused_leftover_snapshot_is_swept() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = private_pins(&work);
    // Left by a killed run: no lease file.
    let without_lease = leftover(&pins, KEY);
    // Left by a killed run: a lease file nobody holds.
    let with_lease = leftover(&pins, KEY2);
    fs::write(lease_file(&with_lease), "").unwrap();
    // A lone lease file whose folder is gone.
    let lone = pins.join("aaaaaaaaaaaaaaaaaaaaaaaa.use");
    fs::write(&lone, "").unwrap();

    let pinned = pin(&selection, &work, false).unwrap();
    for gone in [&without_lease, &with_lease, &lease_file(&with_lease), &lone] {
        assert!(!exists_no_follow(gone), "{} is swept", gone.display());
    }
    assert!(pinned.bundle().join("tools/identity.sh").exists());
}

#[test]
fn a_leftover_snapshot_in_use_is_left_alone() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = private_pins(&work);
    let busy = leftover(&pins, KEY);
    fs::write(lease_file(&busy), "").unwrap();
    let busy_lease = fs::File::open(lease_file(&busy)).unwrap();
    FileExt::lock_shared(&busy_lease).unwrap();
    let lone = pins.join("bbbbbbbbbbbbbbbbbbbbbbbb.use");
    fs::write(&lone, "").unwrap();
    let lone_lease = fs::File::open(&lone).unwrap();
    FileExt::lock_shared(&lone_lease).unwrap();

    let _pinned = pin(&selection, &work, false).unwrap();
    assert!(busy.join("amplifier-bundle/tools/identity.sh").exists());
    assert!(exists_no_follow(&lease_file(&busy)));
    assert!(exists_no_follow(&lone));
    drop((busy_lease, lone_lease));
}

#[test]
fn a_symlink_planted_in_bundle_pins_is_unlinked_and_its_target_left() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = private_pins(&work);
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    let planted = [
        (pins.join(KEY), outside.clone()),
        (
            pins.join(format!(".tmp-{}", uuid::Uuid::new_v4().hyphenated())),
            outside.clone(),
        ),
        (pins.join(format!("{KEY2}.use")), outside.join("keep")),
    ];
    for (link, target) in &planted {
        symlink(target, link).unwrap();
    }

    let _pinned = pin(&selection, &work, false).unwrap();
    for (link, _) in &planted {
        assert!(!exists_no_follow(link), "{} is unlinked", link.display());
    }
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
    assert_eq!(mode_of(&outside), 0o755);
}

#[test]
fn removing_a_snapshot_never_changes_modes_through_a_symlink() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = private_pins(&work);
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    set_mode(&outside, 0o555);
    let home = pins.join(KEY);
    fs::create_dir(&home).unwrap();
    let bundle = write_bundle(&home, "leftover");
    symlink(&outside, bundle.join("escape")).unwrap();
    set_mode(&bundle, 0o555);

    let _pinned = pin(&selection, &work, false).unwrap();
    assert!(!exists_no_follow(&home), "the unused leftover is swept");
    assert_eq!(mode_of(&outside), 0o555, "the link target keeps its mode");
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
    set_mode(&outside, 0o755);
}

// ------------------------------------------------- reuse and replacement

#[test]
fn an_unused_snapshot_whose_files_changed_is_replaced() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    let home = first.home().to_path_buf();
    // Keep the folder as a killed run would leave it, then change a file.
    let kept = root.join("kept");
    move_snapshot(&home, &kept);
    drop(first);
    tamper(
        &kept.join("amplifier-bundle/tools/identity.sh"),
        "#!/bin/sh\necho tampered\n",
    );
    move_snapshot(&kept, &home);

    let second = pin(&selection, &work, false).unwrap();
    assert_eq!(second.home(), home, "same key");
    let tool = fs::read_to_string(second.bundle().join("tools/identity.sh")).unwrap();
    assert!(
        tool.contains("installed-A"),
        "replaced from the source: {tool}"
    );
    assert_eq!(mode_of(&second.bundle().join("tools/identity.sh")), 0o555);
}

#[test]
fn a_snapshot_in_use_whose_files_changed_stops_the_launch_with_e11() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    let home = first.home().to_path_buf();
    let tool = home.join("amplifier-bundle/tools/identity.sh");
    tamper(&tool, "#!/bin/sh\necho tampered\n");

    let text = err_text(pin(&selection, &work, false));
    assert!(
        text.contains(&format!(
            "bundle snapshot {} is in use by another run and its contents differ from {}; \
             wait for that run to end",
            home.display(),
            fs::canonicalize(&source).unwrap().display()
        )),
        "{text}"
    );
    assert!(
        fs::read_to_string(&tool).unwrap().contains("tampered"),
        "left in place"
    );
    assert!(held(&lease_file(&home)), "the first run keeps its lease");
    let entries = pin_entries(&work);
    assert!(
        !entries.iter().any(|n| n.starts_with(".tmp-")),
        "the refused copy is removed: {entries:?}"
    );
    drop(first);
}

#[test]
fn a_snapshot_with_an_extra_file_is_not_reused() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    let home = first.home().to_path_buf();
    let kept = root.join("kept");
    move_snapshot(&home, &kept);
    drop(first);
    let recipes = kept.join("amplifier-bundle/recipes");
    set_mode(&recipes, 0o755);
    fs::write(recipes.join("planted.yaml"), "name: planted\nsteps: []\n").unwrap();
    set_mode(&recipes, 0o555);
    move_snapshot(&kept, &home);

    let second = pin(&selection, &work, false).unwrap();
    assert_eq!(second.home(), home);
    assert!(!second.bundle().join("recipes/planted.yaml").exists());
}

/// Pins `selection`, then leaves the snapshot as a killed run would (no run
/// holds it) with `plant` applied to its `<key>/` folder. Returns the home.
fn unused_snapshot_with(
    root: &Path,
    work: &Path,
    selection: &super::bundle_pin::Selection,
    plant: impl FnOnce(&Path),
) -> PathBuf {
    let first = pin(selection, work, false).unwrap();
    let home = first.home().to_path_buf();
    let kept = root.join("kept");
    move_snapshot(&home, &kept);
    drop(first);
    set_mode(&kept, 0o755);
    plant(&kept);
    move_snapshot(&kept, &home);
    home
}

#[test]
fn an_unused_snapshot_with_a_file_beside_the_bundle_is_replaced() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let home = unused_snapshot_with(&root, &work, &selection, |home| {
        fs::write(home.join("config"), "git_identity=planted\n").unwrap();
    });

    let second = pin(&selection, &work, false).unwrap();
    assert_eq!(second.home(), home, "same key");
    assert!(
        !exists_no_follow(&home.join("config")),
        "the planted file is gone"
    );
    assert!(home.join("pin.json").is_file());
    assert!(second.bundle().join("recipes/probe.yaml").is_file());
}

#[test]
fn an_unused_snapshot_with_a_folder_beside_the_bundle_is_replaced() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let home = unused_snapshot_with(&root, &work, &selection, |home| {
        fs::create_dir(home.join(".claude")).unwrap();
        fs::write(home.join(".claude/x"), "planted\n").unwrap();
    });

    let second = pin(&selection, &work, false).unwrap();
    assert_eq!(second.home(), home, "same key");
    assert!(
        !exists_no_follow(&home.join(".claude")),
        "the planted folder is gone"
    );
}

#[test]
fn an_unused_snapshot_whose_pin_record_is_a_symlink_is_replaced() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let elsewhere = root.join("elsewhere.json");
    fs::write(&elsewhere, "{}").unwrap();
    let home = unused_snapshot_with(&root, &work, &selection, |home| {
        fs::remove_file(home.join("pin.json")).unwrap();
        symlink(&elsewhere, home.join("pin.json")).unwrap();
    });

    let _second = pin(&selection, &work, false).unwrap();
    let meta = fs::symlink_metadata(home.join("pin.json")).unwrap();
    assert!(meta.is_file(), "a fresh pin record replaced the symlink");
    assert_eq!(
        fs::read_to_string(&elsewhere).unwrap(),
        "{}",
        "target left alone"
    );
}

#[test]
fn a_snapshot_in_use_with_a_file_beside_the_bundle_stops_the_launch_with_e11() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    let home = first.home().to_path_buf();
    let planted = home.join("config");
    set_mode(&home, 0o755);
    fs::write(&planted, "git_identity=planted\n").unwrap();

    let text = err_text(pin(&selection, &work, false));
    assert!(
        text.contains(&format!(
            "bundle snapshot {} is in use by another run and its contents differ from {}",
            home.display(),
            fs::canonicalize(&source).unwrap().display()
        )),
        "{text}"
    );
    assert!(
        planted.exists(),
        "nothing is removed while the snapshot is in use"
    );
    assert!(first.bundle().join("recipes/probe.yaml").exists());
    assert!(held(&lease_file(&home)), "the first run keeps its lease");
    drop(first);
}

/// Every path in the tree at `root` (symlinks not followed) with a write bit.
fn writable_entries(root: &Path) -> Vec<PathBuf> {
    let meta = fs::symlink_metadata(root).unwrap();
    let mut found = Vec::new();
    if meta.file_type().is_symlink() {
        return found;
    }
    if meta.permissions().mode() & 0o222 != 0 {
        found.push(root.to_path_buf());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(root).unwrap() {
            found.extend(writable_entries(&entry.unwrap().path()));
        }
    }
    found
}

/// `chmod -R u+w` on the tree at `root`, never through a symlink.
fn add_owner_write(root: &Path) {
    let meta = fs::symlink_metadata(root).unwrap();
    if meta.file_type().is_symlink() {
        return;
    }
    set_mode(root, (meta.permissions().mode() & 0o7777) | 0o200);
    if meta.is_dir() {
        for entry in fs::read_dir(root).unwrap() {
            add_owner_write(&entry.unwrap().path());
        }
    }
}

#[test]
fn a_reused_snapshot_made_writable_is_made_read_only_again() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let first = pin(&selection, &work, false).unwrap();
    add_owner_write(first.home());
    assert!(!writable_entries(first.home()).is_empty());

    let second = pin(&selection, &work, false).unwrap();
    assert_eq!(second.home(), first.home(), "reused, same key");
    assert_eq!(
        writable_entries(second.home()),
        Vec::<PathBuf>::new(),
        "no write bit is left in the reused snapshot, its home and pin record included"
    );
    drop(second);
    drop(first);
}

#[test]
fn a_step_cannot_write_into_a_placed_snapshot_and_its_removal_still_works() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pinned = pin(&selection, &work, false).unwrap();
    let home = pinned.home().to_path_buf();
    assert_eq!(writable_entries(&home), Vec::<PathBuf>::new());
    // Root ignores mode bits, so the write refusal is checked only for
    // other users.
    // SAFETY: geteuid has no failure mode and touches no memory.
    if unsafe { libc::geteuid() } != 0 {
        let error = fs::write(home.join("config"), "planted\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(fs::create_dir(home.join(".claude")).is_err());
        assert!(fs::write(home.join("pin.json"), "{}").is_err());
    }
    assert!(!exists_no_follow(&home.join("config")));

    // The last run's release removes the read-only snapshot.
    drop(pinned);
    assert!(!exists_no_follow(&home), "released snapshot is removed");

    // A sweep by another launch removes a read-only snapshot no run holds.
    let first = pin(&selection, &work, false).unwrap();
    let old = first.home().to_path_buf();
    let kept = root.join("kept-old");
    move_snapshot(&old, &kept);
    drop(first);
    let stale = pins_dir(&work).join("0".repeat(24));
    assert_ne!(stale, old, "a key no launch derives");
    move_snapshot(&kept, &stale);
    assert_eq!(mode_of(&stale), 0o555);
    let _second = pin(&selection, &work, false).unwrap();
    assert!(
        !exists_no_follow(&stale),
        "the sweep removed the read-only snapshot"
    );
}

// --------------------------------------------------------------- E12

#[test]
fn an_unsafe_bundle_pins_folder_is_refused_with_e12() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = pins_dir(&work);
    fs::create_dir(&pins).unwrap();
    set_mode(&pins, 0o777);
    let text = err_text(pin(&selection, &work, false));
    assert!(
        text.contains(&format!(
            "untrusted bundle snapshot directory {}",
            pins.display()
        )),
        "{text}"
    );
    assert!(pin_entries(&work).is_empty(), "nothing is written");
    set_mode(&pins, 0o700);
    fs::remove_dir(&pins).unwrap();

    // A bundle-pins that is a symlink to a private folder is refused too.
    let target = root.join("elsewhere-pins");
    fs::create_dir(&target).unwrap();
    set_mode(&target, 0o700);
    symlink(&target, &pins).unwrap();
    assert!(pin(&selection, &work, false).is_err());
    assert_eq!(
        fs::read_dir(&target).unwrap().count(),
        0,
        "nothing written through the link"
    );
}

#[test]
fn a_lease_file_that_is_a_symlink_is_refused_and_its_target_untouched() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let home = pin(&selection, &work, false).unwrap().home().to_path_buf();
    let target = root.join("target-file");
    fs::write(&target, "keep").unwrap();
    set_mode(&target, 0o644);
    symlink(&target, lease_file(&home)).unwrap();

    assert!(pin(&selection, &work, false).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    assert_eq!(mode_of(&target), 0o644);
}

#[test]
fn a_lock_file_that_is_a_symlink_is_refused() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pins = private_pins(&work);
    let target = root.join("target-lock");
    fs::write(&target, "keep").unwrap();
    symlink(&target, pins.join(".lock")).unwrap();
    assert!(pin(&selection, &work, false).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    assert!(!pin_entries(&work).iter().any(|n| is_key(n)), "no snapshot");
}

// ------------------------------------------------------------ state home

#[test]
fn a_state_home_under_the_runtime_folder_or_the_snapshots_is_refused_with_e8() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let runtime = work.join("runs/r/runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &work, false).unwrap();
    let pins = pins_dir(&work);
    let rt = Some(runtime.as_path());

    let in_runtime = runtime.join("x");
    assert_eq!(
        err_text(check_state_home(&in_runtime, &pinned, rt)),
        format!(
            "AMPLIHACK_STATE_HOME {} lies under a run's runtime/ folder",
            in_runtime.display()
        )
    );
    let in_snapshot = pinned.home().to_path_buf();
    assert_eq!(
        err_text(check_state_home(&in_snapshot, &pinned, rt)),
        format!(
            "AMPLIHACK_STATE_HOME {} lies under the bundle snapshots in {}",
            in_snapshot.display(),
            pins.display()
        )
    );
    // Anywhere under bundle-pins, including another run's key.
    for path in [
        pins.clone(),
        pins.join(KEY2).join("state"),
        pins.join(".lock"),
    ] {
        assert!(
            check_state_home(&path, &pinned, rt).is_err(),
            "{}",
            path.display()
        );
    }
    for path in [
        root.join("state"),
        work.join("runs/r/runtime-old"),
        work.join("bundle-pins-old"),
        work.join("runs/r/state"),
    ] {
        assert!(
            check_state_home(&path, &pinned, rt).is_ok(),
            "{}",
            path.display()
        );
    }
}

#[test]
fn a_state_home_linked_into_the_runtime_folder_or_the_snapshots_is_refused() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let runtime = work.join("runs/r/runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &work, false).unwrap();
    let rt = Some(runtime.as_path());
    for (name, target) in [
        ("runtime-link", runtime.clone()),
        ("pins-link", pins_dir(&work)),
    ] {
        let link = root.join(name);
        symlink(&target, &link).unwrap();
        assert!(check_state_home(&link, &pinned, rt).is_err(), "{name}");
        // Not created yet: the nearest existing ancestor is resolved.
        assert!(
            check_state_home(&link.join("later/state"), &pinned, rt).is_err(),
            "{name}"
        );
    }
    assert!(check_state_home(&root.join("state/later"), &pinned, rt).is_ok());
}

#[test]
fn a_dry_run_has_no_state_home_to_check() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let work = work_root(&root);
    let pinned = pin(&selection, &work, true).unwrap();
    assert!(check_state_home(&work.join("bundle-pins/x"), &pinned, None).is_ok());
}

// ------------------------------------------------------------ work root

#[test]
fn a_run_workspace_names_its_work_root_and_a_dry_runtime_has_none() {
    use crate::commands::cleanup::run_dir::{RunScope, RunWorkspace};
    use crate::commands::cleanup::work_root::WorkRoot;
    let (_tmp, root) = scratch();
    let work = work_root(&root);
    let (scope, guard) = RunScope::begin(
        WorkRoot {
            path: work.clone(),
            explicit: true,
        },
        "probe",
        &root,
    )
    .unwrap();
    let workspace = RunWorkspace::Scoped {
        scope: Box::new(scope),
        guard,
    };
    assert_eq!(workspace.work_root(), Some(work.as_path()));
    // The snapshot folder sits beside runs/, never under the run's runtime/.
    assert!(!pins_dir(&work).starts_with(workspace.runtime_dir()));
    assert!(RunWorkspace::dry_runtime().unwrap().work_root().is_none());
}

// ------------------------------------------------------------ log record

#[test]
fn bundle_record_serialises_optional_fields_only_when_present() {
    let record = BundleRecord {
        source: "/src/amplifier-bundle".into(),
        version: "1.0.0".into(),
        snapshot: None,
        choice: "installed".into(),
        inherited: false,
        recipe_file: None,
    };
    let json = serde_json::to_value(&record).unwrap();
    assert_eq!(json["source"], "/src/amplifier-bundle");
    assert_eq!(json["version"], "1.0.0");
    assert_eq!(json["choice"], "installed");
    assert_eq!(json["inherited"], false);
    assert!(json.get("snapshot").is_none(), "{json}");
    assert!(json.get("recipe_file").is_none(), "{json}");

    let full = BundleRecord {
        snapshot: Some("/work/bundle-pins/0123456789abcdef01234567".into()),
        recipe_file: Some("/work/x.yaml".into()),
        ..record
    };
    let json = serde_json::to_value(&full).unwrap();
    assert_eq!(
        json["snapshot"],
        "/work/bundle-pins/0123456789abcdef01234567"
    );
    assert_eq!(json["recipe_file"], "/work/x.yaml");
}
