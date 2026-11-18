//! Unit tests for `recovery_path.rs` (issue #2346).
//!
//! Wire with, at the end of `recovery_path.rs`:
//! `#[cfg(test)] #[path = "tests_recovery_path.rs"] mod tests;`
//! and register the module in `cleanup/mod.rs` with
//! `pub(crate) mod recovery_path;`.
//!
//! Contract (the design's M1, with the documentation step's revisions in
//! `docs/reference/recipe-command.md#work-root-permissions`):
//!
//! ```text
//! pub(crate) enum Fault {
//!     Writable { path: PathBuf, mode: u32 },      // ancestor, 0o022 bit, no sticky bit
//!     NotPrivate { path: PathBuf, mode: u32 },    // work root or runs/ that is not private
//!     NotOwned { path: PathBuf, uid: u32 },
//!     Link { path: PathBuf, target: Option<PathBuf> },
//!     NotFolder { path: PathBuf },
//!     Inspect { path: PathBuf, error: String },
//!     NonNormal { path: PathBuf },
//! }
//! pub(crate) fn ancestor_mode_ok(mode: u32) -> bool
//! pub(crate) fn path_faults(dir: &Path) -> Vec<Fault>
//! pub(crate) enum RootMode { NotOwned, /* nonprivate modes */, Private }
//! pub(crate) fn root_mode(uid: u32, euid: u32, mode: u32) -> RootMode
//! pub(crate) fn prepare_work_root(root: &Path) -> anyhow::Result<()>
//! pub(crate) fn shell_word(path: &Path) -> Option<String>
//! ```
//!
//! Paths in messages go through `commands::install::paths::sanitize_path`
//! (control characters become `?`, invalid UTF-8 becomes U+FFFD). Suggested
//! commands and `AMPLIHACK_WORK_ROOT` values go through `shell_word`.
//!
//! Every fixture lives in a temporary folder created 0700 with an explicit
//! mode, so these tests give the same answer under umask 0002 and 0022. The
//! runner's own `Directory::open` must accept that folder first; when it does
//! not, `TMPDIR` sits below a folder the recovery store's rule stops, and the
//! test says so instead of failing on a fixture it could not build.
//!
//! No test changes the process umask or any owner.

use super::*;
use recipe_runner_rs::recovery::private_fs::Directory;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

// --------------------------------------------------------------- fixtures

/// A temporary folder created 0700 whatever the umask, canonical, and
/// accepted by the pinned runner as a place for a recovery store.
fn private_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("rp-2346-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a 0700 temporary folder");
    let root = tmp.path().canonicalize().unwrap();
    if let Err(error) = Directory::open(&root, false) {
        panic!(
            "the temporary folder {} is not usable for a recovery store ({error:#}); a folder \
             above TMPDIR ({}) is group- or world-writable without the sticky bit, or is a \
             symbolic link. Run the tests with TMPDIR under folders only you can write.",
            root.display(),
            std::env::temp_dir().display()
        );
    }
    (tmp, root)
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("chmod {mode:04o} {}: {e}", path.display()));
}

/// The permission bits, special bits included, without following a link, as
/// octal text, so a failure reads `0775` rather than `509`.
fn mode_of(path: &Path) -> String {
    let meta =
        fs::symlink_metadata(path).unwrap_or_else(|e| panic!("lstat {}: {e}", path.display()));
    octal(meta.mode())
}

fn octal(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

/// `fs::create_dir` and an explicit chmod, so the umask plays no part.
fn dir(path: &Path, mode: u32) -> PathBuf {
    fs::create_dir(path).unwrap_or_else(|e| panic!("mkdir {}: {e}", path.display()));
    set_mode(path, mode);
    assert_eq!(
        mode_of(path),
        octal(mode),
        "the fixture could not give {} mode {mode:04o}",
        path.display()
    );
    path.to_path_buf()
}

/// Gives a folder made unreadable on purpose its owner's access back, so the
/// temporary folder can be removed even when an assertion fails.
struct Restore(PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Root bypasses permission checks, so a test that needs `EACCES` cannot
/// arrange one when run as root.
fn running_as_root() -> bool {
    euid() == 0
}

/// The documented quoting rule, written out independently of `shell_word`,
/// so an exact-line assertion holds whatever characters TMPDIR contains.
fn word(path: &Path) -> String {
    let text = path.to_str().expect("UTF-8 fixture path");
    let plain = text
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._+,:@%=-".contains(&b));
    if plain {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The whole error, context chain included.
fn message(result: anyhow::Result<()>) -> String {
    match result {
        Ok(()) => panic!("expected the work root to be stopped, but it was accepted"),
        Err(error) => format!("{error:#}"),
    }
}

fn has_line(text: &str, expected: &str) -> bool {
    text.lines().any(|line| line == expected)
}

fn fault(f: &Fault) -> (&'static str, PathBuf) {
    match f {
        Fault::Writable { path, .. } => ("writable", path.clone()),
        Fault::NotPrivate { path, .. } => ("not-private", path.clone()),
        Fault::NotOwned { path, .. } => ("not-owned", path.clone()),
        Fault::Link { path, .. } => ("link", path.clone()),
        Fault::NotFolder { path } => ("not-folder", path.clone()),
        Fault::Inspect { path, .. } => ("inspect", path.clone()),
        Fault::NonNormal { path } => ("non-normal", path.clone()),
    }
}

fn kinds(faults: &[Fault]) -> Vec<(&'static str, PathBuf)> {
    faults.iter().map(fault).collect()
}

// ------------------------------------------------------------ pure rules

#[test]
fn ancestor_rule_is_no_group_or_other_write_or_the_sticky_bit() {
    for (mode, ok) in [
        (0o700, true),
        (0o750, true),
        (0o755, true),
        (0o555, true),
        (0o1777, true),
        (0o1775, true),
        (0o1757, true),
        // With the file-type bits of a folder, as `st_mode` carries them.
        (0o40755, true),
        (0o41777, true),
        (0o775, false),
        (0o770, false),
        (0o2775, false),
        (0o757, false),
        (0o777, false),
        (0o40775, false),
    ] {
        assert_eq!(
            ancestor_mode_ok(mode),
            ok,
            "mode {mode:o}: expected {}",
            if ok { "pass" } else { "stop" }
        );
    }
}

#[test]
fn root_mode_sorts_owner_and_mode_bits() {
    let me = 1000;
    for mode in [0o700, 0o2700, 0o1700, 0o40700] {
        assert_eq!(
            root_mode(me, me, mode),
            RootMode::Private,
            "an owned root with mode {mode:o}"
        );
    }
    for mode in [0o755, 0o750, 0o711, 0o500, 0o2755, 0o775, 0o777] {
        assert_ne!(root_mode(me, me, mode), RootMode::Private);
    }
    for mode in [0o700, 0o755, 0o775, 0o777] {
        assert_eq!(
            root_mode(me + 1, me, mode),
            RootMode::NotOwned,
            "a root owned by another uid, mode {mode:o}"
        );
    }
}

// ------------------------------------------------------------ shell_word

#[test]
fn shell_word_leaves_a_plain_path_as_it_is() {
    for plain in [
        "/home/alice/.amplihack/work",
        "/srv/canary",
        "/data/a+b,c:d@e%f=g-h_i.j/work",
    ] {
        assert_eq!(shell_word(Path::new(plain)).as_deref(), Some(plain));
    }
}

#[test]
fn shell_word_single_quotes_any_other_path_and_the_quoted_form_reads_back() {
    assert_eq!(
        shell_word(Path::new("/srv/team share")).as_deref(),
        Some("'/srv/team share'")
    );
    assert_eq!(
        shell_word(Path::new("/srv/it's")).as_deref(),
        Some(r"'/srv/it'\''s'")
    );
    for raw in [
        "/srv/team share",
        "/srv/it's",
        "/srv/$(id)",
        "/srv/`id`",
        "/srv/a;b",
        "/srv/*",
        "/srv/~x",
        "/srv/a!b",
        "/srv/a\"b",
        r"/srv/a\b",
        "/srv/a&b|c",
        "/srv/ünïcode",
    ] {
        let quoted = shell_word(Path::new(raw))
            .unwrap_or_else(|| panic!("{raw:?} must get a suggested command"));
        assert!(
            quoted.starts_with('\'') && quoted.ends_with('\''),
            "{raw:?} must be single-quoted, got {quoted}"
        );
        // The quoted form, pasted into a shell, is the path again and runs
        // nothing.
        let out = Command::new("/bin/sh")
            .env_clear()
            .arg("-c")
            .arg(format!("printf %s {quoted}"))
            .output()
            .expect("run /bin/sh");
        assert!(out.status.success(), "{quoted}: {out:?}");
        assert_eq!(
            out.stdout,
            raw.as_bytes(),
            "{quoted} must read back as {raw:?}"
        );
    }
}

#[test]
fn shell_word_gives_nothing_for_control_characters_or_invalid_utf8() {
    for raw in ["/a\tb", "/a\nb", "/a\x1b[2Jb", "/a\x7fb", "/a\u{85}b"] {
        assert_eq!(
            shell_word(Path::new(raw)),
            None,
            "{raw:?} must get no suggested command"
        );
    }
    let invalid = Path::new(OsStr::from_bytes(b"/srv/bad\xff"));
    assert_eq!(shell_word(invalid), None);
}

// ------------------------------------------------------------ path_faults

#[test]
fn path_faults_names_every_open_folder_outermost_first() {
    let (_tmp, t) = private_root();
    let a = dir(&t.join("a"), 0o775);
    let b = dir(&a.join("b"), 0o757);
    let faults = path_faults(&b);
    assert_eq!(
        kinds(&faults),
        vec![("writable", a.clone()), ("writable", b.clone())]
    );
    let modes: Vec<u32> = faults
        .iter()
        .map(|f| match f {
            Fault::Writable { mode, .. } => mode & 0o7777,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(modes, vec![0o775, 0o757]);
}

#[test]
fn path_faults_stops_at_a_symbolic_link_and_records_its_target() {
    let (_tmp, t) = private_root();
    let real = dir(&t.join("real"), 0o700);
    // Past the link, an open folder that the walk must not reach.
    dir(&real.join("x"), 0o775);
    let link = t.join("l");
    symlink(&real, &link).unwrap();
    let faults = path_faults(&link.join("x"));
    assert_eq!(kinds(&faults), vec![("link", link.clone())]);
    match &faults[0] {
        Fault::Link { target, .. } => assert_eq!(target.as_deref(), Some(real.as_path())),
        _ => unreachable!(),
    }
}

#[test]
fn path_faults_keeps_open_folders_found_before_a_link() {
    let (_tmp, t) = private_root();
    let real = dir(&t.join("real"), 0o700);
    let a = dir(&t.join("a"), 0o775);
    let link = a.join("l");
    symlink(&real, &link).unwrap();
    assert_eq!(
        kinds(&path_faults(&link.join("x"))),
        vec![("writable", a), ("link", link)]
    );
}

#[test]
fn path_faults_names_a_file_where_a_folder_is_expected() {
    let (_tmp, t) = private_root();
    let file = t.join("f");
    fs::write(&file, b"not a folder").unwrap();
    assert_eq!(
        kinds(&path_faults(&file.join("x"))),
        vec![("not-folder", file)]
    );
}

#[test]
fn path_faults_ends_quietly_at_the_first_missing_folder() {
    let (_tmp, t) = private_root();
    assert!(path_faults(&t.join("missing/deeper")).is_empty());
    let a = dir(&t.join("a"), 0o775);
    assert_eq!(
        kinds(&path_faults(&a.join("missing/deeper"))),
        vec![("writable", a)]
    );
}

#[test]
fn path_faults_names_a_folder_it_cannot_inspect() {
    if running_as_root() {
        eprintln!("skipped: root is never refused a lookup");
        return;
    }
    let (_tmp, t) = private_root();
    let locked = dir(&t.join("locked"), 0o700);
    let _restore = Restore(locked.clone());
    set_mode(&locked, 0o000);
    let faults = path_faults(&locked.join("x"));
    assert_eq!(faults.len(), 1, "{:?}", kinds(&faults));
    let (kind, path) = fault(&faults[0]);
    assert_eq!(kind, "inspect");
    assert!(path.starts_with(&locked), "{}", path.display());
    let text = message(prepare_work_root(&locked.join("x/work")));
    assert!(text.contains(&locked.display().to_string()), "{text}");
    assert!(text.contains("Permission denied"), "{text}");
}

// ----------------------------------------------------- the work root itself

#[test]
fn an_owned_nonprivate_root_is_named_with_chmod_700_and_left_unchanged() {
    let (_tmp, t) = private_root();
    for mode in [0o755, 0o750, 0o711, 0o500, 0o2755] {
        let root = dir(&t.join(format!("w{mode:o}")), mode);
        let result = prepare_work_root(&root);
        assert_eq!(
            mode_of(&root),
            octal(mode),
            "an existing {mode:04o} root must never be changed"
        );
        let text = message(result);
        for part in [
            root.display().to_string(),
            format!("mode {mode:04o}"),
            format!("chmod 700 {}", word(&root)),
        ] {
            assert!(text.contains(&part), "missing {part:?}: {text}");
        }
    }
}

#[test]
fn a_private_root_is_left_as_it_is_including_setgid_and_sticky() {
    let (_tmp, t) = private_root();
    for mode in [0o700, 0o2700, 0o1700] {
        let root = dir(&t.join(format!("w{mode:o}")), mode);
        prepare_work_root(&root).unwrap_or_else(|e| panic!("{mode:04o}: {e:#}"));
        assert_eq!(
            mode_of(&root),
            octal(mode),
            "a private root is never changed"
        );
    }
}

#[test]
fn a_root_open_to_group_or_others_is_named_with_chmod_700_and_left_unchanged() {
    let (_tmp, t) = private_root();
    for mode in [0o775, 0o770, 0o757, 0o777] {
        let root = dir(&t.join(format!("w{mode:o}")), mode);
        let text = message(prepare_work_root(&root));
        assert!(text.contains(&root.display().to_string()), "{text}");
        assert!(text.contains(&format!("mode {mode:04o}")), "{text}");
        assert!(
            text.contains(&format!("`chmod 700 {}`", word(&root))),
            "{text}"
        );
        assert_eq!(mode_of(&root), octal(mode), "the root is never changed");
    }
}

#[test]
fn a_missing_root_is_accepted_and_not_created() {
    let (_tmp, t) = private_root();
    let root = t.join("new/work");
    prepare_work_root(&root).unwrap();
    assert!(
        !t.join("new").exists(),
        "the check creates nothing; the runner creates the root at 0700"
    );
}

#[test]
fn a_root_that_is_a_symbolic_link_is_named_with_its_target_and_not_followed() {
    let (_tmp, t) = private_root();
    let real = dir(&t.join("real"), 0o755);
    let root = t.join("work");
    symlink(&real, &root).unwrap();
    let text = message(prepare_work_root(&root));
    let expected = format!(
        "work root {} is a symbolic link to {}; the recovery store needs a folder, not a \
         link. Set AMPLIHACK_WORK_ROOT to {}",
        root.display(),
        real.display(),
        word(&real)
    );
    assert!(
        has_line(&text, &expected),
        "expected {expected:?} in:\n{text}"
    );
    assert!(
        fs::symlink_metadata(&root)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        mode_of(&real),
        octal(0o755),
        "the link is never followed to change a mode"
    );
}

#[test]
fn a_root_that_is_a_file_is_named() {
    let (_tmp, t) = private_root();
    let root = t.join("work");
    fs::write(&root, b"file").unwrap();
    let text = message(prepare_work_root(&root));
    assert!(text.contains(&root.display().to_string()), "{text}");
    assert!(text.contains("not a folder"), "{text}");
}

#[test]
fn an_unreadable_root_stops_with_chmod_700_and_is_unchanged() {
    if running_as_root() {
        eprintln!("skipped: root can open any folder");
        return;
    }
    let (_tmp, t) = private_root();
    for mode in [0o300, 0o100] {
        let root = dir(&t.join(format!("w{mode:o}")), 0o700);
        let _restore = Restore(root.clone());
        set_mode(&root, mode);
        let text = message(prepare_work_root(&root));
        assert!(text.contains(&root.display().to_string()), "{text}");
        assert!(
            text.contains(&format!("`chmod 700 {}`", word(&root))),
            "{text}"
        );
        assert_eq!(mode_of(&root), octal(mode), "nothing is changed");
    }
}

#[test]
fn repeated_validation_never_changes_an_existing_root() {
    let (_tmp, t) = private_root();
    let root = dir(&t.join("work"), 0o755);
    let parked = t.join("parked");
    let first = prepare_work_root(&root);
    assert_eq!(mode_of(&root), "0755", "the first check changed the root");
    assert!(first.is_err());
    fs::rename(&root, &parked).unwrap();
    dir(&root, 0o755);
    let text = message(prepare_work_root(&root));
    assert!(text.contains(&root.display().to_string()), "{text}");
    assert_eq!(
        mode_of(&root),
        octal(0o755),
        "a different folder is never changed"
    );
    assert_eq!(
        mode_of(&parked),
        octal(0o755),
        "the inspected folder is not reached"
    );
}

#[test]
fn ownership_diagnostic_names_both_uids_and_requires_a_correctly_owned_location() {
    let (_tmp, t) = private_root();
    let root = t.join("work");
    let owner = euid().wrapping_add(1);
    let text = describe(
        &root,
        &[Fault::NotOwned {
            path: root.clone(),
            uid: owner,
        }],
    );
    for part in [
        root.display().to_string(),
        format!("uid {owner}"),
        format!("uid {}", euid()),
    ] {
        assert!(text.contains(&part), "missing {part:?}: {text}");
    }
    assert!(
        text.contains("correctly owned") || text.contains("owned by"),
        "{text}"
    );
    assert!(
        !text.contains("chmod"),
        "mode advice cannot fix ownership: {text}"
    );
}

// ------------------------------------------------------------- runs/

#[test]
fn an_open_runs_folder_is_named_and_left_unchanged() {
    let (_tmp, t) = private_root();
    for mode in [0o775, 0o755, 0o750] {
        let root = dir(&t.join(format!("w{mode:o}")), 0o700);
        let runs = dir(&root.join("runs"), mode);
        let text = message(prepare_work_root(&root));
        let header = format!(
            "cannot open the recovery store under work root {}: runs folder is not private",
            root.display()
        );
        let line = format!(
            "  {} has mode {mode:04o}; fix it with `chmod 700 {}`",
            runs.display(),
            word(&runs)
        );
        assert!(has_line(&text, &header), "expected {header:?} in:\n{text}");
        assert!(has_line(&text, &line), "expected {line:?} in:\n{text}");
        assert_eq!(mode_of(&runs), octal(mode), "runs/ is never changed");
    }
}

#[test]
fn a_runs_link_or_file_is_named() {
    let (_tmp, t) = private_root();
    let elsewhere = dir(&t.join("elsewhere"), 0o700);

    let root = dir(&t.join("linked"), 0o700);
    symlink(&elsewhere, root.join("runs")).unwrap();
    let text = message(prepare_work_root(&root));
    assert!(
        text.contains(&root.join("runs").display().to_string()),
        "{text}"
    );

    let root = dir(&t.join("file"), 0o700);
    fs::write(root.join("runs"), b"file").unwrap();
    let text = message(prepare_work_root(&root));
    assert!(
        text.contains(&root.join("runs").display().to_string()),
        "{text}"
    );
}

#[test]
fn a_private_runs_folder_passes() {
    let (_tmp, t) = private_root();
    let root = dir(&t.join("work"), 0o700);
    dir(&root.join("runs"), 0o700);
    prepare_work_root(&root).unwrap();
}

// ------------------------------------------------------ order of checks

#[test]
fn no_mode_changes_when_runs_stops_the_run() {
    let (_tmp, t) = private_root();
    let root = dir(&t.join("work"), 0o755);
    let runs = dir(&root.join("runs"), 0o775);
    let text = message(prepare_work_root(&root));
    assert!(text.contains(&runs.display().to_string()), "{text}");
    assert_eq!(
        mode_of(&root),
        octal(0o755),
        "an existing root is never changed"
    );
    assert!(
        !text.contains("set it to 0700"),
        "no notice for a change that was not made: {text}"
    );
}

#[test]
fn no_mode_changes_when_a_folder_above_stops_the_run() {
    let (_tmp, t) = private_root();
    let above = dir(&t.join("shared"), 0o775);
    let root = dir(&above.join("work"), 0o755);
    let text = message(prepare_work_root(&root));
    assert!(text.contains(&above.display().to_string()), "{text}");
    assert_eq!(mode_of(&root), octal(0o755), "the root is not changed");
    assert_eq!(
        mode_of(&above),
        octal(0o775),
        "a folder above is never changed"
    );
}

// ---------------------------------------------- folders above the work root

#[test]
fn a_group_writable_folder_above_the_root_gives_the_documented_message() {
    let (_tmp, t) = private_root();
    let canary = dir(&t.join("canary"), 0o775);
    let root = canary.join("work");
    let text = message(prepare_work_root(&root));
    let header = format!(
        "cannot open the recovery store under work root {}: other users can change a folder \
         above it",
        root.display()
    );
    let line = format!(
        "  {} is group-writable (mode 0775) and has no sticky bit; fix it with `chmod g-w {}`",
        canary.display(),
        word(&canary)
    );
    assert!(has_line(&text, &header), "expected {header:?} in:\n{text}");
    assert!(has_line(&text, &line), "expected {line:?} in:\n{text}");
    assert!(
        !root.exists(),
        "nothing is created below the failing folder"
    );
    assert_eq!(
        mode_of(&canary),
        octal(0o775),
        "a folder above is never changed"
    );
}

#[test]
fn the_fix_follows_the_write_bits() {
    let (_tmp, t) = private_root();

    let world = dir(&t.join("world"), 0o757);
    let text = message(prepare_work_root(&world.join("work")));
    assert!(
        text.contains(&format!(
            "{} is world-writable (mode 0757) and has no sticky bit",
            world.display()
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!("`chmod o-w {}`", word(&world))),
        "{text}"
    );
    assert!(!text.contains("group-writable"), "{text}");

    let both = dir(&t.join("both"), 0o777);
    let text = message(prepare_work_root(&both.join("work")));
    for part in [
        "group-writable",
        "world-writable",
        "(mode 0777)",
        "has no sticky bit",
    ] {
        assert!(text.contains(part), "{part:?} missing from:\n{text}");
    }
    assert!(
        text.contains(&format!("`chmod go-w {}`", word(&both))),
        "{text}"
    );

    let setgid = dir(&t.join("setgid"), 0o2775);
    let text = message(prepare_work_root(&setgid.join("work")));
    assert!(text.contains("(mode 2775)"), "{text}");
    assert!(
        text.contains(&format!("`chmod g-w {}`", word(&setgid))),
        "{text}"
    );
}

#[test]
fn sticky_or_closed_folders_above_the_root_pass() {
    let (_tmp, t) = private_root();
    for mode in [0o1777, 0o1775, 0o755, 0o750, 0o555] {
        let above = dir(&t.join(format!("a{mode:o}")), mode);
        prepare_work_root(&above.join("work"))
            .unwrap_or_else(|e| panic!("a {mode:04o} folder above must pass: {e:#}"));
    }
}

#[test]
fn two_open_folders_are_both_named_outermost_first() {
    let (_tmp, t) = private_root();
    let a = dir(&t.join("a"), 0o775);
    let b = dir(&a.join("b"), 0o757);
    let text = message(prepare_work_root(&b.join("work")));
    let first = format!("  {} is group-writable (mode 0775)", a.display());
    let second = format!("  {} is world-writable (mode 0757)", b.display());
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing from:\n{text}"))
    };
    assert!(at(&first) < at(&second), "outermost first:\n{text}");
}

#[test]
fn a_folder_with_a_space_is_printed_plain_and_quoted_in_the_command() {
    let (_tmp, t) = private_root();
    let team = dir(&t.join("team share"), 0o775);
    let text = message(prepare_work_root(&team.join("work")));
    let line = format!(
        "  {} is group-writable (mode 0775) and has no sticky bit; fix it with `chmod g-w {}`",
        team.display(),
        word(&team)
    );
    assert!(
        word(&team).starts_with('\''),
        "the fixture path must need quotes"
    );
    assert!(has_line(&text, &line), "expected {line:?} in:\n{text}");
}

#[test]
fn a_symbolic_link_above_the_root_is_named_with_its_target_and_the_resolved_root() {
    let (_tmp, t) = private_root();
    let real = dir(&t.join("real"), 0o700);
    let scratch = t.join("scratch");
    symlink(&real, &scratch).unwrap();
    let root = scratch.join("work");
    let text = message(prepare_work_root(&root));
    let header = format!(
        "cannot open the recovery store under work root {}: a folder above it is a symbolic link",
        root.display()
    );
    let line = format!(
        "  {} is a symbolic link to {}; the recovery store needs a folder, not a link, at each \
         step of the path. Set AMPLIHACK_WORK_ROOT to {}",
        scratch.display(),
        real.display(),
        word(&real.join("work"))
    );
    assert!(has_line(&text, &header), "expected {header:?} in:\n{text}");
    assert!(has_line(&text, &line), "expected {line:?} in:\n{text}");
    assert!(!real.join("work").exists(), "nothing is created");
}

#[test]
fn control_characters_in_a_link_target_are_printed_as_question_marks() {
    let (_tmp, t) = private_root();
    let link = t.join("l");
    symlink(OsStr::from_bytes(b"/x\x1b[2J\ny"), &link).unwrap();
    let text = message(prepare_work_root(&link.join("work")));
    assert!(!text.contains('\x1b'), "raw escape in:\n{text:?}");
    assert!(
        text.contains("/x?[2J?y"),
        "sanitised target missing:\n{text}"
    );
    assert!(
        !text.lines().any(|line| line.starts_with('y')),
        "a newline in the target must not start a line of its own:\n{text}"
    );
}

#[test]
fn a_folder_name_with_a_control_character_gets_no_command() {
    let (_tmp, t) = private_root();
    let odd = dir(&t.join("a\tb"), 0o775);
    let text = message(prepare_work_root(&odd.join("work")));
    assert!(
        text.contains(&format!(
            "{}/a?b is group-writable (mode 0775)",
            t.display()
        )),
        "{text}"
    );
    assert!(!text.contains('\t'), "raw tab in:\n{text:?}");
    assert!(
        !text.contains("`chmod"),
        "a printed form that cannot be pasted back gets no command:\n{text}"
    );
}

#[test]
fn a_folder_name_that_is_not_utf8_gets_no_command() {
    let (_tmp, t) = private_root();
    let bad = t.join(OsStr::from_bytes(b"bad\xff"));
    if let Err(error) = fs::create_dir(&bad) {
        eprintln!("skipped: this filesystem does not take a non-UTF-8 name: {error}");
        return;
    }
    set_mode(&bad, 0o775);
    let text = message(prepare_work_root(&bad.join("work")));
    assert!(
        text.contains(&format!(
            "{}/bad\u{FFFD} is group-writable (mode 0775)",
            t.display()
        )),
        "{text}"
    );
    assert!(!text.contains("`chmod"), "{text}");
}

#[test]
fn a_relative_or_dotted_root_stops_before_the_folders_are_read() {
    let (_tmp, t) = private_root();
    let a = dir(&t.join("a"), 0o775);
    for root in [
        PathBuf::from("relative/work"),
        a.join("..").join("a").join("work"),
    ] {
        let text = message(prepare_work_root(&root));
        assert!(
            !text.contains("group-writable"),
            "{} must be stopped by its shape, before any folder is read:\n{text}",
            root.display()
        );
    }
}

// ----------------------------------------- equivalence with the pinned runner

/// The rule here and the runner's `Directory::open` give the same answer for
/// each mode of the folder above the root. A pin bump that changes either
/// rule fails here.
#[test]
fn the_ancestor_rule_matches_the_pinned_runner_for_each_mode() {
    let (_tmp, t) = private_root();
    for mode in [
        0o700, 0o750, 0o755, 0o775, 0o757, 0o777, 0o1777, 0o1775, 0o2775,
    ] {
        let a = dir(&t.join(format!("a{mode:o}")), 0o700);
        let root = dir(&a.join("work"), 0o700);
        set_mode(&a, mode);
        let runner = Directory::open(&root, false).is_ok();
        assert_eq!(
            path_faults(&a).is_empty(),
            runner,
            "mode {mode:04o}: the runner {} the folder",
            if runner { "accepts" } else { "stops" }
        );
        assert_eq!(
            prepare_work_root(&root).is_ok(),
            runner,
            "mode {mode:04o}: prepare_work_root must agree with the runner"
        );
    }
}

#[test]
fn links_files_and_dotted_paths_match_the_pinned_runner() {
    let (_tmp, t) = private_root();
    let real = dir(&t.join("real"), 0o700);
    dir(&real.join("work"), 0o700);
    let link = t.join("link");
    symlink(&real, &link).unwrap();
    let file = t.join("file");
    fs::write(&file, b"file").unwrap();
    for root in [
        link.join("work"),
        file.join("work"),
        real.join("..").join("real").join("work"),
    ] {
        assert!(
            Directory::open(&root, false).is_err(),
            "fixture: the runner must stop {}",
            root.display()
        );
        assert!(
            prepare_work_root(&root).is_err(),
            "{} is stopped by the runner, so the check must name it",
            root.display()
        );
    }
}

/// `runs/` is held to the runner's private rule, the stricter of the two the
/// runner applies to it.
#[test]
fn the_runs_rule_matches_the_pinned_runner() {
    let (_tmp, t) = private_root();
    for mode in [0o700, 0o755, 0o775] {
        let root = dir(&t.join(format!("w{mode:o}")), 0o700);
        dir(&root.join("runs"), mode);
        let runner = Directory::open(&root, false)
            .and_then(|d| d.child("runs", false))
            .is_ok();
        assert_eq!(
            prepare_work_root(&root).is_ok(),
            runner,
            "runs/ at {mode:04o}: the runner {}",
            if runner { "accepts it" } else { "stops" }
        );
    }
}

// A folder above the root that is 1777 and owned by another user (as `/tmp`
// is owned by root) passes both rules: neither checks the owners of folders
// above the root. A test cannot make such a folder without changing an
// owner, which no test here does; the limit is documented in
// docs/reference/recipe-command.md#limits.

// ----------------------------------------------------- the runner's reason

/// When the runner's own check fails after this one passed, the message names
/// the work root and ends with the runner's reason on the same line: the
/// top-level `Error: {error}` prints only the outermost message.
#[test]
fn a_runner_failure_ends_the_message_with_the_runners_reason() {
    if running_as_root() {
        eprintln!("skipped: root can create a folder under a 0555 folder");
        return;
    }
    let (_tmp, t) = private_root();
    let home = dir(&t.join("home"), 0o700);
    // No write bit for anyone, so the rule passes, but the runner cannot
    // create the work root below it.
    let locked = dir(&t.join("locked"), 0o700);
    let _restore = Restore(locked.clone());
    set_mode(&locked, 0o555);
    let root = locked.join("work");

    let _lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = crate::test_support::env_paths(&[
        ("HOME", Some(home.as_path())),
        ("AMPLIHACK_WORK_ROOT", Some(root.as_path())),
        ("AMPLIHACK_WORK_ROOT_EXPLICIT", None),
    ]);
    let error = match super::super::run_dir::RunWorkspace::begin_durable("issue-2346", &t) {
        Ok(_) => panic!("the runner cannot create {}", root.display()),
        Err(error) => error,
    };
    let top = error.to_string();
    let prefix = format!(
        "cannot open the recovery store at work root {}: ",
        root.display()
    );
    assert!(
        top.starts_with(&prefix),
        "expected the top-level message to start {prefix:?}, got {top:?}"
    );
    assert!(
        top.contains("Permission denied"),
        "the runner's reason must be on the same line: {top:?}"
    );
    assert!(!root.exists());
}
