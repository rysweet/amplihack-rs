//! Individual assertions retain failures and dependencies without inventing success.
use super::environment::{confined_path, observe_git, private_dir};
use super::hosting::HostingStore;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const CASES: &[&str] = &[
    "build",
    "finalization-valid",
    "finalization-missing",
    "finalization-unreadable",
    "drive-control",
    "drive-missing",
    "cleanup-interrupted",
    "cleanup-other-stopped",
    "cleanup-clean",
    "cleanup-removal-failure",
];
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Regression,
    Harness,
    Isolation,
    Prerequisite,
    MalformedResponse,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Pass,
    Fail,
    Xfail {
        issue: u64,
    },
    NotExecuted {
        blocking_assertion: String,
        reason: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allowance {
    pub assertion_id: String,
    pub issue: u64,
    pub base_sha: String,
    pub evidence_path: PathBuf,
    pub failure_fingerprint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assertion {
    case: String,
    id: String,
    passed: bool,
    failure: Option<FailureKind>,
    fingerprint: String,
    blocked: Option<(String, String)>,
}
impl Assertion {
    pub fn passed(case: &str, id: &str) -> Self {
        Self {
            case: case.into(),
            id: id.into(),
            passed: true,
            failure: None,
            fingerprint: String::new(),
            blocked: None,
        }
    }
    pub fn failed(case: &str, id: &str, kind: FailureKind, fingerprint: &str) -> Self {
        Self {
            passed: false,
            failure: Some(kind),
            fingerprint: fingerprint.into(),
            ..Self::passed(case, id)
        }
    }
    pub fn blocked(case: &str, id: &str, dependency: &str, reason: &str) -> Self {
        Self {
            passed: false,
            blocked: Some((dependency.into(), reason.into())),
            ..Self::passed(case, id)
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    root: PathBuf,
    tested_sha: String,
    allowances: Vec<Allowance>,
    assertions: BTreeMap<String, (Assertion, Outcome)>,
}
impl Report {
    pub fn new(root: &Path, tested_sha: &str) -> Result<Self> {
        private_dir(root)?;
        Ok(Self {
            root: root.into(),
            tested_sha: tested_sha.into(),
            allowances: vec![],
            assertions: BTreeMap::new(),
        })
    }
    pub fn open(root: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(
            root.join("report.json"),
        )?)?)
    }
    pub fn add_allowance(&mut self, allowance: Allowance) -> Result<()> {
        ensure!(
            [2802, 2785, 2343].contains(&allowance.issue),
            "issue cannot receive a regression allowance"
        );
        let baseline: Value = serde_json::from_slice(&fs::read(confined_path(
            &self.root,
            &allowance.evidence_path,
        )?)?)?;
        ensure!(
            baseline["tested_sha"] == allowance.base_sha
                && baseline["assertion_id"] == allowance.assertion_id
                && baseline["issue"] == allowance.issue
                && baseline["failure_fingerprint"] == allowance.failure_fingerprint,
            "baseline binding does not match allowance"
        );
        ensure!(
            baseline["executed"] == true
                && baseline["prerequisites_satisfied"] == true
                && baseline["failure_kind"] == "regression"
                && baseline["observations"].is_object(),
            "baseline lacks behavioral proof"
        );
        self.allowances.push(allowance);
        Ok(())
    }
    pub fn classify(&self, assertion: Assertion) -> Result<Outcome> {
        if assertion.passed {
            return Ok(Outcome::Pass);
        }
        if let Some((blocking_assertion, reason)) = assertion.blocked {
            return Ok(Outcome::NotExecuted {
                blocking_assertion,
                reason,
            });
        }
        if assertion.failure == Some(FailureKind::Regression)
            && let Some(allowance) = self.allowances.iter().find(|a| {
                a.assertion_id == assertion.id && a.failure_fingerprint == assertion.fingerprint
            })
        {
            return Ok(Outcome::Xfail {
                issue: allowance.issue,
            });
        }
        Ok(Outcome::Fail)
    }
    pub fn record(&mut self, assertion: Assertion) -> Result<()> {
        ensure!(
            CASES.contains(&assertion.case.as_str())
                && assertion.id.starts_with(&format!("{}.", assertion.case)),
            "assertion is not bound to a required case"
        );
        if let Some((dependency, _)) = &assertion.blocked {
            ensure!(
                dependency.starts_with(&format!("{}.", assertion.case)),
                "cross-case blocking is forbidden"
            );
        }
        ensure!(
            !self.assertions.contains_key(&assertion.id),
            "duplicate assertion {}",
            assertion.id
        );
        let outcome = self.classify(assertion.clone())?;
        println!(
            "{}: {}",
            match &outcome {
                Outcome::Pass => "PASS".into(),
                Outcome::Fail => "FAIL".into(),
                Outcome::Xfail { issue } => format!("XFAIL #{issue}"),
                Outcome::NotExecuted { .. } => "NOT_EXECUTED".into(),
            },
            assertion.id
        );
        self.assertions
            .insert(assertion.id.clone(), (assertion, outcome));
        Ok(())
    }
    pub fn outcome(&self, id: &str) -> Result<Outcome> {
        self.assertions
            .get(id)
            .map(|(_, outcome)| outcome.clone())
            .ok_or_else(|| anyhow::anyhow!("missing assertion {id}"))
    }
    pub fn persist(&self) -> Result<()> {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(self)?,
        )?;
        Ok(())
    }
    pub fn success(&self) -> bool {
        let required = [
            "build.terminal",
            "build.open-pr",
            "build.commit",
            "build.content",
            "build.origin",
            "build.pr-head",
            "build.complete",
            "drive-control.caller-dirty",
            "drive-control.gates",
            "drive-control.retained",
            "drive-control.worktree",
            "drive-control.merge",
            "drive-control.complete",
            "drive-missing.caller-dirty",
            "drive-missing.gates",
            "drive-missing.retained",
            "drive-missing.complete",
            "drive-missing.worktree",
            "drive-missing.merge",
            "finalization-valid.classification",
            "finalization-missing.classification",
            "finalization-unreadable.classification",
            "cleanup-interrupted.commit",
            "cleanup-interrupted.dirty",
            "cleanup-interrupted.explanation",
            "cleanup-interrupted.continuation",
            "cleanup-other-stopped.commit",
            "cleanup-other-stopped.dirty",
            "cleanup-other-stopped.explanation",
            "cleanup-other-stopped.continuation",
            "cleanup-clean.removed",
            "cleanup-removal-failure.reported-once",
        ];
        required.iter().all(|id| self.assertions.contains_key(*id))
            && ["build.complete", "drive-control.complete"]
                .iter()
                .all(|id| matches!(self.outcome(id), Ok(Outcome::Pass)))
            && self.assertions.values().all(|(_, outcome)| match outcome {
                Outcome::Pass | Outcome::Xfail { .. } => true,
                Outcome::Fail => false,
                Outcome::NotExecuted {
                    blocking_assertion, ..
                } => matches!(self.outcome(blocking_assertion), Ok(Outcome::Xfail { .. })),
            })
    }
}
#[derive(Debug, Serialize)]
pub struct BuildObservation {
    pub local_committed: bool,
    pub remote_matches: bool,
    pub open_pr_matches: bool,
}
pub fn observe_build(
    checkout: &Path,
    origin: &Path,
    hosting: &HostingStore,
    number: u64,
    file: &str,
) -> Result<BuildObservation> {
    let head = observe_git(checkout, &["rev-parse", "HEAD"])?;
    let branch = observe_git(checkout, &["branch", "--show-current"])?;
    let pr = hosting.pr(number)?;
    let content = observe_git(checkout, &["show", &format!("HEAD:{file}")])?;
    let remote = observe_git(origin, &["rev-parse", &format!("refs/heads/{branch}")])?;
    let changed = observe_git(
        checkout,
        &["diff", "--name-only", "main...HEAD", "--", file],
    )?;
    Ok(BuildObservation {
        local_committed: !content.is_empty() && !changed.is_empty(),
        remote_matches: remote == head,
        open_pr_matches: pr.state == "OPEN"
            && pr.head_sha == head
            && pr.branch == branch
            && hosting.open_prs()?.len() == 1,
    })
}
#[derive(Debug, Deserialize, Serialize)]
pub struct PendingFinalization {
    pub context: String,
    pub context_read_error: Option<String>,
    pub fault_before_consumption: bool,
    pub origin_head: String,
    pub pr_head: String,
    pub pr_state: String,
    pub checks: String,
    pub terminal_state: String,
    pub terminal_success: bool,
    pub exit_code: i32,
}
pub fn validate_pending_prerequisites(observed: &PendingFinalization) -> Result<()> {
    ensure!(
        observed.origin_head == observed.pr_head
            && observed.pr_state == "OPEN"
            && observed.checks == "pending",
        "pending case lacks a current pushed open PR"
    );
    match observed.context.as_str() {
        "valid" => ensure!(
            observed.context_read_error.is_none(),
            "valid context was unreadable"
        ),
        "missing" => ensure!(
            observed.fault_before_consumption
                && observed.context_read_error.as_deref() == Some("NotFound"),
            "missing context fault was not observed before consumption"
        ),
        "unreadable" => ensure!(
            observed.fault_before_consumption
                && observed.context_read_error.as_deref() == Some("PermissionDenied"),
            "unreadable context fault was not observed before consumption"
        ),
        _ => anyhow::bail!("unknown context case"),
    }
    Ok(())
}
pub fn validate_pending_finalization(observed: &PendingFinalization) -> Result<()> {
    validate_pending_prerequisites(observed)?;
    ensure!(
        observed.terminal_state == "BLOCKED_CI"
            && !observed.terminal_success
            && observed.exit_code == 1,
        "pending checks must produce consistent BLOCKED_CI, observed {} / {}",
        observed.terminal_state,
        observed.exit_code
    );
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct RetentionObservation {
    pub commit_reachable: bool,
    pub dirty_content_intact: bool,
    pub branch_usable: bool,
    pub unpushed: bool,
}
pub fn observe_retention(
    checkout: &Path,
    origin: &Path,
    commit: &str,
    dirty_path: &str,
    content: &[u8],
) -> Result<RetentionObservation> {
    let branch = observe_git(checkout, &["branch", "--show-current"])?;
    let head = observe_git(checkout, &["rev-parse", "HEAD"])?;
    let ancestors = observe_git(checkout, &["rev-list", &head])?;
    let remote = observe_git(origin, &["rev-parse", &format!("refs/heads/{branch}")])?;
    let remote_ancestors = observe_git(origin, &["rev-list", &remote])?;
    let dirty = match fs::read(confined_path(checkout, Path::new(dirty_path))?) {
        Ok(bytes) => bytes == content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    Ok(RetentionObservation {
        commit_reachable: ancestors.lines().any(|sha| sha == commit),
        dirty_content_intact: dirty,
        branch_usable: !branch.is_empty(),
        unpushed: !remote_ancestors.lines().any(|sha| sha == commit),
    })
}
#[derive(Debug, Serialize, Deserialize)]
pub struct DriveObservation {
    pub caller_common_dir: PathBuf,
    pub private_common_dir: PathBuf,
    pub execution_common_dir: PathBuf,
    pub launch_checkout: PathBuf,
    pub resolved_worktree: PathBuf,
    pub observed_head: String,
    pub worktree_head: String,
    pub worktree_branch: Option<String>,
    pub registered_linked_worktree: bool,
}
pub fn validate_drive_location(observed: &DriveObservation) -> Result<()> {
    ensure!(
        observed.caller_common_dir != observed.private_common_dir
            && observed.execution_common_dir == observed.private_common_dir,
        "execution did not enter the private repository"
    );
    ensure!(
        observed.launch_checkout != observed.resolved_worktree
            && observed.registered_linked_worktree
            && observed
                .worktree_branch
                .as_ref()
                .is_some_and(|b| !b.is_empty()),
        "detached launch checkout is not a PR worktree"
    );
    ensure!(
        observed.observed_head == observed.worktree_head,
        "worktree does not match observed PR head"
    );
    Ok(())
}
