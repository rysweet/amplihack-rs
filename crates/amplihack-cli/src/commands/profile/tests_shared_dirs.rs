//! Contract for the shared session store (issue #1634): every profile's
//! `projects/`, `file-history/` and `session-env/` is a link to
//! `~/.claude/<name>`, logins stay per profile. See
//! docs/reference/profile-command.md#shared-session-directories; the message
//! wording there is the spec these tests match.
//!
//! Every test builds its own `home/.claude` and `home/.claude-profiles` in a
//! temp dir and passes them in; none reads the real HOME. The profile root is
//! deliberately named `.claude-profiles`, a string-prefix lookalike of the
//! canonical root, so every test also proves that "inside the canonical root"
//! is compared by path components.
//!
//! API used (all `pub(super)` in `profile::shared_dirs`):
//! - `SHARED_SESSION_DIRS`, `Mode { Migrate, LinkOnly(Deferred) }` (Copy),
//!   `Deferred { LiveSession, LivenessUnknown, Raced }` (Copy, PartialEq).
//! - `ensure_shared_session_dirs(profile_dir, profile, canonical_root, mode)`
//!   -> `Report { linked, already_ok, migrated, deferred, conflicts,
//!   warnings, unsupported }`, `Report::is_noop()`,
//!   `Report::render(&ProfileName, Option<&Path>) -> Vec<String>` (the
//!   stderr lines, prefix included, paths shortened against the home given).
//! - `Migrated { name, moved, identical, conflicts, backup }`,
//!   `Conflict { name, path /* relative to the shared dir */, saved_as }`.
//! - `mode_from_liveness(&Liveness, &ProfileName) -> Mode`, `ensure_for`.
//! - `test_hook::{set_force_exdev, set_fail_link_rename}` (thread-local).
//! - `store::exchange`, `store::test_hook::set_no_exchange` (thread-local).

use super::name::ProfileName;
use super::shared_dirs::{
    self, Deferred, Mode, Report, SHARED_SESSION_DIRS, ensure_shared_session_dirs,
};
use super::store::{self, ProfileStore};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode_of(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

struct Fx {
    _tmp: tempfile::TempDir,
    outside: PathBuf,
    home: PathBuf,
    canon: PathBuf,
    root: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        chmod(tmp.path(), 0o700);
        let base = tmp.path().canonicalize().unwrap();
        let home = base.join("home");
        let canon = home.join(".claude");
        let root = home.join(".claude-profiles");
        let outside = base.join("outside");
        for d in [&home, &canon, &root, &outside] {
            fs::create_dir_all(d).unwrap();
            chmod(d, 0o700);
        }
        if let Some(real) = std::env::var_os("HOME") {
            assert_ne!(
                canon,
                PathBuf::from(real).join(".claude"),
                "never the real ~/.claude"
            );
        }
        Self {
            _tmp: tmp,
            outside,
            home,
            canon,
            root,
        }
    }

    fn dir(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }

    /// An empty profile directory, as `add` publishes it (0700).
    fn profile(&self, p: &str) -> PathBuf {
        let d = self.dir(p);
        fs::create_dir_all(&d).unwrap();
        chmod(&d, 0o700);
        d
    }

    /// A pre-#1634 profile: a real `<dir>/<rel>` file with `body`.
    fn legacy(&self, p: &str, rel: &str, body: &[u8]) -> PathBuf {
        let path = self.profile(p).join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn shared(&self, rel: &str, body: &[u8]) -> PathBuf {
        let path = self.canon.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    fn ensure(&self, p: &str, mode: Mode) -> Report {
        ensure_shared_session_dirs(&self.dir(p), &n(p), &self.canon, move || mode)
    }

    /// For runs that must not need liveness: calling `mode` fails the test.
    fn ensure_no_mode(&self, p: &str) -> Report {
        ensure_shared_session_dirs(&self.dir(p), &n(p), &self.canon, || {
            panic!("mode must not be called when no name is a real directory")
        })
    }

    fn lines(&self, p: &str, r: &Report) -> Vec<String> {
        r.render(&n(p), Some(&self.home))
    }

    fn assert_linked(&self, p: &str, name: &str) {
        let link = self.dir(p).join(name);
        assert!(is_link(&link), "{} is not a symlink", link.display());
        let target = fs::read_link(&link).unwrap();
        assert!(target.is_absolute(), "relative link target {target:?}");
        assert_eq!(target, self.canon.join(name));
        assert_eq!(link.canonicalize().unwrap(), self.canon.join(name));
    }

    /// Every `<name>.pre-share-*` backup inside the profile.
    fn backups(&self, p: &str, name: &str) -> Vec<PathBuf> {
        let prefix = format!("{name}.pre-share-");
        let mut v: Vec<PathBuf> = fs::read_dir(self.dir(p))
            .unwrap()
            .map(|e| e.unwrap())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
            .map(|e| e.path())
            .collect();
        v.sort();
        v
    }

    fn only_backup(&self, p: &str, name: &str) -> PathBuf {
        let b = self.backups(p, name);
        assert_eq!(b.len(), 1, "backups: {b:?}");
        b.into_iter().next().unwrap()
    }
}

/// `<name>.pre-share-YYYYMMDDTHHMMSSZ` with an optional `-N`.
fn assert_backup_name(path: &Path, name: &str) {
    let file = path.file_name().unwrap().to_str().unwrap();
    let re = regex::Regex::new(&format!(
        r"^{}\.pre-share-\d{{8}}T\d{{6}}Z(-\d+)?$",
        regex::escape(name)
    ))
    .unwrap();
    assert!(re.is_match(file), "bad backup name {file:?}");
}

/// Path -> (type, mode, size, inode, mtime, link target, contents), without
/// following symlinks. Equal snapshots mean nothing on disk changed.
fn snapshot(base: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(base: &Path, p: &Path, out: &mut BTreeMap<PathBuf, String>) {
        let m = fs::symlink_metadata(p).unwrap();
        let rel = p.strip_prefix(base).unwrap().to_path_buf();
        let mtime = m.modified().unwrap();
        let ft = m.file_type();
        let detail = if ft.is_symlink() {
            format!("link {:?}", fs::read_link(p).unwrap())
        } else if ft.is_file() {
            format!("file {:?}", fs::read(p).unwrap())
        } else if ft.is_dir() {
            "dir".to_string()
        } else {
            "other".to_string()
        };
        out.insert(
            rel,
            format!("{detail} {:o} {} {} {mtime:?}", m.mode(), m.len(), m.ino()),
        );
        if ft.is_dir() {
            for e in fs::read_dir(p).unwrap() {
                walk(base, &e.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(base, base, &mut out);
    out
}

/// No credential-named file anywhere below `dir` (links not followed).
fn assert_no_credentials_under(dir: &Path) {
    fn walk(p: &Path) {
        let Ok(rd) = fs::read_dir(p) else { return };
        for e in rd {
            let e = e.unwrap();
            let name = e.file_name();
            let bytes = name.as_bytes();
            assert!(
                !bytes.starts_with(b".credentials") && bytes != b".claude.json",
                "credential file in the shared store: {}",
                e.path().display()
            );
            if e.file_type().unwrap().is_dir() {
                walk(&e.path());
            }
        }
    }
    for name in SHARED_SESSION_DIRS {
        walk(&dir.join(name));
    }
}

/// Resets a thread-local hook when the test ends, pass or fail.
struct Reset(fn());
impl Drop for Reset {
    fn drop(&mut self) {
        (self.0)();
    }
}

// ---------------------------------------------------------------- links

#[test]
fn shared_names_are_the_three_session_dirs() {
    assert_eq!(
        SHARED_SESSION_DIRS,
        ["projects", "file-history", "session-env"]
    );
}

#[test]
fn fresh_profile_gets_three_absolute_links_without_asking_for_liveness() {
    let f = Fx::new();
    f.profile("A");
    let r = f.ensure_no_mode("A");
    assert_eq!(r.linked, SHARED_SESSION_DIRS.to_vec());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    for name in SHARED_SESSION_DIRS {
        f.assert_linked("A", name);
    }
    assert_eq!(
        f.lines("A", &r),
        vec!["amplihack: profile A: linked projects, file-history, session-env".to_string()]
    );
}

#[test]
fn missing_canonical_dirs_are_created_0700_and_existing_modes_are_kept() {
    let f = Fx::new();
    fs::create_dir(f.canon.join("projects")).unwrap();
    chmod(&f.canon.join("projects"), 0o750);
    f.profile("A");
    f.ensure_no_mode("A");
    assert_eq!(
        mode_of(&f.canon.join("projects")),
        0o750,
        "existing mode changed"
    );
    assert_eq!(mode_of(&f.canon.join("file-history")), 0o700);
    assert_eq!(mode_of(&f.canon.join("session-env")), 0o700);
}

#[test]
fn second_run_changes_nothing_and_does_not_ask_for_liveness() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"one");
    f.ensure("A", Mode::Migrate);
    let before = snapshot(&f.home);

    let r = f.ensure_no_mode("A");

    assert_eq!(snapshot(&f.home), before);
    assert!(r.is_noop(), "second run not a no-op");
    assert_eq!(r.already_ok, SHARED_SESSION_DIRS.to_vec());
    assert!(
        f.lines("A", &r).is_empty(),
        "a no-op printed {:?}",
        f.lines("A", &r)
    );
}

#[test]
fn a_session_written_under_one_profile_is_visible_under_another() {
    let f = Fx::new();
    f.profile("A");
    f.profile("B");
    f.ensure_no_mode("A");
    f.ensure_no_mode("B");
    let via_a = f.dir("A").join("projects/-home-u-repo");
    fs::create_dir_all(&via_a).unwrap();
    fs::write(via_a.join("s1.jsonl"), b"{\"session\":1}\n").unwrap();
    fs::create_dir_all(f.dir("A").join("file-history/s1")).unwrap();
    fs::create_dir_all(f.dir("A").join("session-env/s1")).unwrap();

    assert_eq!(
        fs::read(f.dir("B").join("projects/-home-u-repo/s1.jsonl")).unwrap(),
        b"{\"session\":1}\n"
    );
    assert!(f.dir("B").join("file-history/s1").is_dir());
    assert!(f.dir("B").join("session-env/s1").is_dir());
    assert!(f.canon.join("projects/-home-u-repo/s1.jsonl").is_file());
}

// ------------------------------------------------------------ migration

#[test]
fn migration_moves_sessions_keeps_mtime_and_leaves_a_backup() {
    let f = Fx::new();
    let s1 = f.legacy("A", "projects/-w/s1.jsonl", b"session one");
    f.legacy("A", "projects/-w/s2.jsonl", b"session two");
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    fs::File::options()
        .write(true)
        .open(&s1)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let ino = fs::metadata(&s1).unwrap().ino();

    let r = f.ensure("A", Mode::Migrate);

    f.assert_linked("A", "projects");
    let moved = f.canon.join("projects/-w/s1.jsonl");
    assert_eq!(fs::read(&moved).unwrap(), b"session one");
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s2.jsonl")).unwrap(),
        b"session two"
    );
    assert_eq!(fs::metadata(&moved).unwrap().modified().unwrap(), old);
    assert_eq!(
        fs::metadata(&moved).unwrap().ino(),
        ino,
        "same filesystem: a rename"
    );
    assert_eq!(r.migrated.len(), 1);
    let m = &r.migrated[0];
    assert_eq!(
        (m.name, m.moved, m.identical, m.conflicts),
        ("projects", 2, 0, 0)
    );
    let backup = f.only_backup("A", "projects");
    assert_backup_name(&backup, "projects");
    assert_eq!(m.backup, backup);
    assert!(is_real_dir(&backup));
    assert!(
        !backup.join("-w/s1.jsonl").exists(),
        "moved file also in backup"
    );
    let lines = f.lines("A", &r);
    assert!(
        lines.contains(
            &"amplihack: profile A: projects: moved 2 files into ~/.claude/projects, 0 identical, 0 conflict"
                .to_string()
        ),
        "{lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("amplihack: profile A: projects: old directory kept as ")),
        "{lines:#?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("linked projects")),
        "a migrated name is not reported as linked: {lines:#?}"
    );
}

#[test]
fn all_three_names_are_migrated() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"p");
    f.legacy("A", "file-history/s1/v1", b"fh");
    f.legacy("A", "session-env/s1/env", b"se");
    let r = f.ensure("A", Mode::Migrate);
    for name in SHARED_SESSION_DIRS {
        f.assert_linked("A", name);
        assert_eq!(f.backups("A", name).len(), 1, "{name}");
    }
    assert_eq!(r.migrated.len(), 3);
    assert_eq!(fs::read(f.canon.join("file-history/s1/v1")).unwrap(), b"fh");
    assert_eq!(fs::read(f.canon.join("session-env/s1/env")).unwrap(), b"se");
}

#[test]
fn new_directories_in_the_shared_store_are_0700() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/sub/deep/x.json", b"x");
    chmod(&f.dir("A").join("projects/-w"), 0o755);
    f.ensure("A", Mode::Migrate);
    assert_eq!(mode_of(&f.canon.join("projects/-w")), 0o700);
    assert_eq!(mode_of(&f.canon.join("projects/-w/sub/deep")), 0o700);
}

#[test]
fn an_identical_file_is_left_in_the_backup_and_the_shared_copy_untouched() {
    let f = Fx::new();
    let shared = f.shared("projects/-w/s1.jsonl", b"same");
    let ino = fs::metadata(&shared).unwrap().ino();
    f.legacy("A", "projects/-w/s1.jsonl", b"same");

    let r = f.ensure("A", Mode::Migrate);

    assert_eq!(fs::metadata(&shared).unwrap().ino(), ino);
    let m = &r.migrated[0];
    assert_eq!((m.moved, m.identical, m.conflicts), (0, 1, 0));
    let backup = f.only_backup("A", "projects");
    assert_eq!(fs::read(backup.join("-w/s1.jsonl")).unwrap(), b"same");
    assert!(r.conflicts.is_empty());
}

#[test]
fn a_conflict_keeps_the_shared_copy_even_when_the_profile_copy_is_larger_and_newer() {
    let f = Fx::new();
    let shared = f.shared("projects/-w/s1.jsonl", b"short");
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    fs::File::options()
        .write(true)
        .open(&shared)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let ino = fs::metadata(&shared).unwrap().ino();
    f.legacy(
        "A",
        "projects/-w/s1.jsonl",
        b"a much longer and newer profile copy",
    );

    let r = f.ensure("A", Mode::Migrate);

    assert_eq!(
        fs::read(&shared).unwrap(),
        b"short",
        "canonical overwritten"
    );
    assert_eq!(
        fs::metadata(&shared).unwrap().ino(),
        ino,
        "canonical renamed"
    );
    let saved = f.canon.join("projects/-w/s1.jsonl.conflict-A");
    assert_eq!(
        fs::read(&saved).unwrap(),
        b"a much longer and newer profile copy"
    );
    assert_eq!(r.conflicts.len(), 1);
    assert_eq!(r.conflicts[0].name, "projects");
    assert_eq!(r.conflicts[0].path, Path::new("-w/s1.jsonl"));
    assert_eq!(r.conflicts[0].saved_as, saved);
    assert_eq!(r.migrated[0].conflicts, 1);
    let lines = f.lines("A", &r);
    assert!(
        lines.iter().any(
            |l| l.starts_with("amplihack: profile A: projects: conflict: ")
                && l.contains("kept the shared copy; this profile's copy saved as ")
                && l.ends_with("s1.jsonl.conflict-A")
        ),
        "{lines:#?}"
    );
    assert!(
        lines.contains(
            &"amplihack: profile A: projects: moved 0 files into ~/.claude/projects, 0 identical, 1 conflict"
                .to_string()
        ),
        "{lines:#?}"
    );
}

#[test]
fn conflict_names_reuse_identical_bytes_and_otherwise_count_up() {
    let f = Fx::new();
    f.shared("projects/-w/s1.jsonl", b"canonical");
    // An earlier run already saved exactly these bytes.
    f.shared("projects/-w/s1.jsonl.conflict-A", b"first loser");
    f.legacy("A", "projects/-w/s1.jsonl", b"first loser");
    let r = f.ensure("A", Mode::Migrate);
    assert!(!f.canon.join("projects/-w/s1.jsonl.conflict-A-2").exists());
    assert_eq!(r.conflicts.len(), 1, "a reused slot is still reported");

    // A later, different copy takes the next free suffix.
    fs::remove_file(f.dir("A").join("projects")).unwrap();
    f.legacy("A", "projects/-w/s1.jsonl", b"second loser");
    f.ensure("A", Mode::Migrate);
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl.conflict-A")).unwrap(),
        b"first loser"
    );
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl.conflict-A-2")).unwrap(),
        b"second loser"
    );
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl")).unwrap(),
        b"canonical"
    );
}

#[test]
fn conflicts_from_two_profiles_get_their_own_names() {
    let f = Fx::new();
    f.shared("projects/-w/s1.jsonl", b"canonical");
    f.legacy("A", "projects/-w/s1.jsonl", b"from A");
    f.legacy("B", "projects/-w/s1.jsonl", b"from B");
    f.ensure("A", Mode::Migrate);
    f.ensure("B", Mode::Migrate);
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl.conflict-A")).unwrap(),
        b"from A"
    );
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl.conflict-B")).unwrap(),
        b"from B"
    );
}

#[test]
fn a_real_dir_next_to_an_old_backup_is_migrated_again_and_both_backups_kept() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"one");
    f.ensure("A", Mode::Migrate);
    // Claude (or the user) replaced the link with a real directory.
    fs::remove_file(f.dir("A").join("projects")).unwrap();
    f.legacy("A", "projects/-w/s3.jsonl", b"three");

    let r = f.ensure("A", Mode::Migrate);

    f.assert_linked("A", "projects");
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s3.jsonl")).unwrap(),
        b"three"
    );
    let backups = f.backups("A", "projects");
    assert_eq!(backups.len(), 2, "{backups:?}");
    for b in &backups {
        assert_backup_name(b, "projects");
    }
    assert_eq!(r.migrated[0].moved, 1);
}

// ------------------------------------------------------------- liveness

#[test]
fn a_live_profile_defers_its_real_dir_but_links_missing_names() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"open session");
    let before = snapshot(&f.dir("A").join("projects"));

    let r = f.ensure("A", Mode::LinkOnly(Deferred::LiveSession));

    assert_eq!(
        snapshot(&f.dir("A").join("projects")),
        before,
        "live dir touched"
    );
    assert!(!f.canon.join("projects/-w").exists());
    assert!(f.backups("A", "projects").is_empty());
    assert_eq!(r.deferred, vec![("projects", Deferred::LiveSession)]);
    assert_eq!(r.linked, vec!["file-history", "session-env"]);
    f.assert_linked("A", "file-history");
    f.assert_linked("A", "session-env");
    let lines = f.lines("A", &r);
    assert!(
        lines.contains(
            &"amplihack: profile A: projects: migration deferred: live session".to_string()
        ),
        "{lines:#?}"
    );
    assert!(
        lines.contains(&"amplihack: profile A: linked file-history, session-env".to_string()),
        "{lines:#?}"
    );
}

#[test]
fn unknown_liveness_points_to_the_manual_merge() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    let r = f.ensure("A", Mode::LinkOnly(Deferred::LivenessUnknown));
    assert!(is_real_dir(&f.dir("A").join("projects")));
    assert_eq!(r.deferred, vec![("projects", Deferred::LivenessUnknown)]);
    assert!(
        f.lines("A", &r).contains(
            &"amplihack: profile A: projects: not merged: running sessions can't be checked on this system; merge by hand, see docs/howto/claude-profiles.md#merge-by-hand-on-macos"
                .to_string()
        ),
        "{:#?}",
        f.lines("A", &r)
    );
}

#[test]
fn mode_is_asked_for_once_and_only_when_a_name_is_a_real_dir() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    f.legacy("A", "session-env/s1/e", b"y");
    let calls = Cell::new(0);
    let r = ensure_shared_session_dirs(&f.dir("A"), &n("A"), &f.canon, || {
        calls.set(calls.get() + 1);
        Mode::Migrate
    });
    assert_eq!(calls.get(), 1);
    assert_eq!(r.migrated.len(), 2);
}

#[test]
fn every_name_lands_in_exactly_one_outcome() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    symlink(&f.outside, f.dir("A").join("file-history")).unwrap();
    let r = f.ensure("A", Mode::LinkOnly(Deferred::LiveSession));
    assert_eq!(r.deferred, vec![("projects", Deferred::LiveSession)]);
    assert_eq!(r.linked, vec!["session-env"]);
    assert!(r.already_ok.is_empty() && r.migrated.is_empty());
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("file-history"));
}

#[cfg(target_os = "linux")]
#[test]
fn liveness_maps_to_mode_and_any_doubt_never_migrates() {
    use super::auto_switch::kit::Liveness;
    use std::collections::BTreeSet;
    let a = n("A");
    let known = |names: &[&str]| {
        Liveness::Known(names.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>())
    };
    assert_eq!(
        shared_dirs::mode_from_liveness(&known(&[]), &a),
        Mode::Migrate
    );
    assert_eq!(
        shared_dirs::mode_from_liveness(&known(&["B"]), &a),
        Mode::Migrate
    );
    assert_eq!(
        shared_dirs::mode_from_liveness(&known(&["A"]), &a),
        Mode::LinkOnly(Deferred::LiveSession)
    );
    assert_eq!(
        shared_dirs::mode_from_liveness(&Liveness::Everything, &a),
        Mode::LinkOnly(Deferred::LiveSession)
    );
    // On Linux `Unsupported` means the scan failed: retried next call.
    assert_eq!(
        shared_dirs::mode_from_liveness(&Liveness::Unsupported, &a),
        Mode::LinkOnly(Deferred::LiveSession)
    );
}

#[cfg(all(unix, not(target_os = "linux")))]
#[test]
fn unsupported_liveness_off_linux_is_liveness_unknown() {
    use super::auto_switch::kit::Liveness;
    assert_eq!(
        shared_dirs::mode_from_liveness(&Liveness::Unsupported, &n("A")),
        Mode::LinkOnly(Deferred::LivenessUnknown)
    );
}

/// AC3 end to end through the real scanner: a fake `/proc` holding a Claude
/// process with `CLAUDE_CONFIG_DIR=<root>/A` defers A's migration.
#[test]
fn a_running_claude_in_a_fake_proc_defers_migration() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"open");
    f.profile("B");
    let proc_root = f.outside.join("proc");
    fs::create_dir_all(proc_root.join("self")).unwrap();
    fs::write(proc_root.join("self/environ"), b"HOME=/h\0").unwrap();
    let pid = proc_root.join("777");
    fs::create_dir_all(&pid).unwrap();
    fs::write(
        pid.join("status"),
        "Name:\tclaude\nState:\tS\nUid:\t1000\t1000\t1000\t1000\n",
    )
    .unwrap();
    let mut environ = format!("CLAUDE_CONFIG_DIR={}", f.dir("A").display()).into_bytes();
    environ.push(0);
    fs::write(pid.join("environ"), environ).unwrap();
    fs::write(pid.join("cmdline"), b"claude\0").unwrap();
    symlink(&f.home, pid.join("cwd")).unwrap();

    let live = super::auto_switch::scan(&proc_root, &f.root, 4242, 1000);
    let mode_a = shared_dirs::mode_from_liveness(&live, &n("A"));
    assert_eq!(mode_a, Mode::LinkOnly(Deferred::LiveSession));
    assert_eq!(
        shared_dirs::mode_from_liveness(&live, &n("B")),
        Mode::Migrate
    );

    let r = f.ensure("A", mode_a);
    assert!(is_real_dir(&f.dir("A").join("projects")));
    assert_eq!(r.deferred, vec![("projects", Deferred::LiveSession)]);
}

// ------------------------------------------------- recovery and fallback

#[test]
fn a_stale_temp_link_is_replaced_and_a_reserved_temp_file_is_removed() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    symlink(
        "/nonexistent-target",
        f.dir("A").join(".projects.share-tmp"),
    )
    .unwrap();
    let leftover = f.shared("projects/-w/.amplihack-share-tmp-1-0", b"half copy");

    f.ensure("A", Mode::Migrate);

    f.assert_linked("A", "projects");
    assert!(fs::symlink_metadata(f.dir("A").join(".projects.share-tmp")).is_err());
    assert!(
        fs::symlink_metadata(&leftover).is_err(),
        "reserved temp file kept"
    );
}

#[test]
fn only_reserved_temp_names_are_ever_deleted() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    // Reserved name in the source: never moved, ends up in the backup.
    f.legacy("A", "projects/-w/.amplihack-share-tmp-5-5", b"src tmp");
    let lookalike = f.shared("projects/-w/.amplihack-share-tmpX", b"keep me");
    let as_dir = f.canon.join("projects/-w/.amplihack-share-tmp-2-0");
    fs::create_dir_all(&as_dir).unwrap();
    let as_link = f.canon.join("projects/-w/.amplihack-share-tmp-3-0");
    symlink(&lookalike, &as_link).unwrap();
    // A stale temp *directory* at the link-swap name is not a symlink: kept.
    let stale_dir = f.profile("B").join(".projects.share-tmp");
    fs::create_dir(&stale_dir).unwrap();
    f.legacy("B", "projects/-w/s2.jsonl", b"y");

    f.ensure("A", Mode::Migrate);
    f.ensure("B", Mode::Migrate);

    assert_eq!(fs::read(&lookalike).unwrap(), b"keep me");
    assert!(
        as_dir.is_dir(),
        "a directory with a reserved name was deleted"
    );
    assert!(
        is_link(&as_link),
        "a symlink with a reserved name was deleted"
    );
    assert!(
        stale_dir.is_dir(),
        "a real dir at .<name>.share-tmp was deleted"
    );
    assert!(
        !f.canon
            .join("projects/-w/.amplihack-share-tmp-5-5")
            .exists()
    );
    let backup = f.only_backup("A", "projects");
    assert_eq!(
        fs::read(backup.join("-w/.amplihack-share-tmp-5-5")).unwrap(),
        b"src tmp"
    );
}

#[test]
fn exchange_swaps_two_names_or_reports_it_cannot() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    let b = d.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::write(a.join("marker"), b"in a").unwrap();
    symlink("/target-of-b", &b).unwrap();

    #[cfg(target_os = "linux")]
    {
        assert!(
            store::exchange(&a, &b).unwrap(),
            "tmpfs/ext4 support RENAME_EXCHANGE"
        );
        assert!(is_link(&a) && fs::read_link(&a).unwrap() == Path::new("/target-of-b"));
        assert_eq!(fs::read(b.join("marker")).unwrap(), b"in a");
        fs::remove_file(&a).unwrap();
        fs::rename(&b, &a).unwrap();
        symlink("/target-of-b", &b).unwrap();
    }

    store::test_hook::set_no_exchange(true);
    let _reset = Reset(|| store::test_hook::set_no_exchange(false));
    assert!(!store::exchange(&a, &b).unwrap(), "forced: no exchange");
    assert!(
        is_real_dir(&a) && is_link(&b),
        "a refused exchange moved something"
    );
}

#[test]
fn without_exchange_the_two_rename_fallback_gives_the_same_result() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    store::test_hook::set_no_exchange(true);
    let _reset = Reset(|| store::test_hook::set_no_exchange(false));

    let r = f.ensure("A", Mode::Migrate);

    f.assert_linked("A", "projects");
    assert_backup_name(&f.only_backup("A", "projects"), "projects");
    assert!(fs::symlink_metadata(f.dir("A").join(".projects.share-tmp")).is_err());
    assert_eq!(r.migrated[0].moved, 1);
}

#[test]
fn a_failed_link_swap_is_rolled_back_and_nothing_is_lost() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    f.legacy("A", "projects/-w/.credentials.json", b"stays");
    store::test_hook::set_no_exchange(true);
    shared_dirs::test_hook::set_fail_link_rename(true);
    let _r1 = Reset(|| store::test_hook::set_no_exchange(false));
    let _r2 = Reset(|| shared_dirs::test_hook::set_fail_link_rename(false));

    let r = f.ensure("A", Mode::Migrate);

    assert!(
        is_real_dir(&f.dir("A").join("projects")),
        "old directory not restored"
    );
    assert_eq!(
        fs::read(f.dir("A").join("projects/-w/.credentials.json")).unwrap(),
        b"stays"
    );
    assert!(
        f.backups("A", "projects").is_empty(),
        "rolled back: no backup name"
    );
    // Merged files are already safe in the shared store.
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl")).unwrap(),
        b"x"
    );
    let lines = f.lines("A", &r);
    assert!(
        lines.iter().any(|l| l
            .starts_with("amplihack: profile A: projects: could not swap in the link (")
            && l.ends_with(
                "); ~/.claude-profiles/A/projects was not replaced; files already merged are in ~/.claude/projects"
            )),
        "{lines:#?}"
    );
}

#[test]
fn a_cross_filesystem_move_copies_masks_the_mode_keeps_mtime_then_unlinks() {
    let f = Fx::new();
    let src = f.legacy("A", "projects/-w/s1.jsonl", b"copied across devices");
    chmod(&src, 0o664);
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_650_000_000);
    fs::File::options()
        .write(true)
        .open(&src)
        .unwrap()
        .set_modified(old)
        .unwrap();
    shared_dirs::test_hook::set_force_exdev(true);
    let _reset = Reset(|| shared_dirs::test_hook::set_force_exdev(false));

    let r = f.ensure("A", Mode::Migrate);

    let dst = f.canon.join("projects/-w/s1.jsonl");
    assert_eq!(fs::read(&dst).unwrap(), b"copied across devices");
    assert_eq!(mode_of(&dst), 0o600, "0o664 & 0o700");
    assert_eq!(fs::metadata(&dst).unwrap().modified().unwrap(), old);
    let backup = f.only_backup("A", "projects");
    assert!(
        !backup.join("-w/s1.jsonl").exists(),
        "source not unlinked after copy"
    );
    let leftovers: Vec<_> = fs::read_dir(f.canon.join("projects/-w"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.as_bytes().starts_with(b".amplihack-share-tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    assert_eq!(r.migrated[0].moved, 1);
}

#[test]
fn a_cross_filesystem_merge_checks_the_destination_before_copying() {
    let f = Fx::new();
    let same = f.shared("projects/-w/same.jsonl", b"same");
    let same_ino = fs::metadata(&same).unwrap().ino();
    let kept = f.shared("projects/-w/s1.jsonl", b"canonical");
    let kept_ino = fs::metadata(&kept).unwrap().ino();
    f.legacy("A", "projects/-w/same.jsonl", b"same");
    f.legacy("A", "projects/-w/s1.jsonl", b"profile copy");
    shared_dirs::test_hook::set_force_exdev(true);
    let _reset = Reset(|| shared_dirs::test_hook::set_force_exdev(false));

    let r = f.ensure("A", Mode::Migrate);

    let m = &r.migrated[0];
    assert_eq!((m.moved, m.identical, m.conflicts), (0, 1, 1));
    assert_eq!(fs::metadata(&same).unwrap().ino(), same_ino);
    assert_eq!(fs::metadata(&kept).unwrap().ino(), kept_ino);
    assert_eq!(fs::read(&kept).unwrap(), b"canonical");
    let mut names: Vec<_> = fs::read_dir(f.canon.join("projects/-w"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["s1.jsonl", "s1.jsonl.conflict-A", "same.jsonl"]);
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl.conflict-A")).unwrap(),
        b"profile copy"
    );
    let backup = f.only_backup("A", "projects");
    assert_eq!(fs::read(backup.join("-w/same.jsonl")).unwrap(), b"same");
    assert!(!backup.join("-w/s1.jsonl").exists(), "conflict source kept");
}

// ------------------------------------------------------------- security

#[test]
fn credential_named_files_at_any_depth_stay_in_the_backup() {
    let f = Fx::new();
    let top_login = f.legacy("A", ".credentials.json", b"TOP-LOGIN");
    let top_ino = fs::metadata(&top_login).unwrap().ino();
    f.legacy("A", ".claude.json", b"{\"oauthAccount\":{}}");
    f.legacy("A", "projects/-w/s1.jsonl", b"ok");
    f.legacy("A", "projects/.credentials.json", b"SECRET-1");
    f.legacy("A", "projects/-w/.credentials.json.bak", b"SECRET-2");
    f.legacy("A", "projects/-w/deep/.claude.json", b"SECRET-3");
    let raw = OsStr::from_bytes(b".credentials\xff\xfe");
    let non_utf8 = f.dir("A").join("projects/-w").join(raw);
    fs::write(&non_utf8, b"SECRET-4").unwrap();

    let r = f.ensure("A", Mode::Migrate);

    assert_no_credentials_under(&f.canon);
    assert!(f.canon.join("projects/-w/s1.jsonl").is_file());
    let backup = f.only_backup("A", "projects");
    assert_eq!(
        fs::read(backup.join(".credentials.json")).unwrap(),
        b"SECRET-1"
    );
    assert_eq!(
        fs::read(backup.join("-w/.credentials.json.bak")).unwrap(),
        b"SECRET-2"
    );
    assert_eq!(
        fs::read(backup.join("-w/deep/.claude.json")).unwrap(),
        b"SECRET-3"
    );
    assert_eq!(fs::read(backup.join("-w").join(raw)).unwrap(), b"SECRET-4");
    // Top-level logins are never touched.
    assert_eq!(fs::metadata(&top_login).unwrap().ino(), top_ino);
    assert_eq!(fs::read(&top_login).unwrap(), b"TOP-LOGIN");
    assert!(f.dir("A").join(".claude.json").is_file());
    assert!(!f.canon.join(".credentials.json").exists());
    let lines = f.lines("A", &r);
    let skipped: Vec<_> = lines
        .iter()
        .filter(|l| l.starts_with("amplihack: profile A: projects: skipped credential file "))
        .collect();
    assert_eq!(skipped.len(), 4, "{lines:#?}");
    for l in lines {
        assert!(!l.contains("SECRET"), "file contents printed: {l}");
    }
}

#[test]
fn nested_symlinks_and_special_files_are_never_followed_or_moved() {
    let f = Fx::new();
    let secret = f.outside.join("secret.txt");
    fs::write(&secret, b"outside").unwrap();
    let outside_dir = f.outside.join("tree");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(outside_dir.join("leak.jsonl"), b"outside dir").unwrap();
    f.legacy("A", "projects/-w/s1.jsonl", b"ok");
    symlink(&secret, f.dir("A").join("projects/-w/evil.jsonl")).unwrap();
    symlink(&outside_dir, f.dir("A").join("projects/-linked")).unwrap();
    let fifo =
        std::ffi::CString::new(f.dir("A").join("projects/-w/pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let outside_before = snapshot(&f.outside);

    let r = f.ensure("A", Mode::Migrate);

    assert_eq!(snapshot(&f.outside), outside_before, "outside data touched");
    assert!(fs::symlink_metadata(f.canon.join("projects/-w/evil.jsonl")).is_err());
    assert!(fs::symlink_metadata(f.canon.join("projects/-linked")).is_err());
    assert!(fs::symlink_metadata(f.canon.join("projects/-w/pipe")).is_err());
    let backup = f.only_backup("A", "projects");
    assert!(is_link(&backup.join("-w/evil.jsonl")));
    assert!(is_link(&backup.join("-linked")));
    let lines = f.lines("A", &r);
    assert!(
        lines.iter().any(|l| l.starts_with(
            "amplihack: profile A: projects: skipped 3 symbolic links or special files; left in "
        )),
        "{lines:#?}"
    );
}

#[test]
fn a_foreign_or_dangling_link_at_a_shared_name_is_left_alone() {
    let f = Fx::new();
    f.profile("A");
    symlink(&f.outside, f.dir("A").join("projects")).unwrap();
    symlink("/nonexistent-dangling", f.dir("A").join("file-history")).unwrap();

    let r = f.ensure_no_mode("A");

    assert_eq!(
        fs::read_link(f.dir("A").join("projects")).unwrap(),
        f.outside
    );
    assert_eq!(
        fs::read_link(f.dir("A").join("file-history")).unwrap(),
        Path::new("/nonexistent-dangling")
    );
    f.assert_linked("A", "session-env");
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    let lines = f.lines("A", &r);
    assert!(
        lines.iter().any(
            |l| l.starts_with("amplihack: profile A: projects is a symlink to ")
                && l.ends_with("; left alone")
        ),
        "{lines:#?}"
    );
}

#[test]
fn a_regular_file_at_a_shared_name_is_left_alone() {
    let f = Fx::new();
    let file = f.legacy("A", "projects", b"not a dir");
    let r = f.ensure_no_mode("A");
    assert_eq!(fs::read(&file).unwrap(), b"not a dir");
    assert!(
        f.lines("A", &r).contains(
            &"amplihack: profile A: projects is not a directory or a link; left alone".to_string()
        ),
        "{:#?}",
        f.lines("A", &r)
    );
}

#[test]
fn a_symlinked_canonical_subdir_refuses_only_that_name() {
    let f = Fx::new();
    symlink(&f.outside, f.canon.join("projects")).unwrap();
    f.profile("A");

    let r = f.ensure_no_mode("A");

    assert!(fs::symlink_metadata(f.dir("A").join("projects")).is_err());
    f.assert_linked("A", "file-history");
    f.assert_linked("A", "session-env");
    assert!(
        f.lines("A", &r).contains(
            &"amplihack: profile A: ~/.claude/projects is a symlink or not a directory; projects not shared"
                .to_string()
        ),
        "{:#?}",
        f.lines("A", &r)
    );
}

#[test]
fn a_symlinked_canonical_root_is_refused_and_nothing_changes() {
    let f = Fx::new();
    let real = f.outside.join("real-claude");
    fs::create_dir(&real).unwrap();
    chmod(&real, 0o700);
    let link = f.home.join(".claude-link");
    symlink(&real, &link).unwrap();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    let before = snapshot(&f.home);

    let r = ensure_shared_session_dirs(&f.dir("A"), &n("A"), &link, || Mode::Migrate);

    assert_eq!(snapshot(&f.home), before);
    assert!(fs::read_dir(&real).unwrap().next().is_none());
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("is a symlink or not a directory; sessions not shared"));
}

#[test]
fn a_world_writable_canonical_root_is_refused() {
    for mode in [0o707, 0o777] {
        let f = Fx::new();
        chmod(&f.canon, mode);
        f.legacy("A", "projects/-w/s1.jsonl", b"x");
        let before = snapshot(&f.home);
        let r = f.ensure("A", Mode::Migrate);
        assert_eq!(snapshot(&f.home), before, "mode {mode:o}");
        assert!(
            f.lines("A", &r).contains(
                &"amplihack: profile A: ~/.claude is not owned by you or is writable by group or others; sessions not shared"
                    .to_string()
            ),
            "mode {mode:o}: {:#?}",
            f.lines("A", &r)
        );
    }
}

/// Group write is accepted only for the user's own private group, so the
/// answer depends on the host's groups; both outcomes are checked.
#[test]
fn a_group_writable_canonical_root_follows_the_private_group_rule() {
    let f = Fx::new();
    chmod(&f.canon, 0o770);
    f.profile("A");
    let refused = super::auto_switch::writable_by_others(&fs::metadata(&f.canon).unwrap());
    let before = snapshot(&f.home);

    let r = f.ensure_no_mode("A");

    let warning = "amplihack: profile A: ~/.claude is not owned by you or is writable by group or others; sessions not shared".to_string();
    if refused {
        assert_eq!(snapshot(&f.home), before);
        assert!(
            f.lines("A", &r).contains(&warning),
            "{:#?}",
            f.lines("A", &r)
        );
    } else {
        f.assert_linked("A", "projects");
        assert!(
            !f.lines("A", &r).contains(&warning),
            "{:#?}",
            f.lines("A", &r)
        );
    }
}

#[test]
fn a_canonical_subdir_writable_by_others_refuses_only_that_name() {
    let f = Fx::new();
    fs::create_dir(f.canon.join("projects")).unwrap();
    chmod(&f.canon.join("projects"), 0o707);
    f.legacy("A", "projects/-w/s1.jsonl", b"x");

    let r = f.ensure("A", Mode::Migrate);

    assert!(is_real_dir(&f.dir("A").join("projects")));
    assert_eq!(
        fs::read(f.dir("A").join("projects/-w/s1.jsonl")).unwrap(),
        b"x"
    );
    assert!(!f.canon.join("projects/-w").exists());
    f.assert_linked("A", "file-history");
    f.assert_linked("A", "session-env");
    assert!(
        f.lines("A", &r).contains(
            &"amplihack: profile A: ~/.claude/projects is not owned by you or is writable by group or others; projects not shared"
                .to_string()
        ),
        "{:#?}",
        f.lines("A", &r)
    );
}

#[test]
fn a_readable_but_not_writable_canonical_root_is_accepted() {
    let f = Fx::new();
    chmod(&f.canon, 0o755);
    f.profile("A");
    f.ensure_no_mode("A");
    f.assert_linked("A", "projects");
}

#[test]
fn a_profile_dir_with_any_group_or_other_bit_is_refused() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    chmod(&f.dir("A"), 0o750);
    let before = snapshot(&f.home);
    let r = f.ensure("A", Mode::Migrate);
    assert_eq!(snapshot(&f.home), before);
    assert!(
        f.lines("A", &r).contains(
            &"amplihack: profile A: profile directory is not owned by you or has group or other permissions; sessions not shared"
                .to_string()
        ),
        "{:#?}",
        f.lines("A", &r)
    );
}

#[test]
fn a_missing_canonical_root_is_a_silent_no_op_and_is_never_created() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    fs::remove_dir(&f.canon).unwrap();
    let r = f.ensure("A", Mode::Migrate);
    assert!(!f.canon.exists(), "~/.claude created");
    assert!(is_real_dir(&f.dir("A").join("projects")));
    assert!(r.is_noop());
    assert!(f.lines("A", &r).is_empty());
}

#[test]
fn the_canonical_root_itself_or_a_dir_inside_it_is_never_linked() {
    let f = Fx::new();
    f.shared("projects/-w/s1.jsonl", b"x");
    let before = snapshot(&f.home);
    let r = ensure_shared_session_dirs(&f.canon, &n("A"), &f.canon, || Mode::Migrate);
    assert!(r.is_noop());
    assert_eq!(snapshot(&f.home), before);
    let inside = f.canon.join("nested");
    fs::create_dir(&inside).unwrap();
    chmod(&inside, 0o700);
    let before_inside = snapshot(&f.home);
    let r = ensure_shared_session_dirs(&inside, &n("nested"), &f.canon, || Mode::Migrate);
    assert!(r.is_noop());
    assert_eq!(snapshot(&f.home), before_inside);
    assert!(is_real_dir(&f.canon.join("projects")));
}

#[test]
fn a_prefix_lookalike_of_the_canonical_root_is_not_inside_it() {
    let f = Fx::new();
    assert!(
        f.root
            .to_string_lossy()
            .starts_with(&*f.canon.to_string_lossy())
    );
    f.profile("A");
    let r = f.ensure_no_mode("A");
    assert_eq!(r.linked.len(), 3, "{:?}", r.warnings);
}

#[test]
fn the_opt_out_marker_stops_all_linking_and_migration() {
    for marker_kind in ["file", "dangling-link"] {
        let f = Fx::new();
        f.legacy("A", "projects/-w/s1.jsonl", b"x");
        let marker = f.dir("A").join(".no-shared-sessions");
        if marker_kind == "file" {
            fs::write(&marker, b"").unwrap();
        } else {
            symlink("/nonexistent", &marker).unwrap();
        }
        let before = snapshot(&f.home);
        let r = f.ensure("A", Mode::Migrate);
        assert_eq!(snapshot(&f.home), before, "{marker_kind}");
        assert!(r.is_noop(), "{marker_kind}");
        assert!(f.lines("A", &r).is_empty(), "{marker_kind}");
    }
}

#[test]
fn removing_the_opt_out_marker_lets_the_next_call_migrate() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    let marker = f.dir("A").join(".no-shared-sessions");
    fs::write(&marker, b"").unwrap();
    f.ensure("A", Mode::Migrate);
    fs::remove_file(&marker).unwrap();
    f.ensure("A", Mode::Migrate);
    f.assert_linked("A", "projects");
    assert!(f.canon.join("projects/-w/s1.jsonl").is_file());
}

// ------------------------------------------------------ store integration

fn shared_store(f: &Fx) -> ProfileStore {
    ProfileStore::new(f.root.clone()).with_shared_assets(f.canon.clone())
}

#[test]
fn add_links_the_new_profile_when_the_canonical_root_exists() {
    let f = Fx::new();
    let dir = shared_store(&f).add(&n("A")).unwrap();
    for name in SHARED_SESSION_DIRS {
        f.assert_linked("A", name);
    }
    let regular: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| !e.file_type().unwrap().is_symlink())
        .map(|e| e.file_name())
        .collect();
    assert!(regular.is_empty(), "add created {regular:?}");
    assert_no_credentials_under(&f.canon);
}

#[test]
fn add_without_the_canonical_root_creates_no_links() {
    let f = Fx::new();
    fs::remove_dir(&f.canon).unwrap();
    let dir = shared_store(&f).add(&n("A")).unwrap();
    assert_eq!(fs::read_dir(dir).unwrap().count(), 0);
    assert!(!f.canon.exists());
}

#[test]
fn remove_unlinks_without_following_and_keeps_the_shared_sessions() {
    let f = Fx::new();
    let s = shared_store(&f);
    s.add(&n("A")).unwrap();
    s.add(&n("B")).unwrap();
    fs::create_dir_all(f.dir("A").join("projects/-w")).unwrap();
    fs::write(f.dir("A").join("projects/-w/s1.jsonl"), b"keep").unwrap();
    fs::create_dir_all(f.dir("A").join("file-history/s1")).unwrap();
    // A backup inside the profile goes with it.
    fs::create_dir(f.dir("A").join("projects.pre-share-20260101T000000Z")).unwrap();

    s.remove(&n("A"), true).unwrap();

    assert!(!f.dir("A").exists());
    assert_eq!(
        fs::read(f.canon.join("projects/-w/s1.jsonl")).unwrap(),
        b"keep"
    );
    assert!(f.canon.join("file-history/s1").is_dir());
    assert_eq!(
        fs::read(f.dir("B").join("projects/-w/s1.jsonl")).unwrap(),
        b"keep"
    );
}

#[test]
fn ensure_for_without_a_home_does_nothing() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    let before = snapshot(&f.home);
    shared_dirs::ensure_for(&ProfileStore::new(f.root.clone()), &n("A"));
    assert_eq!(snapshot(&f.home), before);
}

#[cfg(target_os = "linux")]
#[test]
fn ensure_for_migrates_a_profile_no_process_is_using() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/s1.jsonl", b"x");
    shared_dirs::ensure_for(&shared_store(&f), &n("A"));
    f.assert_linked("A", "projects");
    assert!(f.canon.join("projects/-w/s1.jsonl").is_file());
    assert_no_credentials_under(&f.canon);
}

#[test]
fn a_same_filesystem_move_makes_the_file_owner_only() {
    let f = Fx::new();
    let src = f.legacy("A", "projects/-w/s1.jsonl", b"transcript");
    chmod(&src, 0o644);
    let hist = f.legacy("A", "file-history/x/v1", b"old .env contents");
    chmod(&hist, 0o664);

    let r = f.ensure("A", Mode::Migrate);

    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(mode_of(&f.canon.join("projects/-w/s1.jsonl")), 0o600);
    assert_eq!(mode_of(&f.canon.join("file-history/x/v1")), 0o600);
}

#[test]
fn a_conflicting_memory_index_keeps_the_shared_copy_and_gains_the_missing_entries() {
    let f = Fx::new();
    let shared_index = f.shared(
        "projects/-w/memory/MEMORY.md",
        b"# Memory\n- [Shared](shared.md) \xe2\x80\x94 from the default login\n- [Both](both.md)",
    );
    f.shared("projects/-w/memory/shared.md", b"shared note");
    f.shared("projects/-w/memory/clash.md", b"shared clash");
    f.shared("projects/-w/memory/both.md", b"both");
    let profile_index = b"# Profile memory\n\n- [Both](both.md)\n- [Mine](mine.md) \xe2\x80\x94 only in A\n- [Clash](clash.md)\n";
    f.legacy("A", "projects/-w/memory/MEMORY.md", profile_index);
    f.legacy("A", "projects/-w/memory/mine.md", b"profile note");
    f.legacy("A", "projects/-w/memory/clash.md", b"profile clash");
    f.legacy("A", "projects/-w/memory/both.md", b"both");

    let r = f.ensure("A", Mode::Migrate);

    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let index = fs::read_to_string(&shared_index).unwrap();
    assert_eq!(
        index,
        "# Memory\n- [Shared](shared.md) \u{2014} from the default login\n- [Both](both.md)\n\
         - [Mine](mine.md) \u{2014} only in A\n- [Clash](clash.md.conflict-A)\n"
    );
    // The profile's index and the losing memory file are still kept whole.
    assert_eq!(
        fs::read(f.canon.join("projects/-w/memory/MEMORY.md.conflict-A")).unwrap(),
        profile_index
    );
    assert_eq!(
        fs::read(f.canon.join("projects/-w/memory/clash.md.conflict-A")).unwrap(),
        b"profile clash"
    );
    assert_eq!(
        fs::read(f.canon.join("projects/-w/memory/mine.md")).unwrap(),
        b"profile note"
    );
    let lines = f.lines("A", &r);
    assert!(
        lines.contains(
            &"amplihack: profile A: projects: memory index -w/memory/MEMORY.md: added 2 entries from this profile to the shared copy"
                .to_string()
        ),
        "{lines:#?}"
    );

    // A second migration of the same index adds nothing twice.
    fs::remove_file(f.dir("A").join("projects")).unwrap();
    f.legacy("A", "projects/-w/memory/MEMORY.md", profile_index);
    f.legacy("A", "projects/-w/memory/clash.md", b"profile clash");
    let again = f.ensure("A", Mode::Migrate);
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert_eq!(fs::read_to_string(&shared_index).unwrap(), index);
}

#[test]
fn a_memory_index_the_shared_store_lacks_is_simply_moved() {
    let f = Fx::new();
    f.legacy("A", "projects/-w/memory/MEMORY.md", b"- [Mine](mine.md)\n");
    let r = f.ensure("A", Mode::Migrate);
    assert_eq!(
        fs::read(f.canon.join("projects/-w/memory/MEMORY.md")).unwrap(),
        b"- [Mine](mine.md)\n"
    );
    assert!(r.conflicts.is_empty());
    assert!(!f.lines("A", &r).iter().any(|l| l.contains("memory index")));
}

#[test]
fn an_index_outside_a_project_memory_dir_is_an_ordinary_conflict() {
    let f = Fx::new();
    let shared = f.shared("projects/-w/MEMORY.md", b"- [A](a.md)\n");
    f.legacy("A", "projects/-w/MEMORY.md", b"- [B](b.md)\n");
    let r = f.ensure("A", Mode::Migrate);
    assert_eq!(fs::read(&shared).unwrap(), b"- [A](a.md)\n");
    assert_eq!(r.conflicts.len(), 1);
}

#[test]
fn a_memory_index_that_cannot_be_appended_to_is_reported_and_the_merge_goes_on() {
    let f = Fx::new();
    let shared_index = f.shared("projects/-w/memory/MEMORY.md", b"- [S](s.md)\n");
    chmod(&shared_index, 0o400);
    f.legacy("A", "projects/-w/memory/MEMORY.md", b"- [Mine](mine.md)\n");
    f.legacy("A", "projects/-x/s2.jsonl", b"other project");

    let r = f.ensure("A", Mode::Migrate);
    chmod(&shared_index, 0o600);

    assert_eq!(fs::read(&shared_index).unwrap(), b"- [S](s.md)\n");
    assert_eq!(
        fs::read(f.canon.join("projects/-w/memory/MEMORY.md.conflict-A")).unwrap(),
        b"- [Mine](mine.md)\n"
    );
    assert_eq!(
        fs::read(f.canon.join("projects/-x/s2.jsonl")).unwrap(),
        b"other project",
        "the rest of the merge stopped"
    );
    f.assert_linked("A", "projects");
    let lines = f.lines("A", &r);
    assert!(
        lines.iter().any(|l| l.starts_with(
            "amplihack: profile A: projects: memory index -w/memory/MEMORY.md: entries not added ("
        ) && l
            .ends_with("add them by hand from ~/.claude/projects/-w/memory/MEMORY.md.conflict-A")),
        "{lines:#?}"
    );
}

#[test]
fn memory_links_are_rewritten_once_even_when_conflict_names_chain() {
    let f = Fx::new();
    f.shared("projects/-w/memory/MEMORY.md", b"- [S](s.md)\n");
    f.shared("projects/-w/memory/a.md", b"shared a");
    f.shared("projects/-w/memory/a.md.conflict-A", b"shared second");
    f.legacy(
        "A",
        "projects/-w/memory/MEMORY.md",
        b"- [A](a.md)\n- [A2](a.md.conflict-A)\n",
    );
    f.legacy("A", "projects/-w/memory/a.md", b"profile a");
    f.legacy("A", "projects/-w/memory/a.md.conflict-A", b"profile second");

    f.ensure("A", Mode::Migrate);

    let index = fs::read_to_string(f.canon.join("projects/-w/memory/MEMORY.md")).unwrap();
    let saved_a = |target: &str| {
        let line = index
            .lines()
            .find(|l| l.starts_with(target))
            .unwrap()
            .to_string();
        let name = line
            .split("](")
            .nth(1)
            .unwrap()
            .trim_end_matches(')')
            .to_string();
        (
            fs::read(f.canon.join("projects/-w/memory").join(&name)).unwrap(),
            name,
        )
    };
    assert_eq!(saved_a("- [A](").0, b"profile a", "{index}");
    assert_eq!(saved_a("- [A2](").0, b"profile second", "{index}");
}
