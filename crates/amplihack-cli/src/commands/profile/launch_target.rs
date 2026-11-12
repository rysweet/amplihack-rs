//! A destination carries its own join permission through resolution and spawn.
//! Supervisor contexts contain only names, paths and non-secret evidence.

use super::{ProfileError, ProfileStore, auto_switch, name::ProfileName};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) const JOIN_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_JOIN";

pub(crate) const CONTEXT_ENV: &str = "AMPLIHACK_CLAUDE_LAUNCH_CONTEXT";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionOrigin {
    Explicit,
    Inherited,
    Default,
    Supervisor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JoinPolicy {
    Never,
    ExistingSession,
}

#[derive(Clone, Debug)]
pub(crate) enum LaunchTarget {
    Ambient,
    Profile(Box<NamedTarget>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NamedTarget {
    format_version: u32,
    profile: String,
    directory: PathBuf,
    root: PathBuf,
    join_policy: JoinPolicy,
    origin: SelectionOrigin,
    #[cfg(unix)]
    evidence: Option<auto_switch::launch_fallback::Evidence>,
    fallback: Option<(String, String)>,
    per_token: Option<String>,
}

fn invalid(reason: impl Into<String>) -> ProfileError {
    ProfileError::LaunchDestination(reason.into())
}

/// An attempt or direct launch owns its private hand-off until its child exits.
pub(crate) struct LaunchContext(Option<PathBuf>);
impl LaunchContext {
    pub(super) fn new(path: Option<PathBuf>) -> Self {
        Self(path)
    }
    pub(crate) fn apply(&self, cmd: &mut Command) {
        if let Some(path) = &self.0 {
            cmd.env(CONTEXT_ENV, path);
        }
    }
}
impl Drop for LaunchContext {
    fn drop(&mut self) {
        if let Some(path) = &self.0
            && let Err(e) = std::fs::remove_file(path)
        {
            eprintln!("warning: could not remove launch context: {}", e.kind());
        }
    }
}

impl LaunchTarget {
    pub(crate) fn named(
        store: &ProfileStore,
        name: &ProfileName,
        join_policy: JoinPolicy,
        origin: SelectionOrigin,
    ) -> Result<Self, ProfileError> {
        let directory = store.resolve(name)?;
        let root = directory
            .parent()
            .ok_or_else(|| invalid("destination has no profile root"))?
            .to_path_buf();
        Ok(Self::Profile(Box::new(NamedTarget {
            format_version: 1,
            profile: name.to_string(),
            directory,
            root,
            join_policy,
            origin,
            #[cfg(unix)]
            evidence: None,
            fallback: None,
            per_token: None,
        })))
    }

    /// Compatibility for callers already selecting an explicit directory.
    pub(crate) fn profile(dir: PathBuf) -> Result<Self, ProfileError> {
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| invalid("invalid destination directory"))?;
        let name = ProfileName::parse(name)?;
        let root = dir
            .parent()
            .ok_or_else(|| invalid("destination has no root"))?;
        let target = Self::named(
            &ProfileStore::new(root.to_path_buf()),
            &name,
            JoinPolicy::Never,
            SelectionOrigin::Explicit,
        )?;
        if target.dir() != Some(dir.as_path()) {
            return Err(invalid("destination directory changed"));
        }
        Ok(target)
    }

    pub(crate) fn dir(&self) -> Option<&Path> {
        match self {
            Self::Ambient => None,
            Self::Profile(t) => Some(&t.directory),
        }
    }

    pub(crate) fn name(&self) -> Option<ProfileName> {
        match self {
            Self::Ambient => None,
            Self::Profile(t) => ProfileName::parse(&t.profile).ok(),
        }
    }

    #[cfg(unix)]
    pub(super) fn set_evidence(
        &mut self,
        evidence: auto_switch::launch_fallback::Evidence,
        fallback: Option<(String, String)>,
        per_token: Option<String>,
    ) {
        if let Self::Profile(t) = self {
            t.evidence = Some(evidence);
            t.fallback = fallback;
            t.per_token = per_token;
        }
    }

    #[cfg(unix)]
    pub(super) fn pinned(
        store: &ProfileStore,
        name: &ProfileName,
        source: &str,
        per_token: Option<String>,
    ) -> Result<Self, ProfileError> {
        let mut target = Self::named(store, name, JoinPolicy::Never, SelectionOrigin::Supervisor)?;
        let evidence = auto_switch::launch_fallback::pin(store, name, source)?;
        target.set_evidence(evidence, None, per_token);
        Ok(target)
    }

    fn is_pinned(&self) -> bool {
        #[cfg(unix)]
        if let Self::Profile(t) = self {
            return t.evidence.is_some();
        }
        false
    }

    #[cfg(unix)]
    pub(super) fn is_fallback(&self) -> bool {
        matches!(self, Self::Profile(t) if t.fallback.is_some())
    }

    /// Applied after all builders: an ambient directory cannot replace this
    /// destination, and descendants retain the same identity and root.
    pub(crate) fn apply(&self, cmd: &mut Command) {
        if let Self::Profile(t) = self {
            // Attempts restore their own context after this application;
            // actual children keep one only when evidence must be pinned.
            cmd.env_remove(CONTEXT_ENV);
            let restrict_descendants = t.origin != SelectionOrigin::Explicit
                && (std::env::var_os(JOIN_ENV).as_deref() == Some(std::ffi::OsStr::new("never"))
                    || self.is_pinned());
            if restrict_descendants {
                cmd.env(JOIN_ENV, "never");
            } else {
                cmd.env_remove(JOIN_ENV);
            }
            cmd.env("CLAUDE_CONFIG_DIR", &t.directory)
                .env(super::PROFILE_ENV, &t.profile)
                .env(super::ROOT_ENV, &t.root);
        }
    }

    /// Owned protection lasts through environment application and child creation.
    pub(crate) fn prepare_launch(&self) -> Result<Option<auto_switch::LaunchHold>, ProfileError> {
        self.prepare().map_err(|error| match self {
            Self::Profile(t) if t.fallback.is_some() => {
                let (profile, live) = t.fallback.as_ref().expect("fallback present");
                ProfileError::DefaultConflict {
                    profile: profile.clone(),
                    live: live.clone(),
                    source: Some(Box::new(error)),
                }
            }
            _ => error,
        })
    }

    fn prepare(&self) -> Result<Option<auto_switch::LaunchHold>, ProfileError> {
        let Self::Profile(t) = self else {
            return Ok(None);
        };
        let name = ProfileName::parse(&t.profile)?;
        let store = ProfileStore::from_env()?;
        if store.root().canonicalize()? != t.root {
            return Err(invalid("destination root changed"));
        }
        let join = t.join_policy == JoinPolicy::ExistingSession
            && auto_switch::inside_session(&t.directory);
        let hold = auto_switch::guard_launch(&store, &name, join)?;
        let verified = store.verify_profile_dir(&name)?;
        if verified != t.directory {
            return Err(invalid("destination directory changed"));
        }
        #[cfg(unix)]
        let mut hold = hold;
        #[cfg(unix)]
        {
            super::kind_guard(
                &store,
                &name,
                super::usage_cmd::home_dir().as_deref(),
                false,
            )?;
            if let Some(evidence) = &t.evidence {
                if t.join_policy != JoinPolicy::Never {
                    return Err(invalid("pinned destination cannot join another session"));
                }
                hold.protect_refresh(&t.directory)?;
                auto_switch::launch_fallback::validate(&store, &name, evidence)?;
            }
        }
        super::shared_dirs::ensure_for(&store, &name);
        Ok(Some(hold))
    }

    /// Direct fallback launches also pass pinned evidence to descendants.
    /// This uses the existing private scratch area, under the launch hold.
    pub(crate) fn child_context(&self) -> anyhow::Result<LaunchContext> {
        #[cfg(unix)]
        if let Self::Profile(t) = self
            && t.evidence.is_some()
        {
            let state = auto_switch::kit::State::new(&t.root)?;
            let dir = state.dir.join("rerun");
            state.ensure_sub(&dir)?;
            return Ok(LaunchContext::new(self.write_context(&dir, 0)?));
        }
        Ok(LaunchContext::new(None))
    }

    /// Printed only after protected validation has succeeded.
    pub(crate) fn report(&self) {
        if let Self::Profile(t) = self {
            if let Some((default, hosted)) = &t.fallback {
                eprintln!(
                    "amplihack: default profile '{default}' hosts the login of profile '{hosted}' for a running session; using profile '{}' for this launch",
                    t.profile
                );
            }
            #[cfg(unix)]
            auto_switch::launch_fallback::record(&t.root, t.per_token.as_deref());
        }
    }

    /// Load a private supervisor hand-off. A context never bypasses the
    /// authoritative profile, lease and protected evidence checks at spawn.
    #[cfg(unix)]
    pub(super) fn from_context() -> Result<Option<Self>, ProfileError> {
        use std::os::unix::fs::MetadataExt;
        let Some(path) = std::env::var_os(CONTEXT_ENV).filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let path = PathBuf::from(path);
        let parent = path
            .parent()
            .ok_or_else(|| invalid("launch context has no parent"))?;
        super::fsx::check_private_dir(parent)
            .map_err(|_| invalid("launch context directory is unsafe"))?;
        let (bytes, meta) = super::fsx::read_bounded_nofollow(&path, 64 * 1024)
            .map_err(|_| invalid("launch context cannot be read"))?
            .ok_or_else(|| invalid("launch context is missing"))?;
        if meta.uid() != super::fsx::euid() || meta.mode() & 0o077 != 0 || meta.nlink() != 1 {
            return Err(invalid("launch context is not a private owned file"));
        }
        let t: NamedTarget =
            serde_json::from_slice(&bytes).map_err(|_| invalid("launch context is malformed"))?;
        let name = ProfileName::parse(&t.profile)
            .map_err(|_| invalid("launch context profile is invalid"))?;
        if t.format_version != 1
            || (t.join_policy == JoinPolicy::ExistingSession
                && t.origin != SelectionOrigin::Inherited)
            || (t.evidence.is_some() && t.join_policy != JoinPolicy::Never)
            || (t.fallback.is_some()
                && (t.evidence.is_none() || t.origin != SelectionOrigin::Default))
        {
            return Err(invalid(
                "launch context version or join permission is invalid",
            ));
        }
        if let Some((default, hosted)) = &t.fallback {
            ProfileName::parse(default)
                .map_err(|_| invalid("launch context default is invalid"))?;
            ProfileName::parse(hosted)
                .map_err(|_| invalid("launch context hosted login is invalid"))?;
        }
        let store = ProfileStore::from_env()?;
        if std::env::var_os(super::PROFILE_ENV).as_deref() != Some(std::ffi::OsStr::new(&t.profile))
            || std::env::var_os("CLAUDE_CONFIG_DIR").as_deref() != Some(t.directory.as_os_str())
            || std::env::var_os(super::ROOT_ENV).as_deref() != Some(t.root.as_os_str())
            || store.root().canonicalize()? != t.root
            || store.verify_profile_dir(&name)? != t.directory
        {
            return Err(invalid("launch context destination tuple is inconsistent"));
        }
        Ok(Some(Self::Profile(Box::new(t))))
    }

    #[cfg(unix)]
    pub(super) fn write_context(
        &self,
        dir: &Path,
        attempt: u32,
    ) -> anyhow::Result<Option<PathBuf>> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Self::Profile(t) = self else {
            return Ok(None);
        };
        super::fsx::check_private_dir(dir)?;
        let path = dir.join(format!("{}-{attempt}.launch.json", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let result = (|| -> anyhow::Result<()> {
            file.write_all(&serde_json::to_vec(t)?)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(e) = result {
            std::fs::remove_file(&path)?;
            return Err(e);
        }
        Ok(Some(path))
    }
}
