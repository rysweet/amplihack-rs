//! Issue #1699: fails the test binary if anything in this run changed a real
//! profile's `settings.json`.
//!
//! Compiled into two test binaries: `cli_integration` (through `tests/main.rs`)
//! and the `amplihack-cli` library tests (through
//! `src/commands/install/tests/mod.rs`), where the in-process install tests
//! run. It uses only `std`, `sha2`, `libc` and `amplihack_utils`, all normal
//! dependencies of the cli crate.
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
//!   the changed paths are printed and the process ends with `_exit(1)`. The
//!   one exception (see [`split_by_origin`]) is a `settings.json` whose hooks
//!   path lies under another process's temp directory, which is what an
//!   unfixed checkout's test run writes; such paths are printed as a warning.
//!   Every other change fails the run, as issue #1699 asks.
//!
//! It is read-only: files are opened with `O_NOFOLLOW`, must be regular files,
//! at most 1 MiB of each is hashed, and only lengths and hashes are kept.
//! Allow-list: `<root>/<entry>/settings.json` for real directories, and
//! `<root>/.auto-switch.d/rotate/root`. Credentials, locks and status files
//! the live rotator changes are never read. Where no real root exists (CI),
//! nothing is checked.
//!
//! Limits: another test binary's changes are not seen, a change made and
//! reverted within one run is not seen, and when another process rewrites a
//! file after this run did, only the later content is judged.

#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
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
    /// This process's temp directory, as written and canonical.
    own_temp: Vec<String>,
    /// Every temp directory base (see `temp_bases`), canonical.
    temp_bases: Vec<String>,
}

static BASELINE: OnceLock<Baseline> = OnceLock::new();

// `temp_bases` and `passwd_home` are the functions the production refusal in
// `src/commands/profile/settings_refresh.rs` calls, so the guard judges roots
// by the same rule production uses.
use amplihack_utils::secure_files::{passwd_home, temp_bases};

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

/// Length and first [`MAX_HASHED`] bytes of `path` if it is a regular file,
/// opened without following a symlink.
fn read_head(path: &Path) -> Option<(u64, Vec<u8>)> {
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
    Some((meta.len(), head))
}

/// Length and hash of `path` if it is a regular file.
fn fingerprint(path: &Path) -> Option<(u64, [u8; 32])> {
    let (len, head) = read_head(path)?;
    Some((len, Sha256::digest(&head).into()))
}

/// `temp_dir`, as given and canonical, without a trailing slash. Empty forms
/// (a temp directory of `/`) are dropped.
fn temp_prefixes(temp_dir: &Path) -> Vec<String> {
    let mut prefixes: Vec<String> = Vec::new();
    for form in [Some(temp_dir.to_path_buf()), temp_dir.canonicalize().ok()]
        .into_iter()
        .flatten()
    {
        let text = form.to_string_lossy().trim_end_matches('/').to_string();
        if !text.is_empty() && !prefixes.contains(&text) {
            prefixes.push(text);
        }
    }
    prefixes
}

/// The file name a hooks command ends in.
const HOOKS_BIN: &str = "amplihack-hooks";

/// Whether `text` names an `amplihack-hooks` path under one of `bases`.
/// Paths are read as the tokens between quotes, backslashes and whitespace,
/// which is how `settings.json` stores the hooks command.
fn names_temp_hooks_path(text: &str, bases: &[String]) -> bool {
    text.split(|c: char| c == '"' || c == '\\' || c == '\'' || c.is_whitespace())
        .filter(|token| token.ends_with(&format!("/{HOOKS_BIN}")))
        .any(|token| {
            bases
                .iter()
                .any(|base| token.starts_with(&format!("{base}/")))
        })
}

/// Whether `text` names anything under `own_temp`, as a whole path component.
fn names_own_temp(text: &str, own_temp: &[String]) -> bool {
    own_temp
        .iter()
        .any(|prefix| text.contains(&format!("{prefix}/")) || text.contains(&format!("{prefix}\"")))
}

/// `bases` as strings without a trailing slash; `/` is dropped.
fn base_strings(bases: &[PathBuf]) -> Vec<String> {
    bases
        .iter()
        .map(|base| base.to_string_lossy().trim_end_matches('/').to_string())
        .filter(|text| !text.is_empty())
        .collect()
}

/// Splits `changed` into paths that fail the run and paths put down to
/// another process.
///
/// Issue #1699 asks that a run changing any file under the real root fails,
/// so a change is put down to another process only on positive evidence of
/// the one pattern seen from an unfixed sibling checkout: a `settings.json`
/// that still exists, is readable, names an `amplihack-hooks` path under one
/// of `temp_bases`, and names nothing under `own_temp`. Everything else fails:
/// `.auto-switch.d/rotate/root`, a `settings.json` with no temp hooks path, a
/// removed or unreadable file, and any change when `own_temp` is empty. With
/// `TMPDIR` unset `own_temp` is `/tmp`, so every temp hooks path is this
/// run's and nothing is exempt.
fn split_by_origin(
    changed: &[PathBuf],
    own_temp: &[String],
    temp_bases: &[String],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    changed.iter().cloned().partition(|path| {
        if own_temp.is_empty() || path.file_name() != Some("settings.json".as_ref()) {
            return true;
        }
        let Some((_, head)) = read_head(path) else {
            return true;
        };
        let text = String::from_utf8_lossy(&head);
        names_own_temp(&text, own_temp) || !names_temp_hooks_path(&text, temp_bases)
    })
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
    let mut text = String::from(
        "issue_1699 real-root guard: real profile root changed during this test run:\n",
    );
    for path in changed {
        text.push_str(&format!("  {}\n", path.display()));
    }
    text.push_str(RERUN_HINT);
    text
}

/// The warning for changes put down to another process.
fn report_foreign(foreign: &[PathBuf], own_temp: &[String]) -> String {
    let mut text = format!(
        "issue_1699 real-root guard: warning: changed by another process (hooks under another temp directory, nothing under {} named), not failing:\n",
        own_temp.join(" or ")
    );
    for path in foreign {
        text.push_str(&format!("  {}\n", path.display()));
    }
    text
}

/// The text to print for `changed`, and whether the run fails.
fn verdict(
    changed: &[PathBuf],
    own_temp: &[String],
    temp_bases: &[String],
) -> Option<(String, bool)> {
    if changed.is_empty() {
        return None;
    }
    let (own, foreign) = split_by_origin(changed, own_temp, temp_bases);
    let mut text = String::new();
    if !foreign.is_empty() {
        text.push_str(&report_foreign(&foreign, own_temp));
    }
    if !own.is_empty() {
        text.push_str(&report(&own));
    }
    Some((text, !own.is_empty()))
}

/// Printed after the changed paths: the baseline is taken before the tests
/// start, so a legitimate change made by another process in that window also
/// fails the run.
const RERUN_HINT: &str = "issue_1699 real-root guard: another process can cause this \
    (`amplihack install`, `profile add`, rotator status-line setup or a rotator \
    restart during the run); re-run the tests before investigating\n";

extern "C" fn check_at_exit() {
    let result = std::panic::catch_unwind(|| {
        let baseline = BASELINE.get()?;
        let changed = diff_snapshots(&baseline.files, &snapshot(&baseline.roots));
        verdict(&changed, &baseline.own_temp, &baseline.temp_bases)
    });
    use std::io::Write;
    match result {
        Ok(Some((text, fails))) => {
            let _ = std::io::stderr().write_all(text.as_bytes());
            if fails {
                // SAFETY: _exit ends the process at once; nothing runs after it.
                unsafe { libc::_exit(1) };
            }
        }
        Ok(None) => {}
        Err(_) => {
            let _ = std::io::stderr().write_all(
                b"issue_1699 real-root guard: exit check panicked; profile root not verified\n",
            );
        }
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
        let own_temp = temp_prefixes(&std::env::temp_dir());
        let temp_bases = base_strings(&temp_bases());
        // SAFETY: registering a plain `extern "C"` function has no
        // preconditions; it runs once, at normal process exit.
        unsafe { libc::atexit(check_at_exit) };
        Baseline {
            roots,
            files,
            own_temp,
            temp_bases,
        }
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
    if let Some((text, fails)) = verdict(&changed, &baseline.own_temp, &baseline.temp_bases) {
        assert!(!fails, "{text}");
        eprint!("{text}");
    }
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

    fn hooks_at(dir: &Path) -> String {
        format!(
            "{{\"hooks\":{{\"Stop\":[{{\"hooks\":[{{\"command\":\"\\\"{}/.local/bin/amplihack-hooks\\\" stop\"}}]}}]}}}}",
            dir.display()
        )
    }

    /// A temp layout under a fresh directory: `own` is this run's temp
    /// directory, `other` another process's, both under the one base.
    struct Temps {
        base: PathBuf,
        own: PathBuf,
        other: PathBuf,
        own_temp: Vec<String>,
        bases: Vec<String>,
    }

    fn temps(d: &tempfile::TempDir) -> Temps {
        let base = d.path().canonicalize().unwrap().join("tmp");
        let own = base.join("own-tmp");
        let other = base.join("amplihack-workflow-2vXScw/tmp");
        fs::create_dir_all(&own).unwrap();
        Temps {
            own_temp: temp_prefixes(&own),
            bases: base_strings(std::slice::from_ref(&base)),
            base,
            own,
            other,
        }
    }

    #[test]
    fn change_naming_own_temp_fails_the_run() {
        let (d, root) = root();
        let t = temps(&d);
        let path = root.join("a/settings.json");
        fs::write(&path, hooks_at(&t.own.join(".tmpXYZ"))).unwrap();
        let (text, fails) = verdict(std::slice::from_ref(&path), &t.own_temp, &t.bases).unwrap();
        assert!(fails);
        assert!(text.contains(&format!("  {}\n", path.display())), "{text}");
        assert!(!text.contains("warning"), "{text}");
    }

    #[test]
    fn hooks_under_another_temp_tree_only_warn() {
        let (d, root) = root();
        let t = temps(&d);
        let path = root.join("a/settings.json");
        fs::write(&path, hooks_at(&t.other.join(".tmp1417Cj"))).unwrap();
        let (text, fails) = verdict(std::slice::from_ref(&path), &t.own_temp, &t.bases).unwrap();
        assert!(!fails, "{text}");
        assert!(text.contains("changed by another process"), "{text}");
        assert!(text.contains(&format!("  {}\n", path.display())), "{text}");
    }

    #[test]
    fn rotator_root_change_fails_with_a_distinct_own_temp() {
        let (d, root) = root();
        let t = temps(&d);
        let path = root.join(".auto-switch.d/rotate/root");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, root.display().to_string()).unwrap();
        let (text, fails) = verdict(std::slice::from_ref(&path), &t.own_temp, &t.bases).unwrap();
        assert!(fails, "{text}");
        // Even a temp hooks path in it does not exempt the rotator pointer.
        fs::write(&path, hooks_at(&t.other)).unwrap();
        let (_, fails) = verdict(std::slice::from_ref(&path), &t.own_temp, &t.bases).unwrap();
        assert!(fails);
    }

    #[test]
    fn non_temp_settings_change_fails_with_a_distinct_own_temp() {
        let (d, root) = root();
        let t = temps(&d);
        let path = root.join("a/settings.json");
        let home = d.path().canonicalize().unwrap().join("home/user");
        for content in [
            hooks_at(&home),
            "{\"statusLine\":{\"command\":\"rotator status\"}}".to_string(),
            // A temp path that is not a hooks binary is no evidence either.
            format!("{{\"x\":\"{}/notes\"}}", t.other.display()),
        ] {
            fs::write(&path, &content).unwrap();
            let (text, fails) =
                verdict(std::slice::from_ref(&path), &t.own_temp, &t.bases).unwrap();
            assert!(fails, "{content}: {text}");
            assert!(!text.contains("warning"), "{text}");
        }
    }

    #[test]
    fn a_file_naming_both_temp_trees_fails() {
        let (d, root) = root();
        let t = temps(&d);
        let path = root.join("a/settings.json");
        let both = format!("{}{}", hooks_at(&t.other), hooks_at(&t.own));
        fs::write(&path, both).unwrap();
        let (_, fails) = verdict(std::slice::from_ref(&path), &t.own_temp, &t.bases).unwrap();
        assert!(fails);
    }

    #[test]
    fn own_temp_equal_to_the_base_exempts_nothing() {
        // TMPDIR unset: this run's temp directory is the base itself.
        let (d, root) = root();
        let t = temps(&d);
        let own_temp = temp_prefixes(&t.base);
        let path = root.join("a/settings.json");
        fs::write(&path, hooks_at(&t.other)).unwrap();
        let (_, fails) = verdict(std::slice::from_ref(&path), &own_temp, &t.bases).unwrap();
        assert!(fails);
    }

    #[test]
    fn own_temp_must_match_a_whole_path_component() {
        let (d, root) = root();
        let t = temps(&d);
        let path = root.join("a/settings.json");
        let sibling = t.base.join(format!(
            "{}other/.tmpXYZ",
            t.own.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&path, hooks_at(&sibling)).unwrap();
        let (own_changes, foreign) =
            split_by_origin(std::slice::from_ref(&path), &t.own_temp, &t.bases);
        assert!(own_changes.is_empty());
        assert_eq!(foreign, vec![path]);
    }

    #[test]
    fn removed_file_or_empty_own_temp_fails_the_run() {
        let (d, root) = root();
        let t = temps(&d);
        let removed = root.join("a/settings.json");
        fs::remove_file(&removed).unwrap();
        let (_, fails) = verdict(std::slice::from_ref(&removed), &t.own_temp, &t.bases).unwrap();
        assert!(fails);
        let kept = root.join("b/settings.json");
        fs::write(&kept, hooks_at(&t.other)).unwrap();
        let (_, fails) = verdict(std::slice::from_ref(&kept), &[], &t.bases).unwrap();
        assert!(fails);
        assert!(verdict(&[], &t.own_temp, &t.bases).is_none());
    }

    #[test]
    fn temp_prefixes_drop_root_and_trailing_slash() {
        assert!(temp_prefixes(Path::new("/")).is_empty());
        let d = tempfile::tempdir().unwrap();
        let canonical = d.path().canonicalize().unwrap();
        let with_slash = PathBuf::from(format!("{}/", canonical.display()));
        assert_eq!(
            temp_prefixes(&with_slash),
            vec![canonical.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn report_names_each_changed_path() {
        let text = report(&[PathBuf::from("/r/a/settings.json")]);
        assert_eq!(
            text,
            format!(
                "issue_1699 real-root guard: real profile root changed during this \
                 test run:\n  /r/a/settings.json\n{RERUN_HINT}"
            )
        );
        assert!(RERUN_HINT.starts_with("issue_1699 real-root guard: "));
        assert!(RERUN_HINT.contains("re-run the tests before investigating"));
    }
}
