//! Real production recovery with pre-existing deeper aliases, never symlink leaves.
use super::super::codex_alias_fixture::AliasPaths;
use super::*;
use crate::test_support::{EnvGuard, home_env_lock};
use std::os::unix::fs::PermissionsExt;
mod barriers;
mod safety;

fn pending_fixture(paths: &AliasPaths, committed: bool) -> Value {
    let scratch = paths.dir.path().join("scratch");
    let (root, home, mut pending) = recovery_tests::interrupted(&scratch, true);
    fs::create_dir_all(paths.root.parent().unwrap()).unwrap();
    fs::rename(&root, &paths.root).unwrap();
    fs::remove_dir(&paths.codex_home).unwrap();
    fs::rename(home, &paths.codex_home).unwrap();
    pending["codex_home"] = json!(paths.codex_home);
    pending["ledger"]["codex_home"] = json!(paths.codex_home);
    let previous = json_bytes(&pending["ledger"]).unwrap();
    pending["snapshots"]["ledger"] = json!(previous);
    pending["expected"]["ledger"] = pending["snapshots"]["ledger"].clone();
    let original =
        b"# original exact preference\n[plugins.\"amplihack@amplihack-local\"]\nenabled = true\n";
    pending["config"] = json!(original.to_vec());
    pending["expected"]["config"] = pending["config"].clone();
    fs::write(paths.codex_home.join("config.toml"), original).unwrap();
    pending["installed"] = json!(true);
    let ledger = if committed {
        json!({"schema_version":1,"codex_home":paths.codex_home,"transaction":"test",
            "package_digest":pending["target_digest"],"hooks":{}})
    } else {
        pending["ledger"].clone()
    };
    let bytes = json_bytes(&ledger).unwrap();
    pending["expected"]["ledger"] = json!(bytes);
    fs::write(paths.root.join("ownership.json"), bytes).unwrap();
    fs::write(
        paths.root.join("pending.json"),
        json_bytes(&pending).unwrap(),
    )
    .unwrap();
    pending
}

fn native_fixture(paths: &AliasPaths) -> PathBuf {
    let binary = paths.dir.path().join("native");
    let inventory = json!({"installed":[{"pluginId":ID,"installed":true,"enabled":true,
        "source":{"source":"local","path":paths.root.join("market/plugin")}}]});
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n 'plugin list --json') printf '%s\\n' '{}' ;;\n 'plugin add {ID} --json') printf 'add\\n' >> \"$0.calls\"; printf '{{}}\\n' ;;\n *) exit 29 ;;\nesac\n",
        inventory
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    binary
}

fn recover_alias(explicit: bool, committed: bool) {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let paths = AliasPaths::new(explicit);
    let _env = EnvGuard::set([
        ("HOME", paths.home.to_str().unwrap()),
        ("CODEX_HOME", paths.selected_home()),
    ]);
    let pending = pending_fixture(&paths, committed);
    let binary = native_fixture(&paths);
    let result = recover_install(&paths.root, &binary, &paths.codex_home);
    assert!(result.is_ok(), "stable deeper alias recovery: {result:?}");
    let package = paths.root.join("market/plugin");
    assert_eq!(
        fs::read(package.join("resource")).unwrap(),
        if committed {
            b"target".as_slice()
        } else {
            b"original".as_slice()
        }
    );
    for (key, path) in [
        ("config", paths.codex_home.join("config.toml")),
        ("hooks", paths.codex_home.join("hooks.json")),
        (
            "marketplace",
            paths.root.join("market/.agents/plugins/marketplace.json"),
        ),
        ("ledger", paths.root.join("ownership.json")),
    ] {
        let expected = if committed {
            &pending["expected"][key]
        } else if key == "config" {
            &pending["config"]
        } else {
            &pending["snapshots"][key]
        };
        assert_eq!(
            serde_json::to_value(snapshot(&path).unwrap()).unwrap(),
            *expected,
            "{key}"
        );
    }
    if committed {
        assert!(
            !binary.with_extension("calls").exists(),
            "committed cleanup must not register"
        );
    } else {
        assert_eq!(fs::read(binary.with_extension("calls")).unwrap(), b"add\n");
    }
    assert!(!paths.root.join("pending.json").exists());
    assert!(!paths.root.join("previous-package").exists());
    storage::testing::with_sync_hook(
        |_| bail!("no-journal recovery must not sync"),
        || recover_install(&paths.root, Path::new("must-not-spawn"), &paths.codex_home),
    )
    .unwrap();
    paths.assert_alias_unchanged();
}

#[test]
fn stable_deeper_home_alias_failed_upgrade_recovery_restores_exact_originals() {
    recover_alias(false, false);
}
#[test]
fn stable_deeper_explicit_codex_alias_failed_upgrade_recovery_restores_exact_originals() {
    recover_alias(true, false);
}
#[test]
fn stable_deeper_home_alias_committed_cleanup_preserves_live_resources() {
    recover_alias(false, true);
}
#[test]
fn stable_deeper_explicit_codex_alias_committed_cleanup_preserves_live_resources() {
    recover_alias(true, true);
}
