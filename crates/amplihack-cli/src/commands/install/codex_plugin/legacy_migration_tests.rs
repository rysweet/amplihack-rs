//! Verified prior unframed ownership migrates; any change or foreign content is refused untouched.
#![cfg(unix)]
use super::*;

struct Legacy {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    package: PathBuf,
    source: PathBuf,
    binary: PathBuf,
}

fn fixture() -> Legacy {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("home/.amplihack/codex");
    let home = dir.path().join("home/.codex");
    let package = root.join("market/plugin");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = dir.path().join("native");
    fs::write(&binary, "#!/bin/sh\nprintf '{\"installed\":[]}'\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let hooks_binary = dir.path().join("amplihack-hooks");
    fs::write(&hooks_binary, b"hooks").unwrap();
    fs::create_dir_all(&package).unwrap();
    fs::create_dir_all(&home).unwrap();
    build(&source, &hooks_binary, &package).unwrap();
    let legacy = Legacy {
        _dir: dir,
        root,
        home,
        package,
        source,
        binary,
    };
    legacy.write_ledger(&legacy_digest(&legacy.package).unwrap());
    fs::write(legacy._dir.path().join("amplihack-hooks"), b"hooks").unwrap();
    legacy
}

impl Legacy {
    fn write_ledger(&self, package_digest: &str) {
        let ledger = json!({"schema_version":1,"codex_home":self.home,"transaction":".tmpLegacy",
            "package_digest":package_digest,"hooks":declarations(&self.package).unwrap()});
        fs::write(
            self.root.join("ownership.json"),
            json_bytes(&ledger).unwrap(),
        )
        .unwrap();
    }
    fn prepare(&self) -> Result<()> {
        let hooks = self._dir.path().join("amplihack-hooks");
        path_scope::with_scope(&self.root, &self.home, || {
            prepare(&self.root, &self.home, &self.binary, &self.source, &hooks).map(drop)
        })
    }
    fn ledger_bytes(&self) -> Vec<u8> {
        fs::read(self.root.join("ownership.json")).unwrap()
    }
    fn assert_refused_untouched(&self) {
        let ledger = self.ledger_bytes();
        let package = digest(&self.package).unwrap();
        let error = format!("{:#}", self.prepare().unwrap_err());
        assert!(
            error.contains("legacy or changed Codex ownership proof"),
            "{error}"
        );
        assert_eq!(self.ledger_bytes(), ledger);
        assert_eq!(digest(&self.package).unwrap(), package);
        assert!(!self.root.join("pending.json").exists());
    }
}

#[test]
fn verified_legacy_digest_ownership_migrates_to_framed_proof() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    let before = digest(&f.package).unwrap();
    f.prepare().unwrap();
    let ledger: Ownership = serde_json::from_slice(&f.ledger_bytes()).unwrap();
    assert!(resources::current_digest(&ledger.package_digest));
    assert_eq!(ledger.package_digest, before);
    assert_eq!(ledger.schema_version, 1);
    assert_eq!(ledger.transaction.as_deref(), Some(".tmpLegacy"));
    assert_eq!(ledger.hooks, declarations(&f.package).unwrap());
}

#[test]
fn tampered_legacy_package_is_refused_without_changes() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    fs::write(f.package.join("bin/amplihack-hooks"), b"tampered").unwrap();
    f.assert_refused_untouched();
}

#[test]
fn tampered_legacy_digest_is_refused_without_changes() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    f.write_ledger(&"0".repeat(64));
    f.assert_refused_untouched();
}

#[test]
fn matching_legacy_digest_over_foreign_content_is_not_trusted() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    fs::write(f.package.join("plugin.json"), br#"{"name":"foreign"}"#).unwrap();
    f.write_ledger(&legacy_digest(&f.package).unwrap());
    f.assert_refused_untouched();
}

#[test]
fn legacy_hooks_pointing_elsewhere_are_refused() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    let mut ledger: Value = serde_json::from_slice(&f.ledger_bytes()).unwrap();
    ledger["hooks"]["Stop"][0]["hooks"][0]["command"] = json!("'/usr/bin/foreign' stop");
    fs::write(f.root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
    f.assert_refused_untouched();
}

#[test]
fn malformed_ledger_is_refused_without_changes() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    fs::write(f.root.join("ownership.json"), b"{\"schema_version\":1,").unwrap();
    let ledger = f.ledger_bytes();
    let package = digest(&f.package).unwrap();
    assert!(f.prepare().is_err());
    assert_eq!(f.ledger_bytes(), ledger);
    assert_eq!(digest(&f.package).unwrap(), package);
}

#[test]
fn unknown_schema_version_is_refused_without_changes() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let f = fixture();
    let mut ledger: Value = serde_json::from_slice(&f.ledger_bytes()).unwrap();
    ledger["schema_version"] = json!(99);
    fs::write(f.root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
    let before = f.ledger_bytes();
    let package = digest(&f.package).unwrap();
    assert!(f.prepare().is_err());
    assert_eq!(f.ledger_bytes(), before);
    assert_eq!(digest(&f.package).unwrap(), package);
}
