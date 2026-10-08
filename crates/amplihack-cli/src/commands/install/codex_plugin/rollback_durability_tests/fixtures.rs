use super::*;
use std::os::unix::fs::PermissionsExt;

pub(super) struct Rollback {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub binary: PathBuf,
    pub pending: Value,
}

impl Rollback {
    pub fn new(had_package: bool, original_files: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ancestors = dir.path().join("published/transaction");
        fs::create_dir_all(&ancestors).unwrap();
        let (root, home, mut pending) = recovery_tests::interrupted(&ancestors, had_package);
        if had_package {
            let backup = root.join("previous-package");
            fs::create_dir(backup.join("nested")).unwrap();
            fs::write(backup.join("nested/resource"), b"nested original").unwrap();
            pending["ledger"]["package_digest"] = json!(digest(&backup).unwrap());
            let bytes = json_bytes(&pending["ledger"]).unwrap();
            pending["snapshots"]["ledger"] = json!(bytes);
            pending["expected"]["ledger"] = pending["snapshots"]["ledger"].clone();
            fs::write(root.join("ownership.json"), &bytes).unwrap();
        }
        if original_files {
            let config = b"# original config\napproval_policy = 'on-request'\n";
            pending["config"] = json!(config.to_vec());
            pending["expected"]["config"] = pending["config"].clone();
            fs::write(home.join("config.toml"), config).unwrap();
            pending["snapshots"]["hooks"] = json!(b"\n{\"hooks\":{}}\n\n".to_vec());
            pending["snapshots"]["marketplace"] = json!(b"\n{\"plugins\":[]}\n\n".to_vec());
        }
        fs::write(root.join("pending.json"), json_bytes(&pending).unwrap()).unwrap();
        let binary = dir.path().join("native");
        fs::write(&binary, "#!/bin/sh\nprintf '{\"installed\":[]}'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            dir,
            root,
            home,
            binary,
            pending,
        }
    }

    pub fn recover(&self) -> Result<()> {
        recover_install(&self.root, &self.binary, &self.home)
    }

    pub fn snapshots(&self) -> [(&str, PathBuf); 4] {
        [
            ("config", self.home.join("config.toml")),
            ("hooks", self.home.join("hooks.json")),
            (
                "marketplace",
                self.root.join("market/.agents/plugins/marketplace.json"),
            ),
            ("ledger", self.root.join("ownership.json")),
        ]
    }

    pub fn assert_original(&self) {
        for (key, path) in self.snapshots() {
            let original = if key == "config" {
                &self.pending["config"]
            } else {
                &self.pending["snapshots"][key]
            };
            assert_eq!(
                serde_json::to_value(snapshot(&path).unwrap()).unwrap(),
                *original,
                "{key}"
            );
        }
        assert!(!self.root.join("previous-package").exists());
        if self.pending["had_package"] == true {
            assert_eq!(
                digest(&self.root.join("market/plugin")).unwrap(),
                self.pending["ledger"]["package_digest"].as_str().unwrap()
            );
        } else {
            assert!(!self.root.join("market/plugin").exists());
        }
    }

    pub fn assert_retained_then_retry(&self, failed_path: &Path) {
        let journal = self.root.join("pending.json");
        let before = fs::read(&journal).unwrap();
        let mut injected = false;
        let result = storage::testing::with_sync_hook(
            |path| {
                if path == failed_path && journal.exists() {
                    injected = true;
                    anyhow::bail!("injected rollback prerequisite sync failure");
                }
                Ok(())
            },
            || self.recover(),
        );
        assert!(
            injected,
            "rollback never reached prerequisite {}",
            failed_path.display()
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("injected rollback prerequisite"));
        assert_eq!(fs::read(&journal).unwrap(), before);
        assert_eq!(regular_json(&journal).unwrap(), Some(self.pending.clone()));
        self.assert_original();
        // A retry must repeat the prerequisite even though restoration already completed.
        let mut retried = false;
        storage::testing::with_sync_hook(
            |path| {
                if path == failed_path && journal.exists() {
                    retried = true;
                }
                Ok(())
            },
            || self.recover(),
        )
        .unwrap();
        assert!(
            retried,
            "retry skipped prerequisite {}",
            failed_path.display()
        );
        self.assert_original();
        assert!(!journal.exists());
        // No journal is a true no-op, with no native executable or synchronization needed.
        storage::testing::with_sync_hook(
            |_| anyhow::bail!("unexpected no-journal sync"),
            || recover_install(&self.root, Path::new("must-not-spawn"), &self.home),
        )
        .unwrap();
    }
}
