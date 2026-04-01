use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StageStatus {
    Completed,
    Blocked,
}

impl fmt::Display for StageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

impl FromStr for StageStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "completed" => Ok(Self::Completed),
            "blocked" => Ok(Self::Blocked),
            other => Err(anyhow::anyhow!("unknown StageStatus: {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeltaVerdict {
    Reduced,
    Unchanged,
    Replaced,
}

impl fmt::Display for DeltaVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reduced => write!(f, "reduced"),
            Self::Unchanged => write!(f, "unchanged"),
            Self::Replaced => write!(f, "replaced"),
        }
    }
}

impl FromStr for DeltaVerdict {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "reduced" => Ok(Self::Reduced),
            "unchanged" => Ok(Self::Unchanged),
            "replaced" => Ok(Self::Replaced),
            other => Err(anyhow::anyhow!("unknown DeltaVerdict: {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixVerifyMode {
    ReadOnly,
    IsolatedWorktree,
}

impl fmt::Display for FixVerifyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "read-only"),
            Self::IsolatedWorktree => write!(f, "isolated-worktree"),
        }
    }
}

impl FromStr for FixVerifyMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read-only" => Ok(Self::ReadOnly),
            "isolated-worktree" => Ok(Self::IsolatedWorktree),
            other => Err(anyhow::anyhow!("unknown FixVerifyMode: {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AtlasProvenance {
    IsolatedWorktree,
    CurrentTreeReadOnly,
    Blocked,
}

impl fmt::Display for AtlasProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IsolatedWorktree => write!(f, "isolated-worktree"),
            Self::CurrentTreeReadOnly => write!(f, "current-tree-read-only"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

impl FromStr for AtlasProvenance {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "isolated-worktree" => Ok(Self::IsolatedWorktree),
            "current-tree-read-only" => Ok(Self::CurrentTreeReadOnly),
            "blocked" => Ok(Self::Blocked),
            other => Err(anyhow::anyhow!("unknown AtlasProvenance: {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationStatus {
    Passed,
    Failed,
    Blocked,
}

impl fmt::Display for ValidationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed => write!(f, "passed"),
            Self::Failed => write!(f, "failed"),
            Self::Blocked => write!(f, "blocked"),
        }
    }
}

impl FromStr for ValidationStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "blocked" => Ok(Self::Blocked),
            other => Err(anyhow::anyhow!("unknown ValidationStatus: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryBlocker {
    pub stage: u8,
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

// -- Stage 1 ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage1Result {
    pub status: StageStatus,
    pub mode: FixVerifyMode,
    pub protected_staged_files: Vec<String>,
    pub actions: Vec<String>,
    pub blockers: Vec<RecoveryBlocker>,
}

// -- Stage 2 ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage2ErrorSignature {
    pub signature_id: String,
    pub error_type: String,
    pub headline: String,
    pub normalized_location: String,
    pub normalized_message: String,
    pub occurrences: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage2Result {
    pub status: StageStatus,
    pub baseline_errors: u32,
    pub final_errors: u32,
    pub delta_verdict: DeltaVerdict,
    pub signatures: Vec<Stage2ErrorSignature>,
    pub clusters: Vec<serde_json::Value>,
    pub applied_fixes: Vec<String>,
    pub diagnostics: Vec<String>,
    pub blockers: Vec<RecoveryBlocker>,
}

// -- Stage 3 ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage3ValidatorResult {
    pub name: String,
    pub status: ValidationStatus,
    pub details: String,
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage3Cycle {
    pub cycle_number: u32,
    pub phases: Vec<String>,
    pub findings: Vec<String>,
    pub validators: Vec<String>,
    pub merged_validation: Option<ValidationStatus>,
    pub fix_verify_mode: FixVerifyMode,
    pub blocked: bool,
    pub validation_results: Vec<Stage3ValidatorResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage3Result {
    pub status: StageStatus,
    pub cycles_completed: u32,
    pub fix_verify_mode: FixVerifyMode,
    pub blocked: bool,
    pub phases: Vec<String>,
    pub cycles: Vec<Stage3Cycle>,
    pub blockers: Vec<RecoveryBlocker>,
}

// -- Stage 4 ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stage4AtlasRun {
    pub status: StageStatus,
    pub skill: String,
    pub provenance: AtlasProvenance,
    pub artifacts: Vec<String>,
    pub blockers: Vec<RecoveryBlocker>,
}

// -- Top-level recovery run -------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryRun {
    pub repo_path: PathBuf,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub protected_staged_files: Vec<String>,
    pub stage1: Option<Stage1Result>,
    pub stage2: Option<Stage2Result>,
    pub stage3: Option<Stage3Result>,
    pub stage4: Option<Stage4AtlasRun>,
    pub blockers: Vec<RecoveryBlocker>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_status_display_roundtrip() {
        for variant in [StageStatus::Completed, StageStatus::Blocked] {
            let s = variant.to_string();
            let parsed: StageStatus = s.parse().unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn delta_verdict_display_roundtrip() {
        for v in [
            DeltaVerdict::Reduced,
            DeltaVerdict::Unchanged,
            DeltaVerdict::Replaced,
        ] {
            let s = v.to_string();
            assert_eq!(s.parse::<DeltaVerdict>().unwrap(), v);
        }
    }

    #[test]
    fn fix_verify_mode_display_roundtrip() {
        for v in [FixVerifyMode::ReadOnly, FixVerifyMode::IsolatedWorktree] {
            let s = v.to_string();
            assert_eq!(s.parse::<FixVerifyMode>().unwrap(), v);
        }
    }

    #[test]
    fn atlas_provenance_display_roundtrip() {
        for v in [
            AtlasProvenance::IsolatedWorktree,
            AtlasProvenance::CurrentTreeReadOnly,
            AtlasProvenance::Blocked,
        ] {
            let s = v.to_string();
            assert_eq!(s.parse::<AtlasProvenance>().unwrap(), v);
        }
    }

    #[test]
    fn validation_status_display_roundtrip() {
        for v in [
            ValidationStatus::Passed,
            ValidationStatus::Failed,
            ValidationStatus::Blocked,
        ] {
            let s = v.to_string();
            assert_eq!(s.parse::<ValidationStatus>().unwrap(), v);
        }
    }

    #[test]
    fn invalid_stage_status_parse() {
        assert!("bogus".parse::<StageStatus>().is_err());
    }

    #[test]
    fn recovery_blocker_construction() {
        let b = RecoveryBlocker {
            stage: 1,
            code: "GIT_DIRTY".into(),
            message: ".claude has uncommitted changes".into(),
            retryable: false,
        };
        assert_eq!(b.stage, 1);
        assert!(!b.retryable);
    }

    #[test]
    fn stage1_result_serde_roundtrip() {
        let r = Stage1Result {
            status: StageStatus::Completed,
            mode: FixVerifyMode::ReadOnly,
            protected_staged_files: vec!["a.rs".into()],
            actions: vec!["captured staged files".into()],
            blockers: vec![],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Stage1Result = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, StageStatus::Completed);
        assert_eq!(back.protected_staged_files.len(), 1);
    }

    #[test]
    fn stage2_error_signature_serde() {
        let sig = Stage2ErrorSignature {
            signature_id: "abc123".into(),
            error_type: "TypeError".into(),
            headline: "x is not a function".into(),
            normalized_location: "src/foo.py:10".into(),
            normalized_message: "x is not a function".into(),
            occurrences: 3,
        };
        let json = serde_json::to_value(&sig).unwrap();
        assert_eq!(json["occurrences"], 3);
    }

    #[test]
    fn recovery_run_serde_roundtrip() {
        let run = RecoveryRun {
            repo_path: PathBuf::from("/tmp/repo"),
            started_at: Utc::now(),
            finished_at: None,
            protected_staged_files: vec![],
            stage1: None,
            stage2: None,
            stage3: None,
            stage4: None,
            blockers: vec![],
        };
        let json = serde_json::to_string(&run).unwrap();
        let back: RecoveryRun = serde_json::from_str(&json).unwrap();
        assert_eq!(back.repo_path, PathBuf::from("/tmp/repo"));
    }
}
