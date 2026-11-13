//! Process-local launch identity, independent of credential storage.
use super::{Account, Bound, Path, Reader};
#[cfg(target_os = "macos")]
use super::{BUDGET_SPENT, LOGIN_CHANGED, PathBuf};
#[cfg(target_os = "macos")]
#[path = "launch_keychain.rs"]
mod keychain;

/// Launch-only proof. Credential contents and continuity never leave this
/// process, and Debug deliberately reveals neither storage nor identity.
#[derive(Clone)]
pub(in crate::commands::profile) enum LaunchBound {
    File(Bound),
    #[cfg(target_os = "macos")]
    Keychain {
        dir: PathBuf,
        storage: keychain::Snapshot,
        account: Account,
    },
}
impl std::fmt::Debug for LaunchBound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LaunchBound(<redacted>)")
    }
}
impl LaunchBound {
    pub fn account(&self) -> Option<&Account> {
        match self {
            Self::File(bound) => bound.account(),
            #[cfg(target_os = "macos")]
            Self::Keychain { account, .. } => Some(account),
        }
    }
    /// Local storage checks only; called with state and refresh protection.
    pub fn current(&self) -> bool {
        match self {
            Self::File(bound) => {
                #[cfg(target_os = "macos")]
                if !matches!(keychain::read(&bound.dir), Ok(None)) {
                    return false;
                }
                bound.current()
            }
            #[cfg(target_os = "macos")]
            Self::Keychain { dir, storage, .. } => {
                keychain::read(dir).is_ok_and(|now| now.as_ref() == Some(storage))
            }
        }
    }
}

/// Authenticate the storage Claude will read for a named launch. Keychain is
/// primary on macOS; an inaccessible entry never authorises a file fallback.
pub(in crate::commands::profile) fn launch_bound(
    reader: &Reader<'_>,
    dir: &Path,
    platform: bool,
) -> Result<LaunchBound, &'static str> {
    if let Some(custom) = std::env::var_os("CLAUDE_CODE_CUSTOM_OAUTH_URL").filter(|v| !v.is_empty())
    {
        // A separate OAuth namespace must not send its credentials to the
        // default identity server. Reuse the explicitly configured, guarded
        // transport only when it names that same namespace.
        let (endpoint, _) = crate::commands::profile::usage_direct::Endpoints::from_env();
        if !endpoint.matches_oauth_namespace(&custom) {
            return Err(super::UNAVAILABLE);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(storage) = keychain::read(dir)? {
        use crate::commands::profile::usage_direct::{self, Endpoints};
        if !reader.budget.available() {
            return Err(BUDGET_SPENT);
        }
        let (ep, _) = Endpoints::from_env();
        let account = usage_direct::account_bytes(
            &ep,
            storage.credentials(),
            super::super::cred_identity::identity_read_timeout(),
            chrono::Utc::now().timestamp_millis(),
        )?;
        if keychain::read(dir)?.as_ref() != Some(&storage) {
            return Err(LOGIN_CHANGED);
        }
        return Ok(LaunchBound::Keychain {
            dir: dir.to_path_buf(),
            storage,
            account,
        });
    }
    reader.bound(dir, platform).map(LaunchBound::File)
}
