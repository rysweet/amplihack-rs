use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use tracing::info;

use crate::models::RecoveryRun;

/// Convert a `RecoveryRun` to a JSON value.
pub fn recovery_run_to_json(run: &RecoveryRun) -> serde_json::Value {
    serde_json::to_value(run).unwrap_or_else(|e| {
        serde_json::json!({
            "error": format!("serialization failed: {e}"),
        })
    })
}

/// Write the recovery ledger to a JSON file.
pub fn write_recovery_ledger(run: &RecoveryRun, output_path: &Path) -> Result<()> {
    let json = recovery_run_to_json(run);
    let pretty = serde_json::to_string_pretty(&json)
        .context("failed to serialize recovery ledger")?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .context("failed to create ledger output directory")?;
    }

    fs::write(output_path, &pretty)
        .context("failed to write recovery ledger")?;

    info!("wrote recovery ledger to {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RecoveryRun, StageStatus, Stage1Result, FixVerifyMode};
    use chrono::Utc;
    use std::path::PathBuf;

    fn make_run() -> RecoveryRun {
        RecoveryRun {
            repo_path: PathBuf::from("/test/repo"),
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            protected_staged_files: vec!["foo.rs".into()],
            stage1: Some(Stage1Result {
                status: StageStatus::Completed,
                mode: FixVerifyMode::ReadOnly,
                protected_staged_files: vec!["foo.rs".into()],
                actions: vec!["captured files".into()],
                blockers: vec![],
            }),
            stage2: None,
            stage3: None,
            stage4: None,
            blockers: vec![],
        }
    }

    #[test]
    fn to_json_contains_repo_path() {
        let run = make_run();
        let json = recovery_run_to_json(&run);
        assert_eq!(json["repo_path"], "/test/repo");
    }

    #[test]
    fn to_json_contains_stage1() {
        let run = make_run();
        let json = recovery_run_to_json(&run);
        assert_eq!(json["stage1"]["status"], "completed");
    }

    #[test]
    fn json_roundtrip() {
        let run = make_run();
        let json = recovery_run_to_json(&run);
        let serialized = serde_json::to_string(&json).unwrap();
        let back: RecoveryRun = serde_json::from_str(&serialized).unwrap();
        assert_eq!(back.repo_path, PathBuf::from("/test/repo"));
    }

    #[test]
    fn write_ledger_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("sub/recovery.json");
        let run = make_run();
        write_recovery_ledger(&run, &ledger).unwrap();
        assert!(ledger.exists());
    }

    #[test]
    fn write_ledger_is_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("recovery.json");
        let run = make_run();
        write_recovery_ledger(&run, &ledger).unwrap();
        let content = std::fs::read_to_string(&ledger).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.is_object());
    }
}
