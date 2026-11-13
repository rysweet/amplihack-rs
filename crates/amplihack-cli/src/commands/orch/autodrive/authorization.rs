use anyhow::{Result, bail};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// Captured before executing children; never deserialized from agent output.
#[derive(Debug, Clone, Serialize)]
pub struct InvocationAuthorization {
    schema_version: u32,
    selected_pr: Option<u64>,
    selected_url: Option<String>,
    selected_branch: Option<String>,
    policy_provenance: Option<String>,
    run_id: String,
    no_merge: bool,
    dry_run: bool,
    drive_enabled: bool,
    explicit_mode: Option<super::policy::DriveMode>,
    requested_action: &'static str,
    /// #2329: the private state directory the runner assigned and created for
    /// this invocation. Bound after admission, never taken from child output.
    #[serde(skip_serializing_if = "Option::is_none")]
    state_dir: Option<PathBuf>,
}
impl InvocationAuthorization {
    /// Capture explicit operator inputs, before recipe defaults or inference.
    /// Omitted flags use runner policy: only the full composer defaults to action.
    pub fn capture(recipe: &str, inputs: &BTreeMap<String, String>) -> Result<Self> {
        fn flag(inputs: &BTreeMap<String, String>, key: &str, default: bool) -> Result<bool> {
            match inputs.get(key).map(String::as_str) {
                None => Ok(default),
                Some("true") => Ok(true),
                Some("false") => Ok(false),
                Some(_) => bail!("{key} must be a boolean: exactly true or false"),
            }
        }
        let selected_pr = inputs
            .get("pr_number")
            .filter(|s| !s.is_empty())
            .map(|s| {
                anyhow::ensure!(
                    s.bytes().all(|b| b.is_ascii_digit()),
                    "pr_number must be a positive integer"
                );
                let number: u64 = s.parse()?;
                anyhow::ensure!(number > 0, "pr_number must be a positive integer");
                Ok(number)
            })
            .transpose()?;
        let selected_url = inputs.get("pr_url").filter(|s| !s.is_empty()).cloned();
        let selected_branch = inputs
            .get("existing_branch")
            .filter(|s| !s.is_empty())
            .cloned();
        let no_merge = flag(inputs, "no_merge", recipe != "auto-drive-to-merge")?;
        let dry_run = flag(inputs, "autodrive_dry_run", false)?;
        let drive_enabled = recipe == "auto-drive-to-merge";
        let explicit_mode = inputs
            .get("merge")
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.parse())
            .transpose()?;
        if let Some(directory) = inputs.get("autodrive_state_dir") {
            validate_state_dir(directory)?;
        }
        Ok(Self {
            schema_version: 1,
            selected_pr,
            selected_url,
            selected_branch,
            policy_provenance: None,
            run_id: uuid::Uuid::new_v4().to_string(),
            no_merge,
            dry_run,
            drive_enabled,
            explicit_mode,
            requested_action: if !drive_enabled || dry_run {
                "verify_readiness"
            } else if explicit_mode == Some(super::policy::DriveMode::Handoff) {
                "handoff"
            } else {
                "merge"
            },
            state_dir: None,
        })
    }
    pub(crate) fn validate_selection(&self, binding: &super::records::Binding) -> Result<()> {
        anyhow::ensure!(
            self.selected_pr.is_none_or(|n| n == binding.pr_number),
            "evidence changed explicitly selected PR"
        );
        anyhow::ensure!(
            self.selected_url
                .as_deref()
                .is_none_or(|url| url == binding.pr_url),
            "evidence changed explicitly selected PR URL"
        );
        anyhow::ensure!(
            self.selected_branch
                .as_deref()
                .is_none_or(|branch| branch == binding.head_branch),
            "evidence changed explicitly selected source branch"
        );
        Ok(())
    }
    pub(crate) fn restrict_dry_run(&mut self) {
        self.dry_run = true;
        self.requested_action = "verify_readiness";
    }
    pub(crate) fn bind_policy(&mut self, provenance: String) {
        self.policy_provenance = Some(provenance);
    }
    pub(crate) fn bind_state_dir(&mut self, directory: PathBuf) {
        self.state_dir = Some(directory);
    }
    pub(crate) fn state_dir(&self) -> Option<&Path> {
        self.state_dir.as_deref()
    }
    pub(crate) fn policy_provenance(&self) -> Option<&str> {
        self.policy_provenance.as_deref()
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn allows_merge(&self) -> bool {
        self.allows_drive() && self.explicit_mode != Some(super::policy::DriveMode::Handoff)
    }
    pub(crate) fn allows_drive(&self) -> bool {
        self.drive_enabled && !self.dry_run
    }
    pub(crate) fn explicit_mode(&self) -> Option<super::policy::DriveMode> {
        self.explicit_mode
    }
    pub(crate) fn mode_input(&self) -> &'static str {
        match self.explicit_mode {
            Some(super::policy::DriveMode::Merge) => "merge",
            Some(super::policy::DriveMode::Handoff) => "handoff",
            None => "",
        }
    }
    pub fn allows_followups(&self) -> bool {
        !self.dry_run
    }
    pub fn no_merge(&self) -> bool {
        self.no_merge
    }
    pub fn dry_run(&self) -> bool {
        self.dry_run
    }
}

impl InvocationAuthorization {
    /// Internal local helper entry: reads the captured private invocation, never a candidate.
    pub(crate) fn read_private(path: &std::path::Path) -> Result<Self> {
        use super::state::EvidenceStore;
        use serde::Deserialize;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Stored {
            schema_version: u32,
            selected_pr: Option<u64>,
            selected_url: Option<String>,
            selected_branch: Option<String>,
            policy_provenance: Option<String>,
            run_id: String,
            no_merge: bool,
            dry_run: bool,
            drive_enabled: bool,
            explicit_mode: Option<super::policy::DriveMode>,
            requested_action: String,
            #[serde(default)]
            state_dir: Option<PathBuf>,
        }
        let store = EvidenceStore::create(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("invocation has no parent"))?,
        )?;
        let bytes = store.read_private(
            path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("invalid invocation filename"))?,
        )?;
        let stored: Stored = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(stored.schema_version == 1, "unsupported invocation schema");
        anyhow::ensure!(
            stored
                .policy_provenance
                .as_deref()
                .and_then(|p| p.strip_prefix("trusted-launch-bundle:sha256:"))
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())),
            "invocation has no runner-pinned policy provenance"
        );
        anyhow::ensure!(
            uuid::Uuid::parse_str(&stored.run_id).is_ok(),
            "invalid invocation run_id"
        );
        let requested_action = if !stored.drive_enabled || stored.dry_run {
            "verify_readiness"
        } else if stored.explicit_mode == Some(super::policy::DriveMode::Handoff) {
            "handoff"
        } else {
            "merge"
        };
        anyhow::ensure!(
            stored.requested_action == requested_action,
            "contradictory captured invocation"
        );
        Ok(Self {
            schema_version: stored.schema_version,
            selected_pr: stored.selected_pr,
            selected_url: stored.selected_url,
            selected_branch: stored.selected_branch,
            policy_provenance: stored.policy_provenance,
            run_id: stored.run_id,
            no_merge: stored.no_merge,
            dry_run: stored.dry_run,
            drive_enabled: stored.drive_enabled,
            explicit_mode: stored.explicit_mode,
            requested_action,
            state_dir: stored.state_dir,
        })
    }
}

/// #2329 (V1): an operator-chosen state directory must be a non-empty absolute
/// path with no `..` component and no control character. The refusal is a
/// fixed message, so no operator value is echoed to the terminal.
fn validate_state_dir(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || !path.is_absolute()
        || path.components().any(|c| matches!(c, Component::ParentDir))
        || value.chars().any(char::is_control)
    {
        bail!("autodrive_state_dir must be an absolute path");
    }
    Ok(())
}
