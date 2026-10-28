//! Issue #1699: fails the test binary if anything in this run changed a real
//! profile's `settings.json`.
//!
//! Compiled into two test binaries: `cli_integration` (through `tests/main.rs`)
//! and the `amplihack-cli` library tests (through
//! `src/commands/install/tests/mod.rs`), where the in-process install tests
//! run. It uses only `std`, `sha2` and `libc`.
//!
//! How it works:
//! - [`arm`] records a baseline once per process: for each real profile root
//!   (the root named by `AMPLIHACK_CLAUDE_PROFILE_ROOT`, `~/.claude-profiles`
//!   and `~/.amplihack/claude-profiles`, for both `$HOME` and the passwd home,
//!   skipping any root under a temporary directory) the length and SHA-256 of
//!   each allow-listed file.
//! - On Linux it is armed before `main`, so changes made by any test are
//!   seen; elsewhere it is armed when the guard test runs.
//! - At exit a `libc::atexit` handler takes a second snapshot. If they differ
//!   it prints the changed paths and ends the process with `_exit(1)`.
//!
//! It is read-only: files are opened with `O_NOFOLLOW`, must be regular files,
//! at most 1 MiB of each is hashed, and only lengths and hashes are kept.
//! Allow-list: `<root>/<entry>/settings.json` for real directories, and
//! `<root>/.auto-switch.d/rotate/root`. Credentials, locks and status files
//! the live rotator changes are never read. Where no real root exists (CI),
//! nothing is checked.
//!
//! Limits: another test binary's changes are not seen, and a change made and
//! reverted within one run is not seen.

#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{CStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The variable naming the profile root.
const PROFILE_ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

/// Set in `cli_integration`'s re-executed quiet children; their parent's
/// check covers them.
const QUIET_CHILD_ENV: &str = "AMPLIHACK_CLI_INTEGRATION_QUIET_CHILD";

/// Bytes hashed per file at most.
const MAX_HASHED: u64 = 1024 * 1024;

/// Path to (length, SHA-256 of the first [`MAX_HASHED`] bytes).
type Snapshot = BTreeMap<PathBuf, (u64, [u8; 32])>;

struct Baseline {
    roots: Vec<PathBuf>,
    files: Snapshot,
}

static BASELINE: OnceLock<Baseline> = OnceLock::new();

/// Canonical temporary-directory bases: `temp_dir()`, `$TMPDIR` and `/tmp`.
fn temp_bases() -> Vec<PathBuf> {
    let mut candidates = vec![std::env::temp_dir(), PathBuf::from("/tmp")];
    if let Some(tmpdir) = std::env::var_os("TMPDIR").map(PathBuf::from)
        && tmpdir.is_absolute()
    {
        candidates.push(tmpdir);
    }
    let mut bases: Vec<PathBuf> = candidates
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    bases.sort();
    bases.dedup();
    bases
}

/// The home directory in the passwd database, which no test can change.
fn passwd_home() -> Option<PathBuf> {
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: an all-zero passwd is a valid out-parameter for getpwuid_r.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call and `buf.len()` is its size;
    // geteuid has no preconditions.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: on success pw_dir points to a NUL-terminated string in `buf`.
    let dir = unsafe { CStr::from_ptr(pwd.pw_dir) };
    Some(PathBuf::from(OsString::from_vec(dir.to_bytes().to_vec())))
}

/// The root candidates for `homes` and an explicit root, kept only where they
/// are existing directories not under any of `temp_bases`. Canonical and
/// de-duplicated.
fn keep_real_roots(
    explicit: Option<PathBuf>,
    homes: &[PathBuf],
    temp_bases: &[PathBuf],
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = explicit.into_iter().collect();
    for home in homes {
        candidates.push(home.join(".claude-profiles"));
        candidates.push(home.join(".amplihack/claude-profiles"));
    }
    let mut roots: Vec<PathBuf> = candidates
        .iter()
        .filter(|p| p.is_absolute())
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| p.is_dir())
        .filter(|p| !temp_bases.iter().any(|base| p.starts_with(base)))
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// The real profile roots of this user, from the process environment and the
/// passwd database.
fn real_roots() -> Vec<PathBuf> {
    let explicit = std::env::var_os(PROFILE_ROOT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let homes: Vec<PathBuf> = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(passwd_home())
        .collect();
    keep_real_roots(explicit, &homes, &temp_bases())
}

/// Length and hash of `path` if it is a regular file, opened without
/// following a symlink.
fn fingerprint(path: &Path) -> Option<(u64, [u8; 32])> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut head = Vec::new();
    file.take(MAX_HASHED).read_to_end(&mut head).ok()?;
    Some((meta.len(), Sha256::digest(&head).into()))
}

/// The allow-listed files under `roots`.
fn snapshot(roots: &[PathBuf]) -> Snapshot {
    let mut files = Snapshot::new();
    for root in roots {
        let mut paths = vec![root.join(".auto-switch.d/rotate/root")];
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                if is_dir {
                    paths.push(entry.path().join("settings.json"));
                }
            }
        }
        for path in paths {
            if let Some(print) = fingerprint(&path) {
                files.insert(path, print);
            }
        }
    }
    files
}

/// Paths added, removed or changed between two snapshots, sorted.
fn diff_snapshots(before: &Snapshot, after: &Snapshot) -> Vec<PathBuf> {
    let mut changed: Vec<PathBuf> = before
        .iter()
        .filter(|(path, print)| after.get(*path) != Some(*print))
        .map(|(path, _)| path.clone())
        .chain(
            after
                .keys()
                .filter(|path| !before.contains_key(*path))
                .cloned(),
        )
        .collect();
    changed.sort();
    changed.dedup();
    changed
}

fn report(changed: &[PathBuf]) -> String {
    let mut text = String::from("real profile root changed during this test run:\n");
    for path in changed {
        text.push_str(&format!("  {}\n", path.display()));
    }
    text
}

extern "C" fn check_at_exit() {
    let result = std::panic::catch_unwind(|| {
        let baseline = BASELINE.get()?;
        let changed = diff_snapshots(&baseline.files, &snapshot(&baseline.roots));
        (!changed.is_empty()).then(|| report(&changed))
    });
    if let Ok(Some(text)) = result {
        use std::io::Write;
        let _ = std::io::stderr().write_all(text.as_bytes());
        // SAFETY: _exit ends the process at once; nothing runs after it.
        unsafe { libc::_exit(1) };
    }
}

/// Records the baseline and registers the exit check, once per process.
/// Does nothing in a quiet child.
fn arm() -> Option<&'static Baseline> {
    if std::env::var_os(QUIET_CHILD_ENV).is_some() {
        return None;
    }
    Some(BASELINE.get_or_init(|| {
        let roots = real_roots();
        let files = snapshot(&roots);
        // SAFETY: registering a plain `extern "C"` function has no
        // preconditions; it runs once, at normal process exit.
        unsafe { libc::atexit(check_at_exit) };
        Baseline { roots, files }
    }))
}

/// Arms the guard before `main`, so the baseline predates every test.
#[cfg(target_os = "linux")]
#[used]
#[unsafe(link_section = ".init_array")]
static ARM_BEFORE_MAIN: extern "C" fn() = {
    extern "C" fn arm_before_main() {
        let _ = std::panic::catch_unwind(arm);
    }
    arm_before_main
};

#[test]
fn real_profile_roots_unchanged_by_this_run() {
    let Some(baseline) = arm() else { return };
    let changed = diff_snapshots(&baseline.files, &snapshot(&baseline.roots));
    assert!(changed.is_empty(), "{}", report(&changed));
}

#[cfg(test)]
mod fixture {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap().join("profiles");
        for name in ["a", "b"] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("settings.json"), "{}").unwrap();
        }
        (d, root)
    }

    #[test]
    fn diff_ignores_unchanged_and_unlisted_files() {
        let (_d, root) = root();
        fs::write(root.join("a/notes.txt"), "1").unwrap();
        fs::write(root.join("a/.credentials.json"), "1").unwrap();
        fs::write(root.join("settings.json"), "{}").unwrap();
        let before = snapshot(std::slice::from_ref(&root));
        assert_eq!(before.len(), 2, "{before:?}");
        fs::write(root.join("a/notes.txt"), "2").unwrap();
        fs::write(root.join("a/.credentials.json"), "2").unwrap();
        fs::write(root.join("settings.json"), "{\"x\":1}").unwrap();
        let after = snapshot(std::slice::from_ref(&root));
        assert!(diff_snapshots(&before, &after).is_empty());
    }

    #[test]
    fn diff_detects_changed_settings() {
        let (_d, root) = root();
        let before = snapshot(std::slice::from_ref(&root));
        fs::write(root.join("b/settings.json"), "{\"hooks\":{}}").unwrap();
        let after = snapshot(std::slice::from_ref(&root));
        assert_eq!(
            diff_snapshots(&before, &after),
            vec![root.join("b/settings.json")]
        );
    }

    #[test]
    fn diff_detects_same_length_rewrite() {
        let (_d, root) = root();
        fs::write(root.join("a/settings.json"), "{\"k\":1}").unwrap();
        let before = snapshot(std::slice::from_ref(&root));
        fs::write(root.join("a/settings.json"), "{\"k\":2}").unwrap();
        let after = snapshot(std::slice::from_ref(&root));
        assert_eq!(
            diff_snapshots(&before, &after),
            vec![root.join("a/settings.json")]
        );
    }

    #[test]
    fn diff_detects_added_and_removed_settings() {
        let (_d, root) = root();
        let before = snapshot(std::slice::from_ref(&root));
        fs::remove_file(root.join("a/settings.json")).unwrap();
        fs::create_dir_all(root.join("c")).unwrap();
        fs::write(root.join("c/settings.json"), "{}").unwrap();
        let after = snapshot(std::slice::from_ref(&root));
        assert_eq!(
            diff_snapshots(&before, &after),
            vec![root.join("a/settings.json"), root.join("c/settings.json")]
        );
    }

    #[test]
    fn diff_detects_rotator_root_pointer_change() {
        let (_d, root) = root();
        fs::create_dir_all(root.join(".auto-switch.d/rotate")).unwrap();
        fs::write(root.join(".auto-switch.d/rotate/root"), "/a").unwrap();
        let before = snapshot(std::slice::from_ref(&root));
        fs::write(root.join(".auto-switch.d/rotate/root"), "/b").unwrap();
        let after = snapshot(std::slice::from_ref(&root));
        assert_eq!(
            diff_snapshots(&before, &after),
            vec![root.join(".auto-switch.d/rotate/root")]
        );
    }

    #[test]
    fn snapshot_does_not_follow_symlinks() {
        let (d, root) = root();
        let outside = d.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("settings.json"), "{}").unwrap();
        symlink(&outside, root.join("alias")).unwrap();
        fs::create_dir_all(root.join("linked")).unwrap();
        symlink(
            outside.join("settings.json"),
            root.join("linked/settings.json"),
        )
        .unwrap();
        let files = snapshot(std::slice::from_ref(&root));
        let keys: Vec<&PathBuf> = files.keys().collect();
        assert_eq!(
            keys,
            vec![&root.join("a/settings.json"), &root.join("b/settings.json")]
        );
    }

    #[test]
    fn roots_under_temp_bases_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().canonicalize().unwrap();
        let tmp = base.join("tmp");
        let real_home = base.join("real");
        let temp_home = tmp.join("home");
        for home in [&real_home, &temp_home] {
            fs::create_dir_all(home.join(".claude-profiles")).unwrap();
        }
        let roots = keep_real_roots(
            Some(tmp.join("home/.claude-profiles")),
            &[real_home.clone(), temp_home],
            std::slice::from_ref(&tmp),
        );
        assert_eq!(roots, vec![real_home.join(".claude-profiles")]);
    }

    #[test]
    fn missing_or_relative_roots_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap();
        fs::create_dir_all(home.join(".amplihack/claude-profiles")).unwrap();
        let roots = keep_real_roots(
            Some(PathBuf::from("relative/root")),
            std::slice::from_ref(&home),
            &[],
        );
        assert_eq!(roots, vec![home.join(".amplihack/claude-profiles")]);
    }

    #[test]
    fn report_names_each_changed_path() {
        let text = report(&[PathBuf::from("/r/a/settings.json")]);
        assert_eq!(
            text,
            "real profile root changed during this test run:\n  /r/a/settings.json\n"
        );
    }
}
