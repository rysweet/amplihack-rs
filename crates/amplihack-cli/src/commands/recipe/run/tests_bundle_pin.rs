//! Issue #2528: one bundle per recipe run. Unit tests for `bundle_pin.rs`.
//!
//! Wire with, in `run/mod.rs`:
//! `mod bundle_pin;` and
//! `#[cfg(test)] #[path = "tests_bundle_pin.rs"] mod tests_bundle_pin;`
//!
//! Contract (docs/reference/recipe-run-bundle.md):
//! - `BundleChoice { Installed, Checkout }` with `parse`, `from_inputs`, `as_str`;
//!   derives `Debug, Clone, Copy, PartialEq, Eq`.
//! - `SelectInputs<'a>` with the `pub(crate)` fields used by `inputs()` below.
//!   `select_from` reads nothing from the process environment.
//! - `Selection` with `source()` (canonical `<home>/amplifier-bundle`),
//!   `recipes_dir()`, `choice()`, `version()`, `inherited()` and `notes()`
//!   (stderr notes, such as a skipped checkout or an ignored marker).
//! - `pin(&Selection, runtime_dir, dry_run) -> PinnedBundle` with `home()`,
//!   `bundle()`, `source()` and `record()`.
//! - `check_state_home(state_home, &PinnedBundle)`: E8.
//!
//! Every test builds its own temporary homes; none reads or changes the
//! process environment, the real `~/.amplihack` or the working directory.

#![cfg(unix)]

use super::bundle_pin::{BundleChoice, SelectInputs, check_state_home, pin, select_from};
use super::correlation::BundleRecord;
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

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let root = fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

fn os(path: &Path) -> Option<&OsStr> {
    Some(path.as_os_str())
}

/// Inputs with no candidates, no marker and no choice.
fn inputs(working_dir: &Path) -> SelectInputs<'_> {
    SelectInputs {
        choice: None,
        working_dir,
        amplihack_home: None,
        configured_home: None,
        user_home: None,
        run_bundle: None,
        run_bundle_choice: None,
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

// ------------------------------------------------------------ trust

#[test]
fn a_group_writable_bundle_root_is_refused_with_no_fallback() {
    let (_tmp, root) = scratch();
    let checkout = root.join("checkout");
    let bundle = write_checkout(&checkout, "B");
    set_mode(&bundle, 0o775);
    let installed = root.join("installed");
    write_bundle(&installed, "A");

    // Checkout requested: refused, never falls back to the installed bundle.
    let text = err_text(select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        amplihack_home: os(&installed),
        ..inputs(&checkout)
    }));
    assert!(text.contains("amplifier-bundle"), "{text}");
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
    let runtime = root.join("run/runtime");
    fs::create_dir_all(&runtime).unwrap();

    for dry_run in [true, false] {
        let text = err_text(pin(&selection, &runtime, dry_run));
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

/// A parent run's snapshot at `<root>/runs/r1/runtime/bundle`.
fn parent_snapshot(root: &Path) -> PathBuf {
    let marker = root.join("runs/r1/runtime/bundle");
    fs::create_dir_all(&marker).unwrap();
    write_bundle(&marker, "parent-A");
    for dir in ["runs", "runs/r1"] {
        set_mode(&root.join(dir), 0o755);
    }
    set_mode(&root.join("runs/r1/runtime"), 0o700);
    marker
}

#[test]
fn a_valid_marker_is_reused_without_selection() {
    let (_tmp, root) = scratch();
    let marker = parent_snapshot(&root);
    let checkout = root.join("checkout");
    write_checkout(&checkout, "B");

    for choice in [None, Some(BundleChoice::Installed)] {
        let selection = select_from(SelectInputs {
            choice,
            amplihack_home: os(&marker),
            run_bundle: os(&marker),
            run_bundle_choice: Some(OsStr::new("installed")),
            ..inputs(&checkout)
        })
        .unwrap();
        assert!(selection.inherited(), "choice {choice:?}");
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
    let marker = parent_snapshot(&root);
    let checkout = root.join("checkout");
    write_checkout(&checkout, "B");
    let text = err_text(select_from(SelectInputs {
        choice: Some(BundleChoice::Checkout),
        amplihack_home: os(&marker),
        run_bundle: os(&marker),
        run_bundle_choice: Some(OsStr::new("installed")),
        ..inputs(&checkout)
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
    let marker = parent_snapshot(&root);
    let installed = root.join("installed");
    let installed_bundle = write_bundle(&installed, "A");
    let elsewhere = root.join("elsewhere/bundle");
    fs::create_dir_all(&elsewhere).unwrap();
    write_bundle(&elsewhere, "X");

    // (AMPLIHACK_HOME, marker, marker choice) combinations that each fail one check.
    let cases: Vec<(&Path, &Path, &str, &str)> = vec![
        (&installed, &marker, "installed", "marker != AMPLIHACK_HOME"),
        (
            &elsewhere,
            &elsewhere,
            "installed",
            "not under runtime/bundle",
        ),
        (&marker, &marker, "Installed", "bad choice"),
        (&marker, &marker, "", "empty choice"),
    ];
    for (home, run_bundle, choice, why) in cases {
        let selection = select_from(SelectInputs {
            amplihack_home: os(home),
            run_bundle: os(run_bundle),
            run_bundle_choice: Some(OsStr::new(choice)),
            configured_home: Some(&installed),
            ..inputs(&root)
        })
        .unwrap_or_else(|e| panic!("{why}: {e:#}"));
        assert!(!selection.inherited(), "{why}");
        assert!(!selection.notes().is_empty(), "{why}: a note is printed");
        if home == installed.as_path() {
            assert_eq!(
                selection.source(),
                fs::canonicalize(&installed_bundle).unwrap()
            );
        }
    }
}

#[test]
fn inherited_marker_failing_trust_is_ignored() {
    let (_tmp, root) = scratch();
    let marker = parent_snapshot(&root);
    set_mode(&marker.join("amplifier-bundle"), 0o777);
    let installed = root.join("installed");
    let installed_bundle = write_bundle(&installed, "A");
    let selection = select_from(SelectInputs {
        amplihack_home: os(&marker),
        run_bundle: os(&marker),
        run_bundle_choice: Some(OsStr::new("installed")),
        configured_home: Some(&installed),
        ..inputs(&root)
    })
    .unwrap();
    assert!(!selection.inherited());
    assert_eq!(
        selection.source(),
        fs::canonicalize(&installed_bundle).unwrap()
    );
}

// ------------------------------------------------------------------ pin

fn installed_selection(root: &Path) -> (PathBuf, super::bundle_pin::Selection) {
    let home = root.join("installed");
    let bundle = write_bundle(&home, "installed-A");
    let selection = select_from(SelectInputs {
        amplihack_home: os(&home),
        ..inputs(root)
    })
    .unwrap();
    (bundle, selection)
}

#[test]
fn pin_copies_into_runtime_bundle_with_explicit_modes() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let runtime = root.join("runs/r/runtime");
    fs::create_dir_all(&runtime).unwrap();
    set_mode(&runtime, 0o700);

    let pinned = pin(&selection, &runtime, false).unwrap();
    assert_eq!(pinned.home(), runtime.join("bundle"));
    assert_eq!(pinned.bundle(), runtime.join("bundle/amplifier-bundle"));
    assert_eq!(pinned.source(), fs::canonicalize(&source).unwrap());
    assert_eq!(mode_of(pinned.home()), 0o755);
    assert_eq!(mode_of(pinned.bundle()), 0o755);
    assert_eq!(mode_of(&pinned.bundle().join("tools/identity.sh")), 0o755);
    assert_eq!(mode_of(&pinned.bundle().join("recipes/probe.yaml")), 0o644);
    assert!(
        fs::read_to_string(pinned.bundle().join("tools/identity.sh"))
            .unwrap()
            .contains("installed-A")
    );

    let record = pinned.record();
    assert_eq!(record.choice, "installed");
    assert!(!record.inherited);
    assert_eq!(
        record.snapshot.as_deref(),
        Some(runtime.join("bundle").to_str().unwrap())
    );
    assert_eq!(record.source, pinned.source().to_str().unwrap());
}

#[test]
fn the_snapshot_survives_an_upgrade_of_the_source() {
    let (_tmp, root) = scratch();
    let (source, selection) = installed_selection(&root);
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &runtime, false).unwrap();

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
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &runtime, true).unwrap();
    assert!(!runtime.join("bundle").exists());
    assert_eq!(pinned.bundle(), selection.source());
    assert_eq!(pinned.record().snapshot, None);
}

#[test]
fn an_inherited_selection_pins_the_parent_snapshot_without_copying() {
    let (_tmp, root) = scratch();
    let marker = parent_snapshot(&root);
    let selection = select_from(SelectInputs {
        amplihack_home: os(&marker),
        run_bundle: os(&marker),
        run_bundle_choice: Some(OsStr::new("installed")),
        ..inputs(&root)
    })
    .unwrap();
    let runtime = root.join("child-run/runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &runtime, false).unwrap();
    assert_eq!(pinned.home(), fs::canonicalize(&marker).unwrap());
    assert!(!runtime.join("bundle").exists(), "no second copy");
    assert!(pinned.record().inherited);
}

#[test]
fn the_runtime_dir_is_not_widened() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    set_mode(&runtime, 0o700);
    pin(&selection, &runtime, false).unwrap();
    assert_eq!(mode_of(&runtime), 0o700);
}

// ------------------------------------------------------------ state home

#[test]
fn a_state_home_under_the_run_runtime_folder_is_refused_with_e8() {
    let (_tmp, root) = scratch();
    let (_source, selection) = installed_selection(&root);
    let runtime = root.join("runs/r/runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &runtime, false).unwrap();

    let inside = pinned.home().to_path_buf();
    assert_eq!(
        err_text(check_state_home(&inside, &pinned)),
        format!(
            "AMPLIHACK_STATE_HOME {} lies under a run's runtime/ folder",
            inside.display()
        )
    );
    assert!(check_state_home(&runtime.join("x"), &pinned).is_err());
    assert!(check_state_home(&root.join("state"), &pinned).is_ok());
    // A sibling whose name only starts with `runtime` is not inside.
    assert!(check_state_home(&root.join("runs/r/runtime-old"), &pinned).is_ok());
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
        snapshot: Some("/runs/r/runtime/bundle".into()),
        recipe_file: Some("/work/x.yaml".into()),
        ..record
    };
    let json = serde_json::to_value(&full).unwrap();
    assert_eq!(json["snapshot"], "/runs/r/runtime/bundle");
    assert_eq!(json["recipe_file"], "/work/x.yaml");
}

#[test]
fn a_marker_in_the_short_runtime_layout_is_reused() {
    // `run_dir::runtime_dir_for` uses `<work root>/r/<12 hex>` for long paths.
    let (_tmp, root) = scratch();
    let marker = root.join("work/r/0123456789ab/bundle");
    fs::create_dir_all(&marker).unwrap();
    write_bundle(&marker, "parent-A");
    for dir in ["work", "work/r", "work/r/0123456789ab"] {
        set_mode(&root.join(dir), 0o700);
    }
    let selection = select_from(SelectInputs {
        amplihack_home: os(&marker),
        run_bundle: os(&marker),
        run_bundle_choice: Some(OsStr::new("installed")),
        ..inputs(&root)
    })
    .unwrap();
    assert!(selection.inherited(), "{:?}", selection.notes());

    // A folder named like a short id but not under `r/` is not a snapshot.
    let other = root.join("work/x/0123456789ab/bundle");
    fs::create_dir_all(&other).unwrap();
    write_bundle(&other, "X");
    let installed = root.join("installed");
    write_bundle(&installed, "A");
    let selection = select_from(SelectInputs {
        configured_home: Some(&installed),
        amplihack_home: os(&other),
        run_bundle: os(&other),
        run_bundle_choice: Some(OsStr::new("installed")),
        ..inputs(&root)
    })
    .unwrap();
    assert!(!selection.inherited());
}
