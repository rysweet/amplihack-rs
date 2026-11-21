//! Unit tests for `build_output.rs` (issue #2886): at run end the run removes
//! the build output it created for itself.
//!
//! Wire with, in `build_output.rs`:
//! `#[cfg(test)] #[path = "tests_build_output.rs"] mod tests;`
//!
//! Contract these tests fix:
//! - `RunEnd { run_dir: PathBuf, cwd: PathBuf, work_root: PathBuf }`: what the
//!   ending run is (its folder, the folder it was launched in, its work root);
//! - `Launch { cargo_target_dir: Option<PathBuf>, private_target: bool,
//!   nested: bool }`, with `Launch::from_env()` reading `CARGO_TARGET_DIR`,
//!   `AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR` (exactly `1` after trimming) and a
//!   non-empty `AMPLIHACK_RUN_ID`;
//! - `end_of_run_with(&RunEnd, &Launch, &mut dyn Write)`: returns unit on every
//!   path; one `amplihack: ...` line per removal, per refusal and per kept
//!   worktree on the sink; `end_of_run_in` is the same with the [`Ctx`] given
//!   rather than read from the environment, and `Ctx::with_home` builds one
//!   with the home folder given;
//! - pure formatters `removed_line(&Path, Option<u64>) -> String`,
//!   `total_line(count: usize, known_bytes: u64, any_unknown: bool) -> String`,
//!   `warning_line(&Path, &str) -> String` and `skipped_line(&Path, &str) ->
//!   String`, each ending in one newline;
//! - `same_device(a: u64, b: u64) -> bool` is the pure device comparison;
//! - `REASON_IN_USE`, `REASON_NOT_CARGO` and `REASON_FOREIGN_OWNER` are the
//!   guard reason texts, equal to the `Display` of the matching
//!   `target_remove::Refusal` (that type's visibility is widened as needed).
//!
//! Every fixture lives under a 0700 `tempfile` root. `HOME` is redirected to a
//! temporary folder for the home guards. No real home, `/` or `/tmp` is a
//! removal candidate.

use super::*;
use crate::commands::cleanup::human_size;
use crate::test_support::origin_clones::{Scratch, git, make_dirty, make_unpushed};
use crate::test_support::{EnvGuard, env_lock};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn private_root() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    t
}

/// A folder Cargo would have made, holding `bytes` bytes.
fn cargo_target(at: &Path, bytes: usize) -> PathBuf {
    fs::create_dir_all(at.join("debug/.fingerprint")).unwrap();
    fs::write(
        at.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    fs::write(at.join("debug/blob"), vec![7u8; bytes]).unwrap();
    at.to_path_buf()
}

struct Fx {
    _tmp: tempfile::TempDir,
    _scratch: Scratch,
    root: PathBuf,
    /// A main checkout (its `.git` is a folder).
    main: PathBuf,
    run_dir: PathBuf,
    work_root: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = private_root();
        let scratch = Scratch::new();
        let root = fs::canonicalize(tmp.path()).unwrap();
        let main = scratch.root.join("main");
        scratch.origin.clone_to(&main);
        let run_dir = root.join("work/runs/run-1");
        fs::create_dir_all(&run_dir).unwrap();
        let work_root = root.join("work");
        Self {
            _tmp: tmp,
            _scratch: scratch,
            root,
            main,
            run_dir,
            work_root,
        }
    }

    /// A linked worktree with nothing unsaved in it.
    fn worktree(&self, name: &str) -> PathBuf {
        let path = self.work_root.join("worktrees").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        git(
            &self.main,
            &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
        );
        path
    }

    fn own(&self, worktree: &Path) {
        let mut text = fs::read_to_string(self.run_dir.join("owned-paths")).unwrap_or_default();
        text.push_str(&format!("worktree\t{}\t\n", worktree.display()));
        fs::write(self.run_dir.join("owned-paths"), text).unwrap();
    }

    fn run(&self, cwd: &Path) -> RunEnd {
        RunEnd {
            run_dir: self.run_dir.clone(),
            cwd: cwd.to_path_buf(),
            work_root: self.work_root.clone(),
        }
    }
}

fn launch(claimed: Option<&Path>, private: bool, nested: bool) -> Launch {
    Launch {
        cargo_target_dir: claimed.map(Path::to_path_buf),
        private_target: private,
        nested,
    }
}

fn run_end(fx: &Fx, cwd: &Path, l: &Launch) -> String {
    let mut sink: Vec<u8> = Vec::new();
    end_of_run_with(&fx.run(cwd), l, &mut sink);
    String::from_utf8(sink).unwrap()
}

/// [`run_end`] with the home folder given rather than read from `HOME`.
fn run_end_with_home(fx: &Fx, cwd: &Path, l: &Launch, home: Option<PathBuf>) -> String {
    let end = fx.run(cwd);
    let ctx = Ctx::with_home(&end, home);
    let mut sink: Vec<u8> = Vec::new();
    end_of_run_in(&end, l, &ctx, &mut sink);
    String::from_utf8(sink).unwrap()
}

fn lines(out: &str) -> Vec<&str> {
    out.lines().filter(|l| !l.trim().is_empty()).collect()
}

// ---- the claimed folder ----------------------------------------------------

#[test]
fn a_claimed_private_target_is_removed_and_reported_with_its_size() {
    let fx = Fx::new();
    let t = cargo_target(&fx.root.join("claimed"), 4096);
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert!(!t.exists());
    let l = lines(&out);
    assert_eq!(l.len(), 1, "{out}");
    assert!(l[0].starts_with("amplihack: "), "{out}");
    assert!(l[0].contains(t.to_str().unwrap()), "{out}");
}

#[test]
fn a_valid_cargo_folder_that_was_not_claimed_is_left_alone_and_silent() {
    let fx = Fx::new();
    let t = cargo_target(&fx.root.join("shared"), 1024);
    let out = run_end(&fx, &fx.main, &launch(Some(&t), false, false));
    assert!(t.join("debug/blob").is_file());
    assert!(out.is_empty(), "{out}");
}

#[test]
fn an_unset_or_missing_target_dir_is_a_silent_no_op() {
    let fx = Fx::new();
    assert!(run_end(&fx, &fx.main, &launch(None, true, false)).is_empty());
    let missing = fx.root.join("never-made");
    assert!(run_end(&fx, &fx.main, &launch(Some(&missing), true, false)).is_empty());
}

#[test]
fn an_empty_claimed_folder_is_removed_without_creating_a_lock_file() {
    let fx = Fx::new();
    let t = fx.root.join("empty");
    fs::create_dir(&t).unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert!(!t.exists(), "{out}");
}

#[test]
fn a_relative_claimed_path_is_refused_with_one_warning() {
    let fx = Fx::new();
    let out = run_end(
        &fx,
        &fx.main,
        &launch(Some(Path::new("relative/target")), true, false),
    );
    let l = lines(&out);
    assert_eq!(l.len(), 1, "{out}");
    assert!(l[0].starts_with("amplihack: warning: end-of-run cleanup could not remove "));
}

#[test]
fn a_claimed_symbolic_link_is_refused_and_its_destination_is_untouched() {
    let fx = Fx::new();
    let real = cargo_target(&fx.root.join("real"), 512);
    let link = fx.root.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&link), true, false));
    assert!(real.join("debug/blob").is_file());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(out.contains("warning"), "{out}");
}

#[test]
fn a_claimed_file_is_refused() {
    let fx = Fx::new();
    let f = fx.root.join("a-file");
    fs::write(&f, "x").unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&f), true, false));
    assert!(f.is_file());
    assert!(out.contains("warning"), "{out}");
}

#[test]
fn a_folder_without_cargo_markers_is_refused_and_kept() {
    let fx = Fx::new();
    let t = fx.root.join("photos");
    fs::create_dir(&t).unwrap();
    fs::write(t.join("a.jpg"), "x").unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert!(t.join("a.jpg").is_file());
    assert_eq!(lines(&out).len(), 1, "{out}");
    assert!(out.contains("warning"), "{out}");
}

#[test]
fn a_claimed_checkout_root_is_refused_whether_git_is_a_file_or_a_folder() {
    let fx = Fx::new();
    let as_dir = cargo_target(&fx.root.join("with-git-dir"), 16);
    fs::create_dir(as_dir.join(".git")).unwrap();
    let as_file = cargo_target(&fx.root.join("with-git-file"), 16);
    fs::write(as_file.join(".git"), "gitdir: elsewhere\n").unwrap();
    let nested = cargo_target(&fx.root.join("with-child-repo"), 16);
    fs::create_dir_all(nested.join("child/.git")).unwrap();
    for t in [&as_dir, &as_file, &nested] {
        let out = run_end(&fx, &fx.main, &launch(Some(t), true, false));
        assert!(t.join("debug/blob").is_file(), "{t:?} was removed");
        assert!(out.contains("warning"), "{out}");
    }
}

#[test]
fn a_claimed_folder_that_holds_the_launch_folder_or_the_work_root_is_refused() {
    let fx = Fx::new();
    let outer = cargo_target(&fx.root, 16);
    let out = run_end(&fx, &fx.main, &launch(Some(&outer), true, false));
    assert!(fx.work_root.exists() && fx.main.exists(), "{out}");
    assert!(out.contains("warning"), "{out}");
    // Equal to the work root.
    let out = run_end(&fx, &fx.main, &launch(Some(&fx.work_root), true, false));
    assert!(fx.run_dir.exists(), "{out}");
    // Equal to the launch folder.
    let out = run_end(&fx, &fx.main, &launch(Some(&fx.main), true, false));
    assert!(fx.main.join(".git").exists(), "{out}");
}

#[test]
fn the_home_folder_and_its_ancestors_are_never_removed() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let fx = Fx::new();
    let home = fx.root.join("home/user");
    cargo_target(&home, 16);
    let _env = EnvGuard::set([("HOME", home.to_str().unwrap())]);
    for claimed in [home.clone(), home.parent().unwrap().to_path_buf()] {
        let out = run_end(&fx, &fx.main, &launch(Some(&claimed), true, false));
        assert!(home.join("debug/blob").is_file(), "{claimed:?}: {out}");
        assert!(out.contains("warning"), "{out}");
    }
    let out = run_end(&fx, &fx.main, &launch(Some(Path::new("/")), true, false));
    assert!(out.contains("warning"), "{out}");
}

#[test]
fn a_claimed_folder_is_refused_when_the_home_folder_is_unknown() {
    let fx = Fx::new();
    let t = cargo_target(&fx.root.join("claimed"), 64);
    let out = run_end_with_home(&fx, &fx.main, &launch(Some(&t), true, false), None);
    assert!(t.join("debug/blob").is_file(), "{out}");
    let l = lines(&out);
    assert_eq!(l.len(), 1, "{out}");
    assert!(
        l[0].starts_with("amplihack: warning: end-of-run cleanup could not remove "),
        "{out}"
    );
    assert!(l[0].contains("home folder is unknown"), "{out}");
}

#[test]
fn an_unknown_home_folder_does_not_stop_a_worktree_target_being_removed() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-no-home");
    cargo_target(&wt.join("target"), 64);
    fx.own(&wt);
    let out = run_end_with_home(&fx, &fx.root, &launch(None, false, false), None);
    assert!(!wt.join("target").exists(), "{out}");
}

#[test]
fn a_known_home_folder_still_allows_the_claimed_folder_through() {
    let fx = Fx::new();
    let t = cargo_target(&fx.root.join("claimed"), 64);
    let home = fx.root.join("home/user");
    fs::create_dir_all(&home).unwrap();
    let out = run_end_with_home(&fx, &fx.main, &launch(Some(&t), true, false), Some(home));
    assert!(!t.exists(), "{out}");
}

#[test]
fn a_hostile_folder_name_yields_exactly_one_line() {
    let fx = Fx::new();
    let t = fx.root.join("evil\n\u{1b}[31mname");
    fs::create_dir(&t).unwrap();
    fs::write(t.join("x"), "x").unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert_eq!(out.matches('\n').count(), 1, "{out:?}");
    assert!(!out.contains('\u{1b}'), "{out:?}");
    assert!(out.starts_with("amplihack: "), "{out:?}");
}

#[test]
fn a_held_cargo_lock_keeps_the_folder_with_one_warning_and_no_wait() {
    let fx = Fx::new();
    let t = cargo_target(&fx.root.join("busy"), 64);
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(t.join("debug/.cargo-lock"))
        .unwrap();
    assert!(
        lock.try_lock().is_ok(),
        "the fixture could not take the lock"
    );
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert!(t.join("debug/blob").is_file());
    let l = lines(&out);
    assert_eq!(l.len(), 1, "{out}");
    assert!(l[0].contains("warning"), "{out}");
    assert!(l[0].contains("cargo-lock"), "{out}");
    drop(lock);
}

// ---- worktrees -------------------------------------------------------------

#[test]
fn an_owned_worktree_target_is_removed_and_the_worktree_stays() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-a");
    cargo_target(&wt.join("target"), 2048);
    fx.own(&wt);
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(!wt.join("target").exists(), "{out}");
    assert!(
        wt.join(".git").is_file(),
        "the worktree itself is never removed"
    );
    assert_eq!(lines(&out).len(), 1, "{out}");
}

#[test]
fn the_launch_folder_worktree_target_is_removed_for_an_outermost_run() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-b");
    cargo_target(&wt.join("target"), 128);
    run_end(&fx, &wt, &launch(None, false, false));
    assert!(!wt.join("target").exists());
}

#[test]
fn a_nested_run_leaves_the_launch_folder_target_and_the_inherited_target_dir() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-c");
    let shared = cargo_target(&wt.join("target"), 128);
    let owned = fx.worktree("feat-c-owned");
    let owned_target = cargo_target(&owned.join("target"), 128);
    fx.own(&owned);
    // Inherited CARGO_TARGET_DIR is the parent's, and the flag is cleared for
    // steps, so the nested run does not claim it either.
    let out = run_end(&fx, &wt, &launch(Some(&shared), false, true));
    assert!(shared.join("debug/blob").is_file(), "{out}");
    assert!(!owned_target.exists(), "owned worktrees are still handled");
}

#[test]
fn a_main_checkout_target_is_never_removed() {
    let fx = Fx::new();
    let t = cargo_target(&fx.main.join("target"), 128);
    run_end(&fx, &fx.main, &launch(None, false, false));
    assert!(t.join("debug/blob").is_file());
}

#[test]
fn an_owned_paths_entry_naming_a_main_checkout_is_left_alone_and_said_once() {
    let fx = Fx::new();
    let t = cargo_target(&fx.main.join("target"), 128);
    fx.own(&fx.main);
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(t.join("debug/blob").is_file(), "{out}");
    let l = lines(&out);
    assert_eq!(l.len(), 1, "{out}");
    assert!(
        l[0].starts_with("amplihack: warning: end-of-run cleanup skipped "),
        "{out}"
    );
    assert!(l[0].contains("not a linked worktree"), "{out}");
    assert!(l[0].contains(fx.main.to_str().unwrap()), "{out}");
}

#[test]
fn the_launch_folder_is_never_reported_as_not_a_worktree() {
    let fx = Fx::new();
    // A main checkout is the ordinary launch folder; it must stay silent.
    cargo_target(&fx.main.join("target"), 128);
    let out = run_end(&fx, &fx.main, &launch(None, false, false));
    assert!(out.is_empty(), "{out}");
}

#[test]
fn a_recorded_worktree_that_no_longer_exists_is_silent() {
    let fx = Fx::new();
    let gone = fx.work_root.join("worktrees/already-removed");
    fx.own(&gone);
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(out.is_empty(), "{out}");
}

#[test]
fn an_unreadable_owned_paths_file_is_reported_and_removes_nothing() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-unreadable");
    let t = cargo_target(&wt.join("target"), 64);
    // Larger than the parser's cap, which fails the read the same way for
    // every user, unlike a mode the test's own uid might ignore.
    fs::write(fx.run_dir.join("owned-paths"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(t.join("debug/blob").is_file(), "{out}");
    let l = lines(&out);
    assert_eq!(l.len(), 1, "{out}");
    assert!(
        l[0].starts_with("amplihack: warning: end-of-run cleanup skipped "),
        "{out}"
    );
    assert!(l[0].contains("owned-paths"), "{out}");
}

#[test]
fn a_symbolic_link_named_target_in_a_worktree_is_not_followed() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-d");
    let real = cargo_target(&fx.root.join("elsewhere"), 64);
    std::os::unix::fs::symlink(&real, wt.join("target")).unwrap();
    fx.own(&wt);
    run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(real.join("debug/blob").is_file());
}

#[test]
fn a_worktree_with_uncommitted_work_is_kept_and_its_target_is_removed() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-e");
    make_dirty(&wt);
    cargo_target(&wt.join("target"), 256);
    fx.own(&wt);
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(wt.join("unsaved.txt").is_file());
    assert!(!wt.join("target").exists(), "{out}");
}

#[test]
fn a_worktree_with_unpushed_commits_is_kept_and_its_target_is_removed() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-f");
    make_unpushed(&wt);
    cargo_target(&wt.join("target"), 256);
    fx.own(&wt);
    run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(wt.join(".git").is_file());
    assert!(!wt.join("target").exists());
}

#[test]
fn the_same_folder_named_twice_is_removed_and_reported_once() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-g");
    let t = cargo_target(&wt.join("target"), 256);
    fx.own(&wt);
    let out = run_end(&fx, &wt, &launch(Some(&t), true, false));
    assert!(!t.exists());
    assert_eq!(lines(&out).len(), 1, "{out}");
}

#[test]
fn two_removals_end_with_a_total_line() {
    let fx = Fx::new();
    let a = cargo_target(&fx.root.join("a"), 1000);
    let wt = fx.worktree("feat-h");
    cargo_target(&wt.join("target"), 1000);
    fx.own(&wt);
    let out = run_end(&fx, &fx.root, &launch(Some(&a), true, false));
    let l = lines(&out);
    assert_eq!(l.len(), 3, "{out}");
    assert!(l[2].to_lowercase().contains("total"), "{out}");
}

#[test]
fn one_removal_has_no_total_line() {
    let fx = Fx::new();
    let a = cargo_target(&fx.root.join("a"), 1000);
    let out = run_end(&fx, &fx.main, &launch(Some(&a), true, false));
    assert!(!out.to_lowercase().contains("total"), "{out}");
}

// ---- owned-paths is untrusted, and other security cases ---------------------

#[test]
fn an_owned_paths_entry_that_is_not_a_linked_worktree_is_ignored() {
    let fx = Fx::new();
    // A plain folder with a cargo-shaped target/ and no `.git` at all.
    let plain = fx.root.join("plain");
    let t = cargo_target(&plain.join("target"), 64);
    fx.own(&plain);
    // A `.git` file that does not begin with `gitdir:`.
    let forged = fx.root.join("forged");
    let ft = cargo_target(&forged.join("target"), 64);
    fs::write(forged.join(".git"), "not a gitdir pointer\n").unwrap();
    fx.own(&forged);
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(t.join("debug/blob").is_file());
    assert!(ft.join("debug/blob").is_file());
    // One line each: a recorded entry that was never a candidate is said.
    let l = lines(&out);
    assert_eq!(l.len(), 2, "{out}");
    assert!(
        l.iter()
            .all(|line| line.contains("skipped") && line.contains("not a linked worktree")),
        "{out}"
    );
}

#[test]
fn a_relative_owned_paths_entry_is_ignored() {
    let fx = Fx::new();
    let mut text = fs::read_to_string(fx.run_dir.join("owned-paths")).unwrap_or_default();
    text.push_str("worktree\t../../elsewhere\t\n");
    fs::write(fx.run_dir.join("owned-paths"), text).unwrap();
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(!out.contains("removed"), "{out}");
}

#[test]
fn a_worktree_target_without_cargo_markers_is_refused_and_kept() {
    let fx = Fx::new();
    let wt = fx.worktree("feat-i");
    fs::create_dir(wt.join("target")).unwrap();
    fs::write(wt.join("target/notes.txt"), "mine").unwrap();
    fx.own(&wt);
    let out = run_end(&fx, &fx.root, &launch(None, false, false));
    assert!(wt.join("target/notes.txt").is_file());
    assert!(out.contains("warning"), "{out}");
}

#[test]
fn a_claimed_folder_in_a_world_writable_parent_without_sticky_bit_is_refused() {
    let fx = Fx::new();
    let parent = fx.root.join("open");
    fs::create_dir(&parent).unwrap();
    let t = cargo_target(&parent.join("target"), 32);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert!(t.join("debug/blob").is_file(), "{out}");
    assert!(out.contains("parent is writable by other users"), "{out}");
}

#[test]
fn a_cargo_folder_with_unrecognised_extra_content_is_not_treated_as_empty() {
    let fx = Fx::new();
    // Markers absent, one stray file: the empty-folder exemption must not apply.
    let t = fx.root.join("stray");
    fs::create_dir(&t).unwrap();
    fs::write(t.join("only-file"), "x").unwrap();
    let out = run_end(&fx, &fx.main, &launch(Some(&t), true, false));
    assert!(t.join("only-file").is_file(), "{out}");
}

#[test]
fn a_reason_with_control_characters_is_escaped_in_the_warning() {
    let line = warning_line(Path::new("/w/t"), "bad\n\u{1b}[31mreason");
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
    assert!(!line.contains('\u{1b}'), "{line:?}");
}

#[test]
fn removal_never_changes_the_sink_for_a_run_that_has_nothing_to_remove() {
    let fx = Fx::new();
    assert!(run_end(&fx, &fx.root, &launch(None, false, false)).is_empty());
}

// ---- formatters ------------------------------------------------------------

#[test]
fn removed_line_names_the_folder_and_the_size() {
    let line = removed_line(Path::new("/w/target"), Some(2048));
    assert!(line.starts_with("amplihack: "));
    assert!(line.contains("/w/target"));
    assert!(line.contains(&human_size(2048)));
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
}

#[test]
fn removed_line_says_unknown_when_the_size_is_unknown() {
    let line = removed_line(Path::new("/w/target"), None);
    assert!(line.contains("size unknown"), "{line}");
}

#[test]
fn total_line_sums_known_sizes_and_says_at_least_when_some_are_unknown() {
    let exact = total_line(2, 3072, false);
    assert!(exact.contains(&human_size(3072)), "{exact}");
    assert!(!exact.contains("at least"));
    let partial = total_line(3, 3072, true);
    assert!(partial.contains("at least"), "{partial}");
    assert!(partial.contains("size unknown"), "{partial}");
}

#[test]
fn warning_line_uses_the_end_of_run_wording_and_escapes_the_path() {
    let line = warning_line(Path::new("/w/a\nb"), "not a cargo target");
    assert!(
        line.starts_with("amplihack: warning: end-of-run cleanup could not remove "),
        "{line}"
    );
    assert!(line.contains("not a cargo target"));
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
}

#[test]
fn skipped_line_names_the_path_and_escapes_the_reason() {
    let line = skipped_line(Path::new("/w/a\nb"), "bad\n\u{1b}[31mreason");
    assert!(
        line.starts_with("amplihack: warning: end-of-run cleanup skipped "),
        "{line}"
    );
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
    assert!(!line.contains('\u{1b}'), "{line:?}");
}

#[test]
fn the_device_comparison_is_pure() {
    assert!(same_device(5, 5));
    assert!(!same_device(5, 6));
}

#[test]
fn the_reason_texts_equal_the_refusal_display() {
    use crate::commands::cleanup::target_remove::Refusal;
    assert_eq!(REASON_IN_USE, Refusal::InUse.to_string());
    assert_eq!(REASON_NOT_CARGO, Refusal::NotCargoTarget.to_string());
    assert_eq!(REASON_FOREIGN_OWNER, Refusal::ForeignOwner.to_string());
}

// ---- reading the launch ----------------------------------------------------

#[test]
fn only_the_value_one_claims_the_target_folder() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvGuard::capture(&["AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR"]);
    for (value, want) in [
        (Some("1"), true),
        (Some(" 1 "), true),
        (Some("true"), false),
        (Some("yes"), false),
        (Some("0"), false),
        (Some(""), false),
        (None, false),
    ] {
        match value {
            Some(v) => unsafe { std::env::set_var("AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR", v) },
            None => unsafe { std::env::remove_var("AMPLIHACK_CLEANUP_PRIVATE_TARGET_DIR") },
        }
        assert_eq!(Launch::from_env().private_target, want, "{value:?}");
        assert_eq!(
            crate::commands::cleanup::private_target_dir_claimed(),
            want,
            "{value:?}"
        );
    }
}

#[test]
fn a_non_empty_run_id_at_start_marks_the_run_as_nested() {
    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvGuard::capture(&["AMPLIHACK_RUN_ID", "CARGO_TARGET_DIR"]);
    unsafe { std::env::set_var("AMPLIHACK_RUN_ID", "parent-run") };
    unsafe { std::env::set_var("CARGO_TARGET_DIR", "/some/where") };
    let l = Launch::from_env();
    assert!(l.nested);
    assert_eq!(l.cargo_target_dir, Some(PathBuf::from("/some/where")));
    unsafe { std::env::set_var("AMPLIHACK_RUN_ID", "") };
    assert!(!Launch::from_env().nested);
    unsafe { std::env::remove_var("AMPLIHACK_RUN_ID") };
    assert!(!Launch::from_env().nested);
}

#[test]
fn end_of_run_never_panics_on_nonsense() {
    let end = RunEnd {
        run_dir: PathBuf::from("/definitely/not/here"),
        cwd: PathBuf::from("relative"),
        work_root: PathBuf::from(""),
    };
    let mut sink: Vec<u8> = Vec::new();
    end_of_run_with(&end, &launch(Some(Path::new("")), true, false), &mut sink);
}
