//! TDD guard for issue #1081.
//!
//! The arbitrary resource caps objected to in issue #1081
//! (`DEFAULT_MAX_SESSION_SECS`, `DEFAULT_MAX_API_CALLS`,
//! `DEFAULT_MAX_OUTPUT_BYTES`) lived only in the dead launcher `auto_mode*`
//! modules. Deleting that dead code is the fix. This guard fails if any of the
//! three names reappear anywhere under `crates/`, so the caps cannot silently
//! return.
//!
//! It intentionally does NOT match `DEFAULT_MAX_TURNS`, which is a legitimate
//! live constant in `amplihack-remote` and `fleet` and must remain.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

/// The forbidden cap identifiers. These must exist nowhere in the source tree.
const FORBIDDEN_CAP_NAMES: &[&str] = &[
    "DEFAULT_MAX_SESSION_SECS",
    "DEFAULT_MAX_API_CALLS",
    "DEFAULT_MAX_OUTPUT_BYTES",
];

fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" {
                continue;
            }
            collect_rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn dead_resource_cap_names_are_absent_from_crates_tree() {
    let crates_dir = repo_root().join("crates");
    let mut sources = Vec::new();
    collect_rust_sources(&crates_dir, &mut sources);
    assert!(
        !sources.is_empty(),
        "expected to scan Rust sources under {}",
        crates_dir.display()
    );

    // Exclude this guard file itself, which necessarily names the constants.
    let this_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("dead_code_guard.rs");

    let mut offenders = Vec::new();
    for path in sources {
        if path == this_file {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for name in FORBIDDEN_CAP_NAMES {
            if text.contains(name) {
                offenders.push(format!("{} contains `{}`", path.display(), name));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "issue #1081: dead resource-cap constants must not exist in the tree:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn dead_launcher_auto_mode_modules_are_removed() {
    let launcher_src = repo_root()
        .join("crates")
        .join("amplihack-launcher")
        .join("src");
    for module in [
        "auto_mode.rs",
        "auto_mode_coordinator.rs",
        "auto_mode_state.rs",
        "auto_mode_ui.rs",
    ] {
        let path = launcher_src.join(module);
        assert!(
            !path.exists(),
            "issue #1081: dead launcher module must be deleted: {}",
            path.display()
        );
    }

    let lib_rs = launcher_src.join("lib.rs");
    let text = std::fs::read_to_string(&lib_rs)
        .unwrap_or_else(|e| panic!("read {}: {e}", lib_rs.display()));
    for forbidden in [
        "pub mod auto_mode;",
        "pub mod auto_mode_coordinator;",
        "pub mod auto_mode_state;",
        "pub mod auto_mode_ui;",
        "auto_mode::",
        "auto_mode_coordinator::",
        "auto_mode_state::",
        "auto_mode_ui::",
    ] {
        assert!(
            !text.contains(forbidden),
            "issue #1081: launcher lib.rs must not reference `{forbidden}`"
        );
    }
}
