//! Stable external aliases exist before transactions; selected leaves stay regular.
use std::{fs, path::PathBuf};

pub(super) struct AliasPaths {
    pub dir: tempfile::TempDir,
    pub home: PathBuf,
    pub codex_home: PathBuf,
    pub root: PathBuf,
    pub alias: PathBuf,
    pub real: PathBuf,
    pub explicit: bool,
}

impl AliasPaths {
    pub fn new(explicit: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(real.join("user")).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let home = if explicit {
            let home = dir.path().join("ordinary-home");
            fs::create_dir(&home).unwrap();
            home
        } else {
            alias.join("user")
        };
        let codex_home = if explicit {
            alias.join("user/codex")
        } else {
            home.join(".codex")
        };
        fs::create_dir(&codex_home).unwrap();
        for path in [&home, &codex_home, codex_home.parent().unwrap()] {
            let meta = fs::symlink_metadata(path).unwrap();
            assert!(meta.is_dir() && !meta.file_type().is_symlink());
        }
        let root = home.join(".amplihack/codex");
        Self {
            dir,
            home,
            codex_home,
            root,
            alias,
            real,
            explicit,
        }
    }

    pub fn selected_home(&self) -> &str {
        if self.explicit {
            self.codex_home.to_str().unwrap()
        } else {
            ""
        }
    }

    pub fn assert_alias_unchanged(&self) {
        assert_eq!(fs::read_link(&self.alias).unwrap(), self.real);
    }
}
