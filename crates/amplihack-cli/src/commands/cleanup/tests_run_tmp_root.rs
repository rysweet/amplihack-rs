//! Unit tests for `run_tmp_root.rs` (issue #2291, D2 to D7, D12, S1 to S7,
//! S17). The specification is "Where a run's temp folder goes" in
//! `docs/howto/clean-up-after-recipe-runs.md`.
//!
//! Wire with, at the end of `run_tmp_root.rs`:
//! `#[cfg(test)] #[path = "tests_run_tmp_root.rs"] mod tests;`
//!
//! Contract (nothing here reads the process environment):
//! - `pub(crate) const RUN_TMP_ROOT_ENV: &str = "AMPLIHACK_RUN_TMP_ROOT";`
//! - `pub(crate) const RUN_DIR_PREFIX: &str = "ahr-";`, so the part after the
//!   root, `/ahr-<12 hex>/tmp`, is 21 bytes and a root of up to 59 bytes fits;
//! - `pub(crate) struct TmpRootInputs { pub run_tmp_root: Option<OsString>,
//!   pub tmpdir: Option<OsString>, pub parent_runtime_dir: Option<OsString>,
//!   pub home: Option<PathBuf>, pub euid: u32 }`, with `Default` and
//!   `from_env()`. `parent_runtime_dir` is the caller's
//!   `AMPLIHACK_WORKFLOW_RUNTIME_DIR`; `euid` is the uid the root and the
//!   per-run folder must belong to (root may also be owned by uid 0);
//! - `pub(crate) enum RuntimeChoice { Default(PathBuf),
//!   External { dir: PathBuf, root: PathBuf } }`;
//! - `pub(crate) struct Chosen { pub choice: RuntimeChoice,
//!   pub warning: Option<String> }`;
//! - `pub(crate) fn choose_runtime_dir(work_root: &Path, run_id: &str,
//!   inputs: &TmpRootInputs) -> Chosen`. `Default` holds exactly
//!   `runtime_dir_for(work_root, run_id)`. `External` holds the canonical
//!   root and `<root>/ahr-<short_id>`, which the call has created
//!   (non-recursive, mode 0700); `tmp/` and `artifacts/` are left to the
//!   caller;
//! - `pub(crate) fn external_dir_name(run_id: &str) -> Option<String>`:
//!   `Some("ahr-<12 hex>")`, `None` when the short id is shorter;
//! - `pub(crate) fn usable_explicit_root(work_root: &Path,
//!   inputs: &TmpRootInputs) -> Result<Option<PathBuf>, String>`: legacy
//!   mode only; `Ok(None)` when the variable is unset or empty,
//!   `Ok(Some(canonical root))` when rule 1 accepts it, `Err(warning)`
//!   otherwise.
//!
//! Warnings are one line: `amplihack: <VAR>=<{:?} path> <reason>; using the
//! default run folder`.

use super::*;
use crate::commands::cleanup::run_dir::runtime_dir_for;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const ID: &str = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
const ID2: &str = "9b1d27e4-0c3a-4f6e-8d21-5a7c9e0b4f12";
const SHORT: &str = "3f0c9a526d1e";
const TAIL: &str = "; using the default run folder";

fn me() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// A canonical temp folder holding `home/`, `work/` (mode 0700) and room
/// for roots. Its path is kept short so the 80-byte rule does not fire by
/// accident; tests that need an exact length build it themselves.
struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    work: PathBuf,
    home: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = short_tempdir();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let work = base.join("work");
        let home = base.join("home");
        for d in [&work, &home] {
            fs::create_dir_all(d).unwrap();
            fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            _tmp: tmp,
            base,
            work,
            home,
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let p = self.base.join(name);
        fs::create_dir_all(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }

    fn inputs(&self) -> TmpRootInputs {
        TmpRootInputs {
            home: Some(self.home.clone()),
            euid: me(),
            ..TmpRootInputs::default()
        }
    }

    fn with_root(&self, root: impl Into<OsString>) -> TmpRootInputs {
        TmpRootInputs {
            run_tmp_root: Some(root.into()),
            ..self.inputs()
        }
    }

    fn with_tmpdir(&self, tmpdir: impl Into<OsString>) -> TmpRootInputs {
        TmpRootInputs {
            tmpdir: Some(tmpdir.into()),
            ..self.inputs()
        }
    }

    fn choose(&self, inputs: &TmpRootInputs) -> Chosen {
        choose_runtime_dir(&self.work, ID, inputs)
    }

    fn default_dir(&self) -> PathBuf {
        runtime_dir_for(&self.work, ID)
    }
}

/// A temp folder whose canonical path leaves room for a 56-byte root.
fn short_tempdir() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    if fs::canonicalize(t.path()).unwrap().as_os_str().len() <= 30 {
        return t;
    }
    let t = tempfile::tempdir_in("/tmp").unwrap();
    assert!(
        fs::canonicalize(t.path()).unwrap().as_os_str().len() <= 30,
        "no temp folder short enough for the length tests"
    );
    t
}

fn external(chosen: &Chosen) -> (&Path, &Path) {
    match &chosen.choice {
        RuntimeChoice::External { dir, root } => (dir.as_path(), root.as_path()),
        RuntimeChoice::Default(d) => panic!("expected External, got Default({d:?})"),
    }
}

fn default_of(chosen: &Chosen) -> &Path {
    match &chosen.choice {
        RuntimeChoice::Default(d) => d.as_path(),
        RuntimeChoice::External { dir, .. } => panic!("expected Default, got External({dir:?})"),
    }
}

fn warning(chosen: &Chosen) -> &str {
    chosen
        .warning
        .as_deref()
        .unwrap_or_else(|| panic!("expected one warning, got none: {:?}", chosen.choice))
}

fn assert_warns(chosen: &Chosen, var: &str, reason: &str) {
    let w = warning(chosen);
    assert!(
        w.starts_with(&format!("amplihack: {var}=\"")),
        "warning must name {var}: {w}"
    );
    assert!(w.contains(reason), "warning must give {reason:?}: {w}");
    assert!(w.ends_with(TAIL), "{w}");
    assert!(!w.contains('\n'), "one line only: {w:?}");
}

// ----- names ------------------------------------------------------------

#[test]
fn constants_name_the_variable_and_prefix() {
    assert_eq!(RUN_TMP_ROOT_ENV, "AMPLIHACK_RUN_TMP_ROOT");
    assert_eq!(RUN_DIR_PREFIX, "ahr-");
}

/// The per-run name is short so the macOS default `TMPDIR` (56 bytes once
/// resolved) fits the 80-byte limit with room to spare.
#[test]
fn suffix_is_21_bytes_and_the_root_budget_59() {
    assert_eq!(RUN_SUFFIX_BYTES, "/ahr-3f0c9a526d1e/tmp".len());
    assert_eq!(RUN_SUFFIX_BYTES, 21);
    assert_eq!(MAX_ROOT_BYTES, 59);
}

#[test]
fn external_dir_name_uses_the_12_hex_short_id() {
    assert_eq!(external_dir_name(ID).as_deref(), Some("ahr-3f0c9a526d1e"));
}

#[test]
fn external_dir_name_refuses_a_short_run_id() {
    assert_eq!(external_dir_name("abc"), None);
    assert_eq!(external_dir_name("0123-4567-89a"), None);
    assert_eq!(external_dir_name(""), None);
}

#[test]
fn different_run_ids_get_different_folders_under_one_root() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let inputs = fx.with_root(&root);
    let a = choose_runtime_dir(&fx.work, ID, &inputs);
    let b = choose_runtime_dir(&fx.work, ID2, &inputs);
    let (da, _) = external(&a);
    let (db, _) = external(&b);
    assert_ne!(da, db);
    assert!(da.is_dir() && db.is_dir());
    assert_eq!(da.parent(), db.parent());
}

// ----- precedence -------------------------------------------------------

/// AC3: neither variable applies, so the layout is exactly the default one
/// and nothing is printed.
#[test]
fn neither_variable_set_uses_the_default_without_warning() {
    let fx = Fx::new();
    let chosen = fx.choose(&fx.inputs());
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_eq!(chosen.warning, None);
}

/// AC3: a `TMPDIR` inside the work root is the normal nested case.
#[test]
fn tmpdir_inside_the_work_root_uses_the_default_without_warning() {
    let fx = Fx::new();
    let inside = fx.work.join("runs/x/runtime/tmp");
    fs::create_dir_all(&inside).unwrap();
    let chosen = fx.choose(&fx.with_tmpdir(&inside));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_eq!(chosen.warning, None);
    assert!(
        !inside.join(format!("ahr-{SHORT}")).exists(),
        "nothing is created inside the work root"
    );
}

/// AC1: a suitable caller `TMPDIR` is honoured.
#[test]
fn caller_tmpdir_is_used_when_the_root_variable_is_unset() {
    let fx = Fx::new();
    let caller = fx.dir("callertmp");
    let chosen = fx.choose(&fx.with_tmpdir(&caller));
    let (dir, root) = external(&chosen);
    assert_eq!(root, caller);
    assert_eq!(dir, caller.join(format!("ahr-{SHORT}")));
    assert!(dir.is_dir());
    assert_eq!(chosen.warning, None);
}

#[test]
fn empty_root_variable_counts_as_unset() {
    let fx = Fx::new();
    let caller = fx.dir("callertmp");
    let inputs = TmpRootInputs {
        run_tmp_root: Some(OsString::new()),
        tmpdir: Some(caller.clone().into()),
        ..fx.inputs()
    };
    let chosen = fx.choose(&inputs);
    assert_eq!(external(&chosen).1, caller);
    assert_eq!(chosen.warning, None);
}

/// AC2: `AMPLIHACK_RUN_TMP_ROOT` wins over `TMPDIR`.
#[test]
fn root_variable_wins_over_tmpdir() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let caller = fx.dir("callertmp");
    let inputs = TmpRootInputs {
        run_tmp_root: Some(root.clone().into()),
        tmpdir: Some(caller.clone().into()),
        ..fx.inputs()
    };
    let chosen = fx.choose(&inputs);
    let (dir, got_root) = external(&chosen);
    assert_eq!(got_root, root);
    assert_eq!(dir, root.join(format!("ahr-{SHORT}")));
    assert!(
        !caller.join(format!("ahr-{SHORT}")).exists(),
        "TMPDIR is not used"
    );
    assert_eq!(chosen.warning, None);
}

/// D3 rule 1: a bad root variable never falls through to `TMPDIR`.
#[test]
fn bad_root_variable_does_not_fall_through_to_tmpdir() {
    let fx = Fx::new();
    let caller = fx.dir("callertmp");
    let inputs = TmpRootInputs {
        run_tmp_root: Some("tmp/runs".into()),
        tmpdir: Some(caller.clone().into()),
        ..fx.inputs()
    };
    let chosen = fx.choose(&inputs);
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is not an absolute path");
    assert!(!caller.join(format!("ahr-{SHORT}")).exists());
}

// ----- rule 1: AMPLIHACK_RUN_TMP_ROOT -----------------------------------

#[test]
fn missing_root_is_created_private_with_its_parents() {
    let fx = Fx::new();
    let root = fx.base.join("new/runroot");
    let chosen = fx.choose(&fx.with_root(&root));
    let (dir, got_root) = external(&chosen);
    assert_eq!(got_root, root);
    assert!(dir.is_dir());
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(chosen.warning, None);
}

#[test]
fn relative_root_warns_and_falls_back() {
    let fx = Fx::new();
    let chosen = fx.choose(&fx.with_root("tmp/runs"));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is not an absolute path");
}

#[test]
fn root_that_is_a_file_warns_and_falls_back() {
    let fx = Fx::new();
    let file = fx.base.join("afile");
    fs::write(&file, "x").unwrap();
    let chosen = fx.choose(&fx.with_root(&file));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is not a folder");
}

#[test]
fn root_that_cannot_be_created_warns_and_falls_back() {
    let fx = Fx::new();
    let file = fx.base.join("afile");
    fs::write(&file, "x").unwrap();
    let chosen = fx.choose(&fx.with_root(file.join("below")));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "could not be created");
}

#[test]
fn root_equal_to_the_work_root_is_refused() {
    let fx = Fx::new();
    let chosen = fx.choose(&fx.with_root(&fx.work));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is inside the work root");
}

#[test]
fn root_inside_the_work_root_is_refused() {
    let fx = Fx::new();
    let inside = fx.work.join("tmp");
    fs::create_dir_all(&inside).unwrap();
    let chosen = fx.choose(&fx.with_root(&inside));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is inside the work root");
    assert!(!inside.join(format!("ahr-{SHORT}")).exists());
}

/// S6: the comparison is by resolved path, so a link into the work root is
/// still inside it.
#[test]
fn root_linking_into_the_work_root_is_refused() {
    let fx = Fx::new();
    let inside = fx.work.join("tmp");
    fs::create_dir_all(&inside).unwrap();
    let link = fx.base.join("lnk");
    symlink(&inside, &link).unwrap();
    let chosen = fx.choose(&fx.with_root(&link));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is inside the work root");
}

/// S7: whole path components, so `<base>/work2` is outside `<base>/work`.
#[test]
fn sibling_with_the_work_root_as_a_prefix_is_outside_it() {
    let fx = Fx::new();
    let sibling = fx.dir("work2");
    let chosen = fx.choose(&fx.with_root(&sibling));
    assert_eq!(external(&chosen).1, sibling);
    assert_eq!(chosen.warning, None);
}

#[test]
fn root_inside_home_amplihack_is_refused() {
    let fx = Fx::new();
    let inside = fx.home.join(".amplihack/scratch");
    fs::create_dir_all(&inside).unwrap();
    let chosen = fx.choose(&fx.with_root(&inside));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "AMPLIHACK_RUN_TMP_ROOT",
        "is inside $HOME/.amplihack",
    );
}

/// The resolved root is what is returned, not the path as given.
#[test]
fn root_given_through_a_link_is_returned_resolved() {
    let fx = Fx::new();
    let real = fx.dir("realroot");
    let link = fx.base.join("rootlink");
    symlink(&real, &link).unwrap();
    let chosen = fx.choose(&fx.with_root(&link));
    let (dir, root) = external(&chosen);
    assert_eq!(root, real);
    assert!(dir.starts_with(&real));
}

/// S3: a world-writable root without the sticky bit is refused.
#[test]
fn world_writable_root_without_sticky_bit_is_refused() {
    let fx = Fx::new();
    let root = fx.dir("openroot");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
    let chosen = fx.choose(&fx.with_root(&root));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "AMPLIHACK_RUN_TMP_ROOT",
        "is writable by other users without the sticky bit",
    );
    assert!(!root.join(format!("ahr-{SHORT}")).exists());
}

#[test]
fn group_writable_root_without_sticky_bit_is_refused() {
    let fx = Fx::new();
    let root = fx.dir("grouproot");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).unwrap();
    let chosen = fx.choose(&fx.with_root(&root));
    assert_warns(
        &chosen,
        "AMPLIHACK_RUN_TMP_ROOT",
        "is writable by other users without the sticky bit",
    );
}

/// S3: a sticky world-writable root, like `/tmp`, is accepted.
#[test]
fn sticky_world_writable_root_is_accepted() {
    let fx = Fx::new();
    let root = fx.dir("stickyroot");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o1777)).unwrap();
    let chosen = fx.choose(&fx.with_root(&root));
    let (dir, _) = external(&chosen);
    assert_eq!(chosen.warning, None);
    assert_eq!(
        fs::metadata(dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

/// S3: the owner is checked against the uid given in the inputs.
#[test]
fn root_owned_by_another_user_is_refused() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let inputs = TmpRootInputs {
        euid: me().wrapping_add(1).max(1),
        ..fx.with_root(&root)
    };
    let chosen = fx.choose(&inputs);
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "AMPLIHACK_RUN_TMP_ROOT",
        "is owned by another user",
    );
}

#[test]
fn non_utf8_root_is_refused() {
    let fx = Fx::new();
    let root = fx.base.join(OsStr::from_bytes(b"bad\xffroot"));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let chosen = fx.choose(&fx.with_root(&root));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "is not valid UTF-8");
}

// ----- length -----------------------------------------------------------

/// A folder under `fx.base` whose canonical path is exactly `len` bytes.
fn root_of_len(fx: &Fx, len: usize) -> PathBuf {
    let used = fx.base.as_os_str().len() + 1;
    assert!(len > used, "temp base too long for a {len}-byte root");
    let p = fx.dir(&"r".repeat(len - used));
    assert_eq!(p.as_os_str().len(), len);
    p
}

/// D5: `/ahr-<12 hex>/tmp` adds 21 bytes, so 59 fits.
#[test]
fn root_of_59_bytes_fits_the_80_byte_limit() {
    let fx = Fx::new();
    let root = root_of_len(&fx, 59);
    let chosen = fx.choose(&fx.with_root(&root));
    let (dir, _) = external(&chosen);
    assert_eq!(dir.join("tmp").as_os_str().len(), 80);
    assert_eq!(chosen.warning, None);
}

#[test]
fn root_of_60_bytes_is_refused_with_the_length() {
    let fx = Fx::new();
    let root = root_of_len(&fx, 60);
    let chosen = fx.choose(&fx.with_root(&root));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "AMPLIHACK_RUN_TMP_ROOT",
        "would make the run temp path 81 bytes (limit 80)",
    );
    assert!(!root.join(format!("ahr-{SHORT}")).exists());
}

/// The macOS default `TMPDIR`, `/var/folders/<2>/<30>/T/`, is given through
/// a link (`/var` is `/private/var`) and with a trailing slash, and resolves
/// to 56 bytes. It is honoured with no warning: the run temp path is 77
/// bytes. Before the per-run name was shortened this printed a warning on
/// every macOS run.
#[test]
fn macos_shaped_tmpdir_of_56_resolved_bytes_is_used_without_a_warning() {
    let fx = Fx::new();
    // `<base>/<pad>/T`, 56 bytes, reached as `<base>/var/T/`.
    let pad = 56 - fx.base.as_os_str().len() - 1 - "/T".len();
    let private_var = fx.dir(&"p".repeat(pad));
    let resolved = private_var.join("T");
    fs::create_dir(&resolved).unwrap();
    fs::set_permissions(&resolved, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(resolved.as_os_str().len(), 56);
    symlink(&private_var, fx.base.join("var")).unwrap();
    let mut given = fx.base.join("var/T").into_os_string();
    given.push("/");

    let chosen = fx.choose(&fx.with_tmpdir(&given));
    assert_eq!(chosen.warning, None);
    let (dir, root) = external(&chosen);
    assert_eq!(root, resolved);
    assert_eq!(dir, resolved.join(format!("ahr-{SHORT}")));
    assert_eq!(dir.join("tmp").as_os_str().len(), 77);
}

/// A caller's `TMPDIR` past the 59-byte budget is refused with a warning
/// naming `TMPDIR`, not skipped silently: a silent skip would put the run
/// back on the work root's disk without a word (#2291). One byte less fits.
#[test]
fn tmpdir_past_the_59_byte_budget_warns_naming_tmpdir() {
    let fx = Fx::new();
    let fits = root_of_len(&fx, 59);
    let chosen = fx.choose(&fx.with_tmpdir(&fits));
    assert_eq!(external(&chosen).1, fits);
    assert_eq!(chosen.warning, None);

    let fx = Fx::new();
    let long = root_of_len(&fx, 60);
    let chosen = fx.choose(&fx.with_tmpdir(&long));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "TMPDIR",
        "would make the run temp path 81 bytes (limit 80)",
    );
    assert!(!long.join(format!("ahr-{SHORT}")).exists());
}

/// The length is measured on the resolved root: a short `TMPDIR` that links
/// to a 60-byte folder is refused, with the warning naming the value given.
#[test]
fn tmpdir_resolving_past_the_budget_warns_with_the_given_value() {
    let fx = Fx::new();
    let resolved = root_of_len(&fx, 60);
    let given = fx.base.join("v");
    symlink(&resolved, &given).unwrap();

    let chosen = fx.choose(&fx.with_tmpdir(&given));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "TMPDIR",
        "would make the run temp path 81 bytes (limit 80)",
    );
    assert!(
        warning(&chosen).starts_with(&format!("amplihack: TMPDIR={given:?} ")),
        "{}",
        warning(&chosen)
    );
    assert!(!resolved.join(format!("ahr-{SHORT}")).exists());
}

// ----- rule 2: TMPDIR ---------------------------------------------------

#[test]
fn unusable_tmpdir_is_skipped_silently() {
    let fx = Fx::new();
    let file = fx.base.join("afile");
    fs::write(&file, "x").unwrap();
    let in_home = fx.home.join(".amplihack/tmp");
    fs::create_dir_all(&in_home).unwrap();
    let cases: Vec<OsString> = vec![
        OsString::new(),
        "relative/tmp".into(),
        fx.base.join("missing").into(),
        file.into(),
        in_home.into(),
    ];
    for tmpdir in cases {
        let chosen = fx.choose(&fx.with_tmpdir(&tmpdir));
        assert_eq!(default_of(&chosen), fx.default_dir(), "{tmpdir:?}");
        assert_eq!(chosen.warning, None, "{tmpdir:?}");
    }
    assert!(!fx.base.join("missing").exists(), "TMPDIR is never created");
}

/// The nested case: a sub-recipe's `TMPDIR` lies inside the parent's
/// `AMPLIHACK_WORKFLOW_RUNTIME_DIR` (for example a legacy-mode parent in the
/// system temp folder), so rule 2 is skipped without a warning.
#[test]
fn tmpdir_inside_the_parent_runtime_dir_is_skipped_silently() {
    let fx = Fx::new();
    let parent = fx.dir("amplihack-workflow-abc123");
    let parent_tmp = parent.join("tmp");
    fs::create_dir_all(&parent_tmp).unwrap();
    for parent_var in [parent.clone().into_os_string(), {
        let mut s = parent.clone().into_os_string();
        s.push("/");
        s
    }] {
        let inputs = TmpRootInputs {
            tmpdir: Some(parent_tmp.clone().into()),
            parent_runtime_dir: Some(parent_var.clone()),
            ..fx.inputs()
        };
        let chosen = fx.choose(&inputs);
        assert_eq!(default_of(&chosen), fx.default_dir(), "{parent_var:?}");
        assert_eq!(chosen.warning, None);
    }
    assert!(!parent_tmp.join(format!("ahr-{SHORT}")).exists());
}

/// A parent runtime dir that merely shares a prefix does not count.
#[test]
fn tmpdir_beside_the_parent_runtime_dir_is_used() {
    let fx = Fx::new();
    let parent = fx.dir("parent");
    let caller = fx.dir("parent2");
    let inputs = TmpRootInputs {
        tmpdir: Some(caller.clone().into()),
        parent_runtime_dir: Some(parent.into()),
        ..fx.inputs()
    };
    assert_eq!(external(&fx.choose(&inputs)).1, caller);
}

#[test]
fn tmpdir_without_sticky_bit_warns_naming_tmpdir() {
    let fx = Fx::new();
    let caller = fx.dir("callertmp");
    fs::set_permissions(&caller, fs::Permissions::from_mode(0o777)).unwrap();
    let chosen = fx.choose(&fx.with_tmpdir(&caller));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(
        &chosen,
        "TMPDIR",
        "is writable by other users without the sticky bit",
    );
}

#[test]
fn tmpdir_owned_by_another_user_warns_naming_tmpdir() {
    let fx = Fx::new();
    let caller = fx.dir("callertmp");
    let inputs = TmpRootInputs {
        euid: me().wrapping_add(1).max(1),
        ..fx.with_tmpdir(&caller)
    };
    let chosen = fx.choose(&inputs);
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "TMPDIR", "is owned by another user");
}

// ----- the per-run folder (D4, S1, S2, S5) ------------------------------

#[test]
fn per_run_folder_is_private_and_owned_by_the_user() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let chosen = fx.choose(&fx.with_root(&root));
    let (dir, _) = external(&chosen);
    let meta = fs::symlink_metadata(dir).unwrap();
    assert!(meta.file_type().is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    assert_eq!(meta.uid(), me());
    assert_eq!(dir.parent(), Some(root.as_path()), "directly in the root");
}

#[test]
fn existing_per_run_folder_is_never_reused() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let planted = root.join(format!("ahr-{SHORT}"));
    fs::create_dir(&planted).unwrap();
    fs::write(planted.join("planted"), "x").unwrap();
    let chosen = fx.choose(&fx.with_root(&root));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "already exists");
    assert!(planted.join("planted").exists(), "the folder is left alone");
}

#[test]
fn planted_symlink_at_the_per_run_name_is_refused() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let elsewhere = fx.dir("elsewhere");
    let planted = root.join(format!("ahr-{SHORT}"));
    symlink(&elsewhere, &planted).unwrap();
    let chosen = fx.choose(&fx.with_root(&root));
    assert_eq!(default_of(&chosen), fx.default_dir());
    let w = warning(&chosen);
    assert!(w.contains("per-run folder \""), "{w}");
    assert!(w.contains("already exists"), "{w}");
    assert!(
        fs::symlink_metadata(&planted)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn short_run_id_uses_the_default_layout() {
    let fx = Fx::new();
    let root = fx.dir("runroot");
    let chosen = choose_runtime_dir(&fx.work, "abc", &fx.with_root(&root));
    assert_eq!(default_of(&chosen), runtime_dir_for(&fx.work, "abc"));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

// ----- output safety (S17) ----------------------------------------------

#[test]
fn control_characters_in_the_path_cannot_forge_lines() {
    let fx = Fx::new();
    let chosen = fx.choose(&fx.with_root("evil\namplihack: ok\x1b[31m"));
    let w = warning(&chosen);
    assert!(!w.contains('\n'), "{w:?}");
    assert!(!w.contains('\x1b'), "{w:?}");
    assert!(w.contains("\\n"), "the newline is escaped: {w:?}");
    assert!(w.contains("is not an absolute path"), "{w:?}");
}

#[test]
fn root_that_cannot_be_created_does_not_repeat_the_raw_path() {
    let fx = Fx::new();
    let file = fx.base.join("plain");
    fs::write(&file, "x").unwrap();
    // Absolute, so creation is reached; it fails with ENOTDIR even as root.
    let chosen = fx.choose(&fx.with_root(file.join("evil\namplihack: forged\x1b[31m")));
    assert_eq!(default_of(&chosen), fx.default_dir());
    assert_warns(&chosen, "AMPLIHACK_RUN_TMP_ROOT", "could not be created");
    let w = warning(&chosen);
    assert!(!w.contains('\x1b'), "{w:?}");
    assert!(w.contains("\\n"), "the newline is escaped: {w:?}");
    assert_eq!(
        w.matches("amplihack: forged").count(),
        1,
        "the path appears once, escaped: {w:?}"
    );
}

// ----- legacy mode (D12) ------------------------------------------------

#[test]
fn explicit_root_for_legacy_mode_is_none_when_unset_or_empty() {
    let fx = Fx::new();
    assert_eq!(usable_explicit_root(&fx.work, &fx.inputs()), Ok(None));
    assert_eq!(
        usable_explicit_root(&fx.work, &fx.with_root(OsString::new())),
        Ok(None)
    );
}

#[test]
fn explicit_root_for_legacy_mode_ignores_tmpdir() {
    let fx = Fx::new();
    let caller = fx.dir("callertmp");
    assert_eq!(
        usable_explicit_root(&fx.work, &fx.with_tmpdir(&caller)),
        Ok(None)
    );
}

#[test]
fn explicit_root_for_legacy_mode_is_resolved_when_usable() {
    let fx = Fx::new();
    let real = fx.dir("realroot");
    let link = fx.base.join("rootlink");
    symlink(&real, &link).unwrap();
    assert_eq!(
        usable_explicit_root(&fx.work, &fx.with_root(&link)),
        Ok(Some(real))
    );
}

#[test]
fn explicit_root_for_legacy_mode_refuses_a_bad_root_with_a_warning() {
    let fx = Fx::new();
    let err = usable_explicit_root(&fx.work, &fx.with_root("tmp/runs")).unwrap_err();
    assert!(
        err.starts_with("amplihack: AMPLIHACK_RUN_TMP_ROOT=\""),
        "{err}"
    );
    assert!(err.contains("is not an absolute path"), "{err}");
}
