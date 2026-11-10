//! Fail-closed merge policy for the trusted auto-drive gate.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Command};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}
impl MergeMethod {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "merge" => Ok(Self::Merge),
            "squash" => Ok(Self::Squash),
            "rebase" => Ok(Self::Rebase),
            _ => bail!("merge_method must be exactly merge, squash, or rebase"),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SelectionSource {
    Explicit,
    Repository,
    Default,
}
#[derive(Deserialize)]
pub struct AllowedMergeMethods {
    allow_merge_commit: bool,
    allow_squash_merge: bool,
    allow_rebase_merge: bool,
}
impl AllowedMergeMethods {
    fn allows(&self, method: MergeMethod) -> bool {
        match method {
            MergeMethod::Merge => self.allow_merge_commit,
            MergeMethod::Squash => self.allow_squash_merge,
            MergeMethod::Rebase => self.allow_rebase_merge,
        }
    }
}
#[derive(Serialize)]
pub struct MergeSelection {
    method: MergeMethod,
    source: SelectionSource,
    rationale: String,
}

fn repository_method(root: &Path) -> Result<Option<MergeMethod>> {
    let root = root
        .canonicalize()
        .context("cannot resolve repository root")?;
    anyhow::ensure!(root.is_dir(), "repository root is not a directory");
    let github = root.join(".github");
    // Resolve the parent independently, including dangling or escaping symlinks.
    match fs::symlink_metadata(&github) {
        Ok(_) => anyhow::ensure!(
            github
                .canonicalize()
                .context("cannot resolve .github")?
                .starts_with(&root),
            ".github escapes repository root"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("cannot inspect .github"),
    }
    let path = github.join("amplihack.yml");
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("cannot inspect .github/amplihack.yml"),
    }
    let path = path
        .canonicalize()
        .context("cannot resolve .github/amplihack.yml")?;
    anyhow::ensure!(
        path.starts_with(&root),
        "amplihack.yml escapes repository root"
    );
    anyhow::ensure!(
        fs::metadata(&path)
            .context("cannot inspect resolved amplihack.yml")?
            .is_file(),
        "amplihack.yml must be a regular file"
    );
    let text = fs::read_to_string(path).context("cannot read .github/amplihack.yml")?;
    // serde_yaml's Mapping deserializer rejects duplicate keys and multiple documents.
    let document: serde_yaml::Value = serde_yaml::from_str(&text)
        .context("invalid .github/amplihack.yml: expected one mapping with unique keys")?;
    let mapping = document
        .as_mapping()
        .context("invalid .github/amplihack.yml: expected a top-level mapping")?;
    mapping
        .get(serde_yaml::Value::String("merge_method".into()))
        .map(|value| {
            MergeMethod::parse(
                value
                    .as_str()
                    .context("repository merge_method must be a string")?,
            )
        })
        .transpose()
}

#[derive(Debug, thiserror::Error)]
#[error("gh api refused repository merge permissions (terminal policy exit 79)")]
pub(super) struct PolicyRefusal;

/// Select an allowed method, validating repository configuration even on override.
pub fn select(repository: &str, root: &Path, explicit: &str) -> Result<MergeSelection> {
    let (owner, repo) = repository
        .split_once('/')
        .context("repository must be a canonical OWNER/REPO identity")?;
    anyhow::ensure!(
        !owner.is_empty()
            && owner.len() <= 39
            && !owner.starts_with('-')
            && !owner.ends_with('-')
            && owner
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            && !repo.is_empty()
            && repo.len() <= 100
            && repo != "."
            && repo != ".."
            && repo
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)),
        "repository must be a canonical OWNER/REPO identity"
    );
    let explicit = if explicit.is_empty() {
        None
    } else {
        Some(MergeMethod::parse(explicit)?)
    };
    let configured = repository_method(root)?;
    let output = Command::new("gh")
        .args(["api", &format!("repos/{repository}")])
        .output()
        .context("cannot execute gh api for merge permissions")?;
    if output.status.code() == Some(79) {
        return Err(PolicyRefusal.into());
    }
    anyhow::ensure!(
        output.status.success(),
        "gh api could not read repository merge permissions (status {})",
        output.status
    );
    let allowed: AllowedMergeMethods = serde_json::from_slice(&output.stdout).context(
        "invalid repository merge permissions: all three allow_* fields must be booleans",
    )?;
    choose(explicit, configured, allowed)
}

fn choose(
    explicit: Option<MergeMethod>,
    configured: Option<MergeMethod>,
    allowed: AllowedMergeMethods,
) -> Result<MergeSelection> {
    let (method, source, reason) = if let Some(method) = explicit {
        (method, SelectionSource::Explicit, "explicit recipe context")
    } else if let Some(method) = configured {
        (
            method,
            SelectionSource::Repository,
            "repository .github/amplihack.yml preference",
        )
    } else {
        let method = [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase]
            .into_iter()
            .find(|m| allowed.allows(*m))
            .context("repository allows no merge methods")?;
        (
            method,
            SelectionSource::Default,
            "first allowed method in squash, merge, rebase order",
        )
    };
    anyhow::ensure!(
        allowed.allows(method),
        "selected merge method {} is disallowed by repository permissions",
        method.name()
    );
    Ok(MergeSelection {
        method,
        source,
        rationale: format!(
            "Selected {} from {reason}; repository permissions allow it",
            method.name()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_configuration_requires_a_mapping_and_unique_keys() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".github")).unwrap();
        for text in [
            "",
            "null",
            "[]",
            "merge_method: merge\nmerge_method: rebase\n",
            "merge_method: merge\n---\nmerge_method: squash",
        ] {
            fs::write(root.path().join(".github/amplihack.yml"), text).unwrap();
            assert!(repository_method(root.path()).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn defaults_are_deterministic_for_all_permission_combinations() {
        for mask in 0..8 {
            let allowed = AllowedMergeMethods {
                allow_merge_commit: mask & 1 != 0,
                allow_squash_merge: mask & 2 != 0,
                allow_rebase_merge: mask & 4 != 0,
            };
            let result = choose(None, None, allowed);
            if mask == 0 {
                assert!(result.is_err());
            } else {
                let selection = result.unwrap();
                assert_eq!(selection.source, SelectionSource::Default);
                assert_eq!(
                    selection.method,
                    if mask & 2 != 0 {
                        MergeMethod::Squash
                    } else if mask & 1 != 0 {
                        MergeMethod::Merge
                    } else {
                        MergeMethod::Rebase
                    }
                );
                assert!(selection.rationale.contains(selection.method.name()));
            }
        }
    }

    #[test]
    fn explicit_override_does_not_validate_lower_priority_permission() {
        let selection = choose(
            Some(MergeMethod::Merge),
            Some(MergeMethod::Rebase),
            AllowedMergeMethods {
                allow_merge_commit: true,
                allow_squash_merge: false,
                allow_rebase_merge: false,
            },
        )
        .unwrap();
        assert_eq!(selection.method, MergeMethod::Merge);
        assert_eq!(selection.source, SelectionSource::Explicit);
    }

    #[test]
    fn disallowed_preferences_never_fall_back() {
        for explicit in [false, true] {
            let selection = choose(
                explicit.then_some(MergeMethod::Merge),
                Some(MergeMethod::Merge),
                AllowedMergeMethods {
                    allow_merge_commit: false,
                    allow_squash_merge: true,
                    allow_rebase_merge: true,
                },
            );
            assert!(selection.is_err());
        }
    }
}
