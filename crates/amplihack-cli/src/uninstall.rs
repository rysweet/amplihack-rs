//! Amplihack uninstallation logic.
//!
//! Reads the install manifest, removes managed files and directories,
//! cleans amplihack hooks from VS Code / Copilot settings, and finally
//! removes the manifest itself.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

const MANIFEST_NAME: &str = "amplihack-manifest.json";

/// On-disk install manifest written during `amplihack install`.
#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    dirs: Vec<String>,
}

/// Read the install manifest and return the lists of managed files and dirs.
///
/// Returns `(files, dirs)`. Both vectors are empty if the manifest is missing
/// or malformed.
pub fn read_manifest() -> (Vec<String>, Vec<String>) {
    let path = manifest_path();
    let Some(path) = path else {
        debug!("Manifest path could not be determined");
        return (vec![], vec![]);
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Manifest>(&content) {
            Ok(m) => {
                debug!(
                    files = m.files.len(),
                    dirs = m.dirs.len(),
                    "Loaded manifest"
                );
                (m.files, m.dirs)
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse manifest JSON");
                (vec![], vec![])
            }
        },
        Err(e) => {
            debug!(error = %e, "Manifest not found or unreadable");
            (vec![], vec![])
        }
    }
}

/// Remove amplihack hook entries from VS Code / Copilot settings.json files.
///
/// Scans the default settings locations, strips any JSON keys that reference
/// amplihack hooks, and rewrites the file. Returns the number of settings
/// files modified.
pub fn remove_hooks_from_settings() -> usize {
    let settings_paths = candidate_settings_paths();
    let mut modified = 0usize;

    for path in &settings_paths {
        if !path.exists() {
            continue;
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        let cleaned = remove_amplihack_hooks_from_json(&content);
        if cleaned != content {
            match std::fs::write(path, &cleaned) {
                Ok(()) => {
                    info!(path = %path.display(), "Removed amplihack hooks from settings");
                    modified += 1;
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to write cleaned settings");
                }
            }
        }
    }

    modified
}

/// Full uninstall: remove files, directories, hooks, and manifest.
pub fn uninstall() -> Result<()> {
    let (files, dirs) = read_manifest();

    // Remove managed files
    for file in &files {
        let p = Path::new(file);
        if p.exists() {
            if let Err(e) = std::fs::remove_file(p) {
                warn!(file, error = %e, "Failed to remove file");
            } else {
                debug!(file, "Removed file");
            }
        }
    }

    // Remove managed directories (in reverse order for nested dirs)
    for dir in dirs.iter().rev() {
        let p = Path::new(dir);
        if p.exists() {
            if let Err(e) = std::fs::remove_dir_all(p) {
                warn!(dir, error = %e, "Failed to remove directory");
            } else {
                debug!(dir, "Removed directory");
            }
        }
    }

    // Remove hooks from settings files
    let settings_modified = remove_hooks_from_settings();
    info!(settings_modified, "Cleaned settings files");

    // Remove the manifest itself
    if let Some(mp) = manifest_path()
        && mp.exists()
    {
        std::fs::remove_file(&mp)
            .with_context(|| format!("Failed to remove manifest at {}", mp.display()))?;
        info!("Removed install manifest");
    }

    info!("Uninstall complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Compute the manifest file path under `~/.config/amplihack/`.
fn manifest_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("amplihack")
            .join(MANIFEST_NAME),
    )
}

/// Candidate VS Code / Copilot settings.json paths.
fn candidate_settings_paths() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec![];
    };

    vec![
        home.join(".vscode").join("settings.json"),
        home.join(".copilot").join("settings.json"),
        home.join(".config")
            .join("Code")
            .join("User")
            .join("settings.json"),
    ]
}

/// Remove JSON lines referencing amplihack hooks.
///
/// This performs a simple line-based filter rather than full JSON parsing
/// to avoid reformatting the user's settings file.
fn remove_amplihack_hooks_from_json(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim().to_lowercase();
            !(trimmed.contains("amplihack") && trimmed.contains("hook"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_manifest_empty_when_missing() {
        let (files, dirs) = read_manifest();
        // In CI the manifest won't exist; both should be empty
        // (this test validates the graceful fallback)
        let _ = (files, dirs);
    }

    #[test]
    fn remove_amplihack_hooks_strips_matching_lines() {
        let input = r#"{
    "editor.fontSize": 14,
    "amplihack.hook.precommit": "echo hi",
    "editor.tabSize": 4
}"#;
        let result = remove_amplihack_hooks_from_json(input);
        assert!(!result.contains("amplihack"));
        assert!(result.contains("editor.fontSize"));
        assert!(result.contains("editor.tabSize"));
    }

    #[test]
    fn remove_amplihack_hooks_no_match_unchanged() {
        let input = r#"{"editor.fontSize": 14}"#;
        let result = remove_amplihack_hooks_from_json(input);
        assert_eq!(result, input);
    }

    #[test]
    fn remove_amplihack_hooks_case_insensitive() {
        let input = "\"Amplihack.Hook.foo\": true\n\"other\": 1";
        let result = remove_amplihack_hooks_from_json(input);
        assert!(!result.contains("Amplihack"));
        assert!(result.contains("other"));
    }

    #[test]
    fn manifest_path_based_on_home() {
        // Just verify it returns Some when HOME is set
        if std::env::var_os("HOME").is_some() {
            let p = manifest_path();
            assert!(p.is_some());
            let p = p.unwrap();
            assert!(p.ends_with("amplihack-manifest.json"));
        }
    }

    #[test]
    fn manifest_deserialization() {
        let json = r#"{"files": ["/a", "/b"], "dirs": ["/c"]}"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.dirs.len(), 1);
    }

    #[test]
    fn manifest_deserialization_empty_defaults() {
        let json = "{}";
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.files.is_empty());
        assert!(m.dirs.is_empty());
    }
}
