//! PR recovery readiness helpers.
//!
//! These pure functions support default-workflow recovery of an existing PR by
//! separating workflow readiness from merge readiness. They deliberately do not
//! perform GitHub mutations or launch workflows.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrHeadSnapshot {
    pub expected_head_sha: String,
    pub local_head_sha: String,
    pub pr_head_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPrHead {
    pub head_sha: String,
    pub message: String,
}

pub fn verify_pr_head(snapshot: &PrHeadSnapshot) -> Result<VerifiedPrHead> {
    let expected = snapshot.expected_head_sha.trim();
    let local = snapshot.local_head_sha.trim();
    let pr = snapshot.pr_head_sha.trim();

    if expected.is_empty() {
        bail!("blocked: expected_head_sha is empty");
    }
    if local != expected {
        bail!("blocked: local HEAD {local} does not match expected_head_sha {expected}");
    }
    if pr != expected {
        bail!("blocked: PR headRefOid {pr} does not match expected_head_sha {expected}");
    }

    Ok(VerifiedPrHead {
        head_sha: expected.to_string(),
        message: format!("local HEAD == PR headRefOid == expected_head_sha ({expected})"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledHook {
    pub namespace: String,
    pub file_name: String,
}

impl InstalledHook {
    pub fn new(namespace: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            file_name: file_name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistration {
    pub event: String,
    pub command: String,
}

impl HookRegistration {
    pub fn new(event: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            event: event.into(),
            command: command.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReadinessInput {
    pub installed_hooks: Vec<InstalledHook>,
    pub native_registrations: Vec<HookRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessBlocker {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReadiness {
    pub workflow_ready: bool,
    pub blockers: Vec<ReadinessBlocker>,
}

pub fn inspect_hook_readiness(input: &HookReadinessInput) -> HookReadiness {
    let mut blockers = Vec::new();

    for &(namespace, file_name) in required_installed_hooks() {
        if !input
            .installed_hooks
            .iter()
            .any(|hook| hook.namespace == namespace && hook.file_name == file_name)
        {
            blockers.push(ReadinessBlocker {
                code: "MISSING_INSTALLED_HOOK",
                message: format!("missing installed hook {namespace}/{file_name}"),
            });
        }
    }

    for &(event, command_fragment) in required_native_registrations() {
        if !input.native_registrations.iter().any(|registration| {
            registration.event == event && registration.command.contains(command_fragment)
        }) {
            blockers.push(ReadinessBlocker {
                code: "MISSING_NATIVE_HOOK_REGISTRATION",
                message: format!(
                    "missing native hook registration for {event}: command containing {command_fragment}"
                ),
            });
        }
    }

    HookReadiness {
        workflow_ready: blockers.is_empty(),
        blockers,
    }
}

fn required_installed_hooks() -> &'static [(&'static str, &'static str)] {
    &[
        ("amplihack", "PreToolUse.js"),
        ("amplihack", "PostToolUse.js"),
        ("amplihack", "Stop.js"),
        ("amplihack", "SessionStart.js"),
        ("amplihack", "SessionStop.js"),
        ("amplihack", "UserPromptSubmit.js"),
        ("amplihack", "PreCompact.js"),
        ("xpia", "PreToolUse.js"),
    ]
}

fn required_native_registrations() -> &'static [(&'static str, &'static str)] {
    &[
        ("PreToolUse", "amplihack-hooks pre-tool-use"),
        ("PostToolUse", "amplihack-hooks post-tool-use"),
        ("Stop", "amplihack-hooks stop"),
        ("SessionStart", "amplihack-hooks session-start"),
        ("SessionStop", "amplihack-hooks session-stop"),
        (
            "UserPromptSubmit",
            "amplihack-hooks workflow-classification-reminder",
        ),
        ("UserPromptSubmit", "amplihack-hooks user-prompt-submit"),
        ("PreCompact", "amplihack-hooks pre-compact"),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveCopyEntry {
    pub relative_path: PathBuf,
    pub destination_exists: bool,
}

impl AdditiveCopyEntry {
    pub fn file(relative_path: impl Into<PathBuf>, destination_exists: bool) -> Self {
        Self {
            relative_path: relative_path.into(),
            destination_exists,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveCopyPlan {
    pub destination_root: PathBuf,
    pub entries: Vec<AdditiveCopyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveCopyAction {
    pub relative_path: PathBuf,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveCopyReadiness {
    pub workflow_ready: bool,
    pub actions: Vec<String>,
    planned_actions: Vec<AdditiveCopyAction>,
}

impl AdditiveCopyReadiness {
    pub fn action_for(&self, relative_path: &str) -> Option<&str> {
        self.planned_actions
            .iter()
            .find(|planned| planned.relative_path == Path::new(relative_path))
            .map(|planned| planned.action.as_str())
    }
}

pub fn inspect_additive_copy_plan(plan: &AdditiveCopyPlan) -> Result<AdditiveCopyReadiness> {
    if plan.destination_root.as_os_str().is_empty() {
        bail!("additive copy destination root is empty");
    }

    let mut action_names = Vec::with_capacity(plan.entries.len());
    let mut planned_actions = Vec::with_capacity(plan.entries.len());

    for entry in &plan.entries {
        validate_additive_relative_path(&entry.relative_path)?;
        let action = if entry.destination_exists {
            "skip-existing"
        } else {
            "copy-new"
        };

        action_names.push(action.to_string());
        planned_actions.push(AdditiveCopyAction {
            relative_path: entry.relative_path.clone(),
            action: action.to_string(),
        });
    }

    Ok(AdditiveCopyReadiness {
        workflow_ready: true,
        actions: action_names,
        planned_actions,
    })
}

fn validate_additive_relative_path(path: &Path) -> Result<()> {
    if path.is_absolute() {
        bail!(
            "absolute path is not allowed in additive copy plan: {}",
            path.display()
        );
    }
    if path.as_os_str().is_empty() {
        bail!("empty path is not allowed in additive copy plan");
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => bail!(
                "path traversal is not allowed in additive copy plan: {}",
                path.display()
            ),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "absolute path is not allowed in additive copy plan: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Completed,
    InProgress,
    Queued,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Pending,
    Failure,
    Skipped,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRollup {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: CheckConclusion,
}

impl CheckRollup {
    pub fn new(name: impl Into<String>, status: CheckStatus, conclusion: CheckConclusion) -> Self {
        Self {
            name: name.into(),
            status,
            conclusion,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeState {
    Blocked,
    Clean,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoOpReportInput {
    pub head: PrHeadSnapshot,
    pub checks: Vec<CheckRollup>,
    pub merge_state: MergeState,
    pub hook_ready: bool,
    pub additive_copy_ready: bool,
    pub files_modified: Vec<PathBuf>,
    pub manual_merge_performed: bool,
    pub merge_bypass_performed: bool,
    pub nested_default_workflow_launched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoOpReport {
    pub workflow_ready: bool,
    pub merge_ready: bool,
    pub head_sha: String,
    pub no_op_justification: String,
    pub manual_merge_performed: bool,
    pub merge_bypass_performed: bool,
    pub nested_default_workflow_launched: bool,
}

pub fn render_no_op_report(input: NoOpReportInput) -> Result<NoOpReport> {
    if input.manual_merge_performed {
        bail!("manual merge is prohibited during PR recovery");
    }
    if input.merge_bypass_performed {
        bail!("merge bypass is prohibited during PR recovery");
    }
    if input.nested_default_workflow_launched {
        bail!("nested default-workflow invocation is prohibited during PR recovery");
    }
    if !input.hook_ready {
        bail!("hook readiness is not satisfied");
    }
    if !input.additive_copy_ready {
        bail!("additive-copy readiness is not satisfied");
    }
    if !input.files_modified.is_empty() {
        bail!(
            "no-op report requires no workflow-owned file modifications; got {} modified files",
            input.files_modified.len()
        );
    }

    let verified_head = verify_pr_head(&input.head)?;
    let lint_format_green = check_success(&input.checks, &["Lint & Format", "Lint/Format"]);
    let builds_green = check_success(&input.checks, &["build", "Build"]);
    let test_in_progress = check_in_progress(&input.checks, "Test");

    if !lint_format_green {
        bail!("no-op report blocked: Lint/Format is not green");
    }
    if !builds_green {
        bail!("no-op report blocked: builds are not green");
    }

    let merge_ready = matches!(input.merge_state, MergeState::Clean)
        && input.checks.iter().all(|check| {
            check.status == CheckStatus::Completed
                && matches!(
                    check.conclusion,
                    CheckConclusion::Success | CheckConclusion::Skipped | CheckConclusion::Neutral
                )
        });

    let test_phrase = if test_in_progress {
        "Test in progress"
    } else if check_success(&input.checks, &["Test"]) {
        "Test green"
    } else {
        "Test not green"
    };
    let merge_phrase = match input.merge_state {
        MergeState::Blocked => "merge blocked",
        MergeState::Clean => "merge clean",
        MergeState::Unknown(_) => "merge state unknown",
    };

    Ok(NoOpReport {
        workflow_ready: true,
        merge_ready,
        head_sha: verified_head.head_sha.clone(),
        no_op_justification: format!(
            "No-op justification: head {head}; Lint/Format green; builds green; {test_phrase}; {merge_phrase}; hook/additive-copy readiness satisfied; no manual merge, merge bypass, or nested default-workflow performed.",
            head = verified_head.head_sha
        ),
        manual_merge_performed: false,
        merge_bypass_performed: false,
        nested_default_workflow_launched: false,
    })
}

fn check_success(checks: &[CheckRollup], names: &[&str]) -> bool {
    checks.iter().any(|check| {
        names
            .iter()
            .any(|name| check.name.eq_ignore_ascii_case(name))
            && check.status == CheckStatus::Completed
            && matches!(
                check.conclusion,
                CheckConclusion::Success | CheckConclusion::Skipped | CheckConclusion::Neutral
            )
    })
}

fn check_in_progress(checks: &[CheckRollup], name: &str) -> bool {
    checks.iter().any(|check| {
        check.name.eq_ignore_ascii_case(name)
            && matches!(
                check.status,
                CheckStatus::InProgress | CheckStatus::Queued | CheckStatus::Pending
            )
    })
}
