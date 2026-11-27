//! Home isolation must keep native Codex installation inside the fixture.

use super::super::codex_plugin;
use super::helpers::{create_exe_stub, create_source_repo};
use super::issue_1526_implicit_downgrade::snapshot;
use crate::test_support::{EnvGuard, HomeGuard, env_lock, tempdir_outside_framework_roots};
use std::fs;

#[test]
fn native_install_leaves_an_inherited_codex_home_unchanged() {
    if crate::test_support::isolated_install_test() {
        return;
    }

    let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let scratch = tempdir_outside_framework_roots();
    let inherited_home = scratch.path().join("inherited-codex");
    let home = scratch.path().join("home");
    let source = scratch.path().join("source");
    fs::create_dir_all(&inherited_home).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(
        inherited_home.join("preferences.txt"),
        "# existing preferences\n",
    )
    .unwrap();
    fs::write(inherited_home.join("hooks.json"), "{\"hooks\":{}}\n").unwrap();
    create_source_repo(&source);
    let bin = scratch.path().join("bin");
    let codex = create_exe_stub(&bin, "codex");
    let hooks = create_exe_stub(&bin, "amplihack-hooks");
    let _inherited = EnvGuard::set([("CODEX_HOME", inherited_home.to_str().unwrap())]);
    let before = snapshot(&inherited_home);

    {
        let _home = HomeGuard::set(&home);
        let installation =
            codex_plugin::with_binary(&codex, || codex_plugin::install(&source, &hooks));
        let after = snapshot(&inherited_home);
        let changed: Vec<_> = before
            .keys()
            .chain(after.keys())
            .filter(|path| before.get(*path) != after.get(*path))
            .collect();
        assert!(
            changed.is_empty(),
            "inherited Codex home changed: {changed:?}"
        );
        assert!(
            installation.expect("native installation must succeed"),
            "native installation must not be skipped"
        );
        assert!(home.join(".codex/config.toml").is_file());
        assert!(home.join(".codex/hooks.json").is_file());
        assert!(
            home.join(".amplihack/codex/market/plugin/plugin.json")
                .is_file()
        );
    }

    assert_eq!(
        std::env::var_os("CODEX_HOME").as_deref(),
        Some(inherited_home.as_os_str())
    );
}
