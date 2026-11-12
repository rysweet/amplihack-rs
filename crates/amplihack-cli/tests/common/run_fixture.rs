//! Private filesystem fixture for the owned-runtime cleanup contract.
//!
//! Allocation requires a caller-supplied private TMPDIR. There is deliberately
//! no fallback to /tmp. Unit callers install `environment()` under the crate's
//! shared environment lock; integration callers apply it to their commands.

#![allow(dead_code)] // Shared by unit and integration targets with different needs.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) struct RunFixture {
    _owner: tempfile::TempDir,
    pub base: PathBuf,
    pub work: PathBuf,
    pub tmp: PathBuf,
    pub parent_runtime: PathBuf,
    pub external: PathBuf,
    pub home: PathBuf,
    pub evidence: PathBuf,
    pub proc_root: PathBuf,
}

impl RunFixture {
    pub fn new() -> Self {
        let parent = std::env::var_os("TMPDIR")
            .expect("safety hold: invoke tests with TMPDIR inside the fix run's private directory");
        let parent = fs::canonicalize(parent).expect("private test TMPDIR must exist");
        assert!(parent != Path::new("/tmp") && parent != Path::new("/var/tmp"));
        let owner = tempfile::Builder::new()
            .prefix("f")
            .rand_bytes(3)
            .tempdir_in(parent)
            .unwrap();
        let base = owner.path().canonicalize().unwrap();
        let fx = Self {
            work: base.join("w"),
            parent_runtime: base.join("p"),
            tmp: base.join("p/t"),
            external: base.join("r"),
            home: base.join("p/t/h"),
            evidence: base.join("e"),
            proc_root: base.join("proc"),
            base,
            _owner: owner,
        };
        for dir in [
            &fx.work,
            &fx.tmp,
            &fx.external,
            &fx.home,
            &fx.evidence,
            &fx.proc_root,
        ] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fx
    }

    pub fn environment(&self) -> Vec<(&'static str, OsString)> {
        vec![
            ("TMPDIR", self.tmp.clone().into_os_string()),
            ("TMP", self.tmp.clone().into_os_string()),
            ("TEMP", self.tmp.clone().into_os_string()),
            (
                "AMPLIHACK_WORKFLOW_RUNTIME_DIR",
                self.parent_runtime.clone().into_os_string(),
            ),
            ("AMPLIHACK_WORK_ROOT", self.work.clone().into_os_string()),
            ("HOME", self.home.clone().into_os_string()),
            ("XDG_CACHE_HOME", self.home.join(".cache").into_os_string()),
            ("CODEX_HOME", self.home.join(".codex").into_os_string()),
            (
                "AMPLIHACK_TEST_PROC_ROOT",
                self.proc_root.clone().into_os_string(),
            ),
            ("AMPLIHACK_CLEANUP_ON_EXIT", "0".into()),
            ("AMPLIHACK_RUN_TMP_ROOT", "".into()),
        ]
    }

    pub fn sentinel(&self, dir: &Path) -> PathBuf {
        assert!(
            dir.starts_with(&self.base),
            "all destructive cases stay in this fixture"
        );
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("sentinel");
        fs::write(&path, b"must survive\n").unwrap();
        path
    }

    pub fn assert_sentinel(&self, path: &Path) {
        assert_eq!(
            fs::read(path).unwrap_or_else(|e| panic!("{} did not survive: {e}", path.display())),
            b"must survive\n"
        );
    }
}
