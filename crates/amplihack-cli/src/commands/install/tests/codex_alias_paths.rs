//! Supported-path regressions use the same oracles on immutable and dirty source.
use super::super::codex_alias_fixture::AliasPaths;
use super::super::codex_plugin;
use super::helpers::{create_exe_stub, create_source_repo};
use crate::test_support::{EnvGuard, home_env_lock};
use std::fs;
mod safety;

fn install_alias(explicit: bool) {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fixture = AliasPaths::new(explicit);
    let source = fixture.dir.path().join("source");
    create_source_repo(&source);
    let bin = fixture.dir.path().join("bin");
    let binary = create_exe_stub(&bin, "codex");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    let _env = EnvGuard::set([
        ("HOME", fixture.home.to_str().unwrap()),
        ("CODEX_HOME", fixture.selected_home()),
        ("PATH", &format!("{}:/usr/bin:/bin", bin.display())),
        ("AMPLIHACK_CODEX_BINARY_PATH", binary.to_str().unwrap()),
        ("AMPLIHACK_AGENT_BINARY", "codex"),
    ]);
    let result = codex_plugin::install(&source, &hooks);
    assert!(
        matches!(result, Ok(true)),
        "stable deeper alias install: {result:?}"
    );
    let ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("ownership.json")).unwrap()).unwrap();
    assert_eq!(ledger["codex_home"], fixture.codex_home.to_str().unwrap());
    assert!(fixture.root.join("market/plugin/plugin.json").is_file());
    assert!(!fixture.root.join("pending.json").exists());
    // An actual second transaction exercises previous-package committed cleanup.
    fs::write(
        source.join("amplifier-bundle/skills/marker.txt"),
        b"updated\n",
    )
    .unwrap();
    assert!(codex_plugin::install(&source, &hooks).unwrap());
    assert_eq!(
        fs::read(fixture.root.join("market/plugin/skills/marker.txt")).unwrap(),
        b"updated\n"
    );
    assert!(!fixture.root.join("previous-package").exists());
    assert!(!fixture.root.join("pending.json").exists());
    fixture.assert_alias_unchanged();
}

#[test]
fn stable_deeper_home_alias_install_and_update_preserve_lexical_scope() {
    install_alias(false);
}

#[test]
fn stable_deeper_explicit_codex_alias_install_and_update_preserve_lexical_scope() {
    install_alias(true);
}
