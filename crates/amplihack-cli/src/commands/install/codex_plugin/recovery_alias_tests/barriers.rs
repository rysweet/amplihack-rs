//! Injected barriers establish ordering and retry, not hardware power-loss proof.
use super::*;

#[test]
fn alias_rollback_barriers_use_canonical_paths_bounded_by_caller_anchors() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for explicit in [false, true] {
        let paths = AliasPaths::new(explicit);
        let _env = EnvGuard::set([
            ("HOME", paths.home.to_str().unwrap()),
            ("CODEX_HOME", paths.selected_home()),
        ]);
        pending_fixture(&paths, false);
        let binary = native_fixture(&paths);
        let anchors = [
            fs::canonicalize(&paths.home).unwrap(),
            fs::canonicalize(paths.codex_home.parent().unwrap()).unwrap(),
        ];
        let mut trace = Vec::new();
        let result = storage::testing::with_sync_hook(
            |path| {
                trace.push(path.to_path_buf());
                ensure!(
                    anchors.iter().any(|anchor| path.starts_with(anchor)),
                    "barrier escaped captured anchors: {}",
                    path.display()
                );
                ensure!(
                    fs::canonicalize(path)? == path,
                    "barrier used lexical alias instead of canonical anchor"
                );
                Ok(())
            },
            || recover_install(&paths.root, &binary, &paths.codex_home),
        );
        assert!(result.is_ok(), "bounded alias rollback: {result:?}");
        assert!(!trace.is_empty(), "barrier trace must actually execute");
        for anchor in &anchors {
            assert!(trace.contains(anchor), "anchor publication barrier missing");
        }
        let package_resource = fs::canonicalize(paths.root.join("market/plugin/resource")).unwrap();
        assert_eq!(
            trace.iter().filter(|p| **p == package_resource).count(),
            1,
            "package tree sync once per attempt"
        );
        assert!(!paths.root.join("pending.json").exists());
    }
}

#[test]
fn alias_rollback_prerequisite_failure_retains_exact_journal_and_retries_barrier() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for explicit in [false, true] {
        let paths = AliasPaths::new(explicit);
        let _env = EnvGuard::set([
            ("HOME", paths.home.to_str().unwrap()),
            ("CODEX_HOME", paths.selected_home()),
        ]);
        pending_fixture(&paths, false);
        let binary = native_fixture(&paths);
        let journal = paths.root.join("pending.json");
        let before = fs::read(&journal).unwrap();
        let config = fs::canonicalize(paths.codex_home.join("config.toml")).unwrap();
        let mut hits = 0;
        let result = storage::testing::with_sync_hook(
            |path| {
                if fs::canonicalize(path)? == config && journal.exists() {
                    hits += 1;
                    bail!("injected alias restored-config barrier failure");
                }
                Ok(())
            },
            || recover_install(&paths.root, &binary, &paths.codex_home),
        );
        assert_eq!(hits, 1, "alias recovery must reach restored-config barrier");
        assert!(format!("{:#}", result.unwrap_err()).contains("injected alias restored-config"));
        assert_eq!(fs::read(&journal).unwrap(), before);
        assert_eq!(
            fs::read(paths.root.join("market/plugin/resource")).unwrap(),
            b"original"
        );
        let mut retries = 0;
        storage::testing::with_sync_hook(
            |path| {
                if fs::canonicalize(path)? == config && journal.exists() {
                    retries += 1;
                }
                Ok(())
            },
            || recover_install(&paths.root, &binary, &paths.codex_home),
        )
        .unwrap();
        assert_eq!(
            retries, 1,
            "retry must repeat prerequisite after restoration"
        );
        assert!(!journal.exists());
        paths.assert_alias_unchanged();
    }
}

#[test]
fn alias_committed_cleanup_retries_after_actual_unlink_with_durable_inventory() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    for explicit in [false, true] {
        let paths = AliasPaths::new(explicit);
        let _env = EnvGuard::set([
            ("HOME", paths.home.to_str().unwrap()),
            ("CODEX_HOME", paths.selected_home()),
        ]);
        pending_fixture(&paths, true);
        let backup = paths.root.join("previous-package");
        let mut hits = 0;
        let result = storage::testing::with_sync_hook(
            |path| {
                if fs::canonicalize(path)? == fs::canonicalize(&backup)?
                    && !backup.join("resource").exists()
                {
                    hits += 1;
                    bail!("injected alias committed-cleanup interruption");
                }
                Ok(())
            },
            || recover_install(&paths.root, Path::new("must-not-spawn"), &paths.codex_home),
        );
        assert_eq!(
            hits, 1,
            "cleanup must actually unlink then hit directory barrier"
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("injected alias committed-cleanup"));
        let pending = regular_json(&paths.root.join("pending.json"))
            .unwrap()
            .unwrap();
        assert!(
            pending["backup_cleanup"].is_object(),
            "durable authorization precedes unlink"
        );
        recover_install(&paths.root, Path::new("must-not-spawn"), &paths.codex_home).unwrap();
        assert_eq!(
            fs::read(paths.root.join("market/plugin/resource")).unwrap(),
            b"target"
        );
        assert!(!backup.exists());
        assert!(!paths.root.join("pending.json").exists());
    }
}
