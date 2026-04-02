use std::path::Path;

use anyhow::{Result, bail};
use tracing::{info, warn};

use crate::models::{
    FixVerifyMode, RecoveryBlocker, Stage2Result, Stage3Cycle, Stage3Result, Stage3ValidatorResult,
    StageStatus, ValidationStatus,
};

/// Validate that cycle bounds are within the allowed range [3, 6].
pub fn validate_cycle_bounds(min: u32, max: u32) -> Result<()> {
    if min < 3 {
        bail!("min_cycles must be >= 3, got {min}");
    }
    if max > 6 {
        bail!("max_cycles must be <= 6, got {max}");
    }
    if min > max {
        bail!("min_cycles ({min}) must be <= max_cycles ({max})");
    }
    Ok(())
}

/// Run the collect-only-baseline validator.
fn run_collect_only_baseline(repo_path: &Path) -> Stage3ValidatorResult {
    let status = if repo_path.join("pytest.ini").exists()
        || repo_path.join("pyproject.toml").exists()
        || repo_path.join("setup.py").exists()
    {
        ValidationStatus::Passed
    } else {
        ValidationStatus::Failed
    };

    Stage3ValidatorResult {
        name: "collect-only-baseline".into(),
        status,
        details: "checked for test configuration files".into(),
        metadata: serde_json::json!({"repo_path": repo_path.display().to_string()}),
    }
}

/// Run the stage2-alignment validator.
fn run_stage2_alignment(stage2: &Stage2Result) -> Stage3ValidatorResult {
    let aligned = stage2.final_errors <= stage2.baseline_errors;
    Stage3ValidatorResult {
        name: "stage2-alignment".into(),
        status: if aligned {
            ValidationStatus::Passed
        } else {
            ValidationStatus::Failed
        },
        details: format!(
            "baseline={} final={} verdict={}",
            stage2.baseline_errors, stage2.final_errors, stage2.delta_verdict
        ),
        metadata: serde_json::json!({
            "baseline": stage2.baseline_errors,
            "final": stage2.final_errors,
        }),
    }
}

/// Run the fix-verify-worktree validator.
fn run_fix_verify_worktree(
    worktree_path: Option<&Path>,
    mode: &FixVerifyMode,
) -> Stage3ValidatorResult {
    match mode {
        FixVerifyMode::IsolatedWorktree => {
            let exists = worktree_path.is_some_and(|p| p.exists());
            Stage3ValidatorResult {
                name: "fix-verify-worktree".into(),
                status: if exists {
                    ValidationStatus::Passed
                } else {
                    ValidationStatus::Blocked
                },
                details: format!("worktree exists={exists}"),
                metadata: serde_json::json!({
                    "worktree": worktree_path.map(|p| p.display().to_string()),
                }),
            }
        }
        FixVerifyMode::ReadOnly => Stage3ValidatorResult {
            name: "fix-verify-worktree".into(),
            status: ValidationStatus::Passed,
            details: "read-only mode, worktree not required".into(),
            metadata: serde_json::json!({}),
        },
    }
}

/// Merge validator results into an overall status.
fn merge_validation(results: &[Stage3ValidatorResult]) -> ValidationStatus {
    if results
        .iter()
        .any(|r| r.status == ValidationStatus::Blocked)
    {
        ValidationStatus::Blocked
    } else if results.iter().any(|r| r.status == ValidationStatus::Failed) {
        ValidationStatus::Failed
    } else {
        ValidationStatus::Passed
    }
}

/// Execute a single audit cycle.
fn run_cycle(
    cycle_number: u32,
    stage2: &Stage2Result,
    repo_path: &Path,
    worktree_path: Option<&Path>,
    mode: &FixVerifyMode,
) -> Stage3Cycle {
    info!("stage3: cycle {cycle_number}");

    let validators = vec![
        "collect-only-baseline".to_string(),
        "stage2-alignment".to_string(),
        "fix-verify-worktree".to_string(),
    ];

    let v1 = run_collect_only_baseline(repo_path);
    let v2 = run_stage2_alignment(stage2);
    let v3 = run_fix_verify_worktree(worktree_path, mode);
    let validation_results = vec![v1, v2, v3];

    let merged = merge_validation(&validation_results);
    let blocked = merged == ValidationStatus::Blocked;

    Stage3Cycle {
        cycle_number,
        phases: vec!["validate".into(), "audit".into()],
        findings: vec![],
        validators,
        merged_validation: Some(merged),
        fix_verify_mode: mode.clone(),
        blocked,
        validation_results,
    }
}

/// Stage 3: quality audit cycles with validators.
pub fn run_stage3(
    stage2: &Stage2Result,
    repo_path: &Path,
    worktree_path: Option<&Path>,
    min_cycles: u32,
    max_cycles: u32,
) -> Result<Stage3Result> {
    validate_cycle_bounds(min_cycles, max_cycles)?;
    info!("stage3: running {min_cycles}-{max_cycles} audit cycles");

    let mode = if worktree_path.is_some() {
        FixVerifyMode::IsolatedWorktree
    } else {
        FixVerifyMode::ReadOnly
    };

    let mut cycles = Vec::new();
    let mut blockers = Vec::new();
    let mut all_passed_early = false;

    for i in 1..=max_cycles {
        let cycle = run_cycle(i, stage2, repo_path, worktree_path, &mode);

        if cycle.blocked {
            warn!("stage3: cycle {i} blocked");
            blockers.push(RecoveryBlocker {
                stage: 3,
                code: "CYCLE_BLOCKED".into(),
                message: format!("cycle {i} blocked by validator"),
                retryable: true,
            });
        }

        let passed = cycle
            .merged_validation
            .as_ref()
            .is_some_and(|s| *s == ValidationStatus::Passed);

        cycles.push(cycle);

        // Can exit early after min_cycles if everything passes
        if i >= min_cycles && passed {
            all_passed_early = true;
            break;
        }
    }

    let completed = cycles.len() as u32;
    let overall_blocked = !blockers.is_empty();
    let status = if overall_blocked && !all_passed_early {
        StageStatus::Blocked
    } else {
        StageStatus::Completed
    };

    Ok(Stage3Result {
        status,
        cycles_completed: completed,
        fix_verify_mode: mode,
        blocked: overall_blocked,
        phases: vec!["validate".into(), "audit".into()],
        cycles,
        blockers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DeltaVerdict;

    fn make_stage2(baseline: u32, final_e: u32) -> Stage2Result {
        Stage2Result {
            status: StageStatus::Completed,
            baseline_errors: baseline,
            final_errors: final_e,
            delta_verdict: DeltaVerdict::Unchanged,
            signatures: vec![],
            clusters: vec![],
            applied_fixes: vec![],
            diagnostics: vec![],
            blockers: vec![],
        }
    }

    #[test]
    fn validate_bounds_ok() {
        assert!(validate_cycle_bounds(3, 6).is_ok());
        assert!(validate_cycle_bounds(3, 3).is_ok());
        assert!(validate_cycle_bounds(4, 5).is_ok());
    }

    #[test]
    fn validate_bounds_min_too_low() {
        assert!(validate_cycle_bounds(2, 6).is_err());
    }

    #[test]
    fn validate_bounds_max_too_high() {
        assert!(validate_cycle_bounds(3, 7).is_err());
    }

    #[test]
    fn validate_bounds_inverted() {
        assert!(validate_cycle_bounds(5, 3).is_err());
    }

    #[test]
    fn merge_all_passed() {
        let results = vec![Stage3ValidatorResult {
            name: "a".into(),
            status: ValidationStatus::Passed,
            details: String::new(),
            metadata: serde_json::json!({}),
        }];
        assert_eq!(merge_validation(&results), ValidationStatus::Passed);
    }

    #[test]
    fn merge_one_failed() {
        let results = vec![
            Stage3ValidatorResult {
                name: "a".into(),
                status: ValidationStatus::Passed,
                details: String::new(),
                metadata: serde_json::json!({}),
            },
            Stage3ValidatorResult {
                name: "b".into(),
                status: ValidationStatus::Failed,
                details: String::new(),
                metadata: serde_json::json!({}),
            },
        ];
        assert_eq!(merge_validation(&results), ValidationStatus::Failed);
    }

    #[test]
    fn merge_blocked_takes_precedence() {
        let results = vec![
            Stage3ValidatorResult {
                name: "a".into(),
                status: ValidationStatus::Failed,
                details: String::new(),
                metadata: serde_json::json!({}),
            },
            Stage3ValidatorResult {
                name: "b".into(),
                status: ValidationStatus::Blocked,
                details: String::new(),
                metadata: serde_json::json!({}),
            },
        ];
        assert_eq!(merge_validation(&results), ValidationStatus::Blocked);
    }

    #[test]
    fn run_stage3_minimum_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a pyproject.toml so collect-only-baseline passes
        std::fs::write(tmp.path().join("pyproject.toml"), "[tool.pytest]").unwrap();
        let s2 = make_stage2(5, 3);

        let result = run_stage3(&s2, tmp.path(), None, 3, 6).unwrap();
        assert!(result.cycles_completed >= 3);
        assert_eq!(result.status, StageStatus::Completed);
    }

    #[test]
    fn run_stage3_invalid_bounds_rejected() {
        let s2 = make_stage2(0, 0);
        assert!(run_stage3(&s2, Path::new("."), None, 1, 6).is_err());
    }
}
