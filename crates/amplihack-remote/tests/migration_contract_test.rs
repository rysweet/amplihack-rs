use std::path::{Path, PathBuf};

use amplihack_remote::{SessionManager, VMSize};

fn collect_python_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_python_files(&path, found);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("py") {
            found.push(path);
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate should live under <repo>/crates/amplihack-remote")
        .to_path_buf()
}

#[test]
fn detached_sessions_default_to_32gb_node_heap_contract() {
    assert_eq!(
        SessionManager::DEFAULT_MEMORY_MB,
        32_768,
        "remote start must persist memory_mb=32768 to match NODE_OPTIONS=--max-old-space-size=32768"
    );

    let state_file = tempfile::tempdir()
        .expect("temp dir should be created")
        .path()
        .join("remote-state.json");
    let mut manager = SessionManager::new(Some(state_file)).expect("session manager should load");
    let session = manager
        .create_session("vm-a", "implement issue #536", Some("auto"), Some(10), None)
        .expect("valid session should be created");

    assert_eq!(
        session.memory_mb, 32_768,
        "sessions created without an override should persist the 32GB heap contract"
    );
}

#[test]
fn vm_size_tiers_match_documented_capacity_and_azure_skus() {
    assert_eq!(VMSize::S.capacity(), 1);
    assert_eq!(VMSize::S.azure_size(), "Standard_D8s_v3");
    assert_eq!(VMSize::M.capacity(), 2);
    assert_eq!(VMSize::M.azure_size(), "Standard_E8s_v5");
    assert_eq!(VMSize::L.capacity(), 4);
    assert_eq!(VMSize::L.azure_size(), "Standard_E16s_v5");
    assert_eq!(VMSize::XL.capacity(), 8);
    assert_eq!(VMSize::XL.azure_size(), "Standard_E32s_v5");
}

#[test]
fn python_remote_tree_is_deleted_after_native_port() {
    let remote_dir = repo_root().join("amplifier-bundle/tools/amplihack/remote");
    let mut python_files = Vec::new();
    collect_python_files(&remote_dir, &mut python_files);
    python_files.sort();

    assert!(
        python_files.is_empty(),
        "issue #536 requires deleting every Python file under {}; still found: {:#?}",
        remote_dir.display(),
        python_files
    );
}

#[test]
fn remote_rust_modules_stay_under_500_lines() {
    let src_dir = repo_root().join("crates/amplihack-remote/src");
    let mut oversized = Vec::new();

    for entry in std::fs::read_dir(&src_dir).expect("remote src dir should exist") {
        let path = entry.expect("src entry should be readable").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let line_count = std::fs::read_to_string(&path)
            .expect("source file should be readable")
            .lines()
            .count();
        if line_count > 500 {
            oversized.push((path, line_count));
        }
    }

    assert!(
        oversized.is_empty(),
        "issue #536 requires every materially changed amplihack-remote Rust module to stay <=500 lines; oversized modules: {oversized:#?}"
    );
}

#[test]
fn github_hooks_scope_creep_is_absent() {
    let hooks_dir = repo_root().join(".github/hooks");
    assert!(
        !hooks_dir.exists(),
        "issue #536 forbids .github/hooks scope creep; remove {} before committing",
        hooks_dir.display()
    );
}
