//! Failed native upgrades must preserve the user's disabled preference exactly.
use super::super::codex_plugin;
use super::helpers::{create_exe_stub, create_source_repo};
use super::native_registration_fixture::{executable, fail_upgrade, inventory};
use crate::test_support::{EnvGuard, home_env_lock};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn package_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn scenario(foreign_on_add: bool) {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let source = home.join("source");
    create_source_repo(&source);
    let bin = home.join("bin");
    let binary = executable(&bin);
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    let codex_home = home.join(".codex");
    let _env = EnvGuard::set([
        ("HOME", home.to_str().unwrap()),
        ("CODEX_HOME", codex_home.to_str().unwrap()),
        ("PATH", &format!("{}:/usr/bin:/bin", bin.display())),
        ("AMPLIHACK_CODEX_BINARY_PATH", binary.to_str().unwrap()),
        ("AMPLIHACK_AGENT_BINARY", "codex"),
    ]);
    codex_plugin::install(&source, &hooks).unwrap();
    let root = home.join(".amplihack/codex");
    let config = codex_home.join("config.toml");
    let disabled = format!(
        "# user disabled this owned plugin\n{}",
        fs::read_to_string(&config)
            .unwrap()
            .replace("enabled = true", "enabled = false")
    );
    fs::write(&config, &disabled).unwrap();
    assert_eq!(
        inventory(&binary, home, &codex_home)["installed"][0]["enabled"],
        false
    );
    // Preserve formatting as well as JSON semantics for all original snapshots.
    let paths = [
        config.clone(),
        codex_home.join("hooks.json"),
        root.join("ownership.json"),
        root.join("market/.agents/plugins/marketplace.json"),
    ];
    for path in &paths[1..] {
        let bytes = fs::read(path).unwrap();
        fs::write(path, [b"\n".as_slice(), &bytes, b"\n"].concat()).unwrap();
    }
    let snapshots: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
    let package = root.join("market/plugin");
    let old_package = package_bytes(&package);
    fs::write(
        source.join("amplifier-bundle/skills/marker.txt"),
        "updated source\n",
    )
    .unwrap();
    fail_upgrade(&binary);
    if foreign_on_add {
        fs::write(binary.with_extension("foreign-on-add"), b"").unwrap();
    }
    let adds_before = fs::read_to_string(binary.with_extension("calls"))
        .unwrap()
        .lines()
        .count();
    let error = format!("{:#}", codex_plugin::install(&source, &hooks).unwrap_err());
    assert!(
        error.contains("17"),
        "original upgrade failure must survive: {error}"
    );
    assert_eq!(
        fs::read_to_string(binary.with_extension("calls"))
            .unwrap()
            .lines()
            .count(),
        adds_before + 1,
        "rollback must re-register"
    );
    assert_eq!(package_bytes(&package), old_package);
    assert_eq!(
        inventory(&binary, home, &codex_home)["installed"][0]["installed"],
        true
    );
    if foreign_on_add {
        assert!(error.contains("retained"), "{error}");
        let foreign = fs::read(&config).unwrap();
        assert!(String::from_utf8_lossy(&foreign).contains("# foreign during native add"));
        let journal = fs::read(root.join("pending.json")).unwrap();
        assert!(codex_plugin::install(&source, &hooks).is_err());
        assert_eq!(fs::read(&config).unwrap(), foreign);
        assert_eq!(fs::read(root.join("pending.json")).unwrap(), journal);
    } else {
        assert!(!error.contains("recovery failed"), "{error}");
        assert_eq!(
            inventory(&binary, home, &codex_home)["installed"][0]["enabled"],
            false,
            "rollback must leave the old plugin disabled"
        );
        for (path, bytes) in paths.iter().zip(snapshots) {
            assert_eq!(fs::read(path).unwrap(), bytes, "{}", path.display());
        }
        assert!(!root.join("previous-package").exists());
        assert!(!root.join("pending.json").exists());
    }
}

#[test]
fn failed_upgrade_restores_disabled_owned_plugin_and_exact_original_snapshots() {
    scenario(false);
}

#[test]
fn failed_upgrade_preserves_foreign_config_written_during_reregistration() {
    scenario(true);
}
