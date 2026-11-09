//! `--codex-home-link` / `AMPLIHACK_ROTATE_CODEX_HOME_LINK`: the default
//! Codex home link. Kept out of the rotate `Settings`, which stays `Copy`.

use crate::commands::profile::ProfileError;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

pub(in crate::commands::profile::rotate) const LINK_FLAG: &str = "--codex-home-link";
const LINK_VAR: &str = "AMPLIHACK_ROTATE_CODEX_HOME_LINK";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::commands::profile::rotate) struct CodexSettings {
    pub link: PathBuf,
}

/// The link: the flag, else the variable (empty is unset), else
/// `<home>/.codex`. It must be absolute with no `..` component.
pub(in crate::commands::profile::rotate) fn resolve_codex(
    flag: Option<&str>,
    env: &dyn Fn(&str) -> Option<OsString>,
    home: &Path,
) -> Result<CodexSettings, ProfileError> {
    let (name, raw) = match flag {
        Some(f) => (LINK_FLAG, OsString::from(f)),
        None => match env(LINK_VAR).filter(|v| !v.is_empty()) {
            Some(v) => (LINK_VAR, v),
            None => {
                return Ok(CodexSettings {
                    link: home.join(".codex"),
                });
            }
        },
    };
    let link = PathBuf::from(&raw);
    if !link.is_absolute() || link.components().any(|c| c == Component::ParentDir) {
        let text = raw.to_string_lossy();
        let head: String = text.chars().take(32).collect();
        return Err(ProfileError::RotateConfig(format!(
            "invalid rotate setting {name} {head:?}: expected an absolute path with no .. component"
        )));
    }
    Ok(CodexSettings { link })
}
