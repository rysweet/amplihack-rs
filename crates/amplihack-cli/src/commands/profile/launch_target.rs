//! A destination carries its own join permission through resolution and spawn.
//! Supervisor contexts contain intent only; each launching process authenticates anew.

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
    #[serde(skip)]
    evidence: Option<auto_switch::launch_fallback::Evidence>,
    policy_source: Option<String>,
    fallback: Option<(String, String)>,
    per_token: Option<String>,
}

fn invalid(reason: impl Into<String>) -> ProfileError {
    ProfileError::LaunchDestination(reason.into())
}

/// An attempt or direct launch owns its private hand-off until its child exits.
pub(crate) struct LaunchContext(Option<ContextFile>);
struct ContextFile {
    path: PathBuf,
    file: std::fs::File,
}
impl LaunchContext {
    pub(super) fn empty() -> Self {
        Self(None)
    }
    pub(crate) fn apply(&self, cmd: &mut Command) {
        if let Some(owned) = &self.0 {
            cmd.env(CONTEXT_ENV, &owned.path);
        }
    }
}
impl Drop for ContextFile {
    fn drop(&mut self) {
        let result = (|| -> std::io::Result<()> {
            let current = match std::fs::symlink_metadata(&self.path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let original = self.file.metadata()?;
                if (current.dev(), current.ino()) != (original.dev(), original.ino()) {
                    return Err(std::io::Error::other(
                        "launch context was replaced; left for its owner",
                    ));
                }
            }
            #[cfg(not(unix))]
            let _ = current;
            std::fs::remove_file(&self.path)
        })();
        if let Err(e) = result {
            eprintln!("warning: could not remove owned launch context: {e}");
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
            format_version: 2,
            profile: name.to_string(),
            directory,
            root,
            join_policy,
            origin,
            #[cfg(unix)]
            evidence: None,
            policy_source: None,
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
    pub(super) fn set_billing(&mut self, line: Option<String>) {
        if let Self::Profile(t) = self {
            t.per_token = line;
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
            t.policy_source = Some(evidence.source_host().to_string());
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
        initial: &Self,
    ) -> Result<Self, ProfileError> {
        let mut target = Self::named(store, name, JoinPolicy::Never, SelectionOrigin::Supervisor)?;
        let fallback = match initial {
            Self::Profile(t) => t.fallback.clone(),
            Self::Ambient => None,
        };
        let evidence = auto_switch::launch_fallback::pin(store, name, source)
            .map_err(|error| initial.contextualise(error))?;
        target.set_evidence(evidence, fallback, per_token);
        Ok(target)
    }

    pub(in crate::commands::profile) fn is_pinned(&self) -> bool {
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
            if self.is_pinned() {
                // Claude's secure-storage override must agree with this login.
                cmd.env("CLAUDE_SECURESTORAGE_CONFIG_DIR", &t.directory);
                // Environment builders must not choose another credential
                // namespace after this process authenticated the destination.
                match std::env::var_os("CLAUDE_CODE_CUSTOM_OAUTH_URL") {
                    Some(value) => {
                        cmd.env("CLAUDE_CODE_CUSTOM_OAUTH_URL", value);
                    }
                    None => {
                        cmd.env_remove("CLAUDE_CODE_CUSTOM_OAUTH_URL");
                    }
                }
            }
        }
    }

    /// Owned protection lasts through environment application and child creation.
    pub(crate) fn prepare_launch(&self) -> Result<Option<auto_switch::LaunchHold>, ProfileError> {
        self.prepare().map_err(|error| self.contextualise(error))
    }

    fn contextualise(&self, error: ProfileError) -> ProfileError {
        match self {
            Self::Profile(t) if t.fallback.is_some() => {
                let (profile, live) = t.fallback.as_ref().expect("fallback present");
                ProfileError::DefaultConflict {
                    profile: profile.clone(),
                    live: live.clone(),
                    source: Some(Box::new(error)),
                }
            }
            _ => error,
        }
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
            && std::env::var_os(JOIN_ENV).as_deref() != Some(std::ffi::OsStr::new("never"))
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
            if let Some(evidence) = &t.evidence {
                if t.join_policy != JoinPolicy::Never {
                    return Err(invalid("pinned destination cannot join another session"));
                }
                if t.origin == SelectionOrigin::Default
                    && t.fallback
                        .as_ref()
                        .is_some_and(|(host, login)| !evidence.names_conflict(host, login))
                {
                    return Err(invalid("default source lease changed since selection"));
                }
                hold.protect_refresh(&t.directory)?;
                auto_switch::launch_fallback::validate(&store, &name, evidence)?;
                // An authenticated own Keychain OAuth login is equivalent to
                // an own OAuth file; uncertainty still refuses subscription.
                super::kind::launch_guard(&store, &name, None, || evidence.own_oauth())?;
            } else {
                super::kind_guard(
                    &store,
                    &name,
                    super::usage_cmd::home_dir().as_deref(),
                    false,
                )?;
            }
        }
        super::shared_dirs::ensure_for(&store, &name);
        Ok(Some(hold))
    }

    /// Direct fallback launches also pass restricted intent to descendants.
    /// This uses the existing private scratch area, under the launch hold.
    pub(crate) fn child_context(&self) -> anyhow::Result<LaunchContext> {
        self.make_child_context()
            .map_err(|error| self.handoff_error(error))
    }

    pub(in crate::commands::profile) fn handoff_error(
        &self,
        error: anyhow::Error,
    ) -> anyhow::Error {
        super::with_hint(
            self.contextualise(invalid(format!("private launch hand-off failed: {error}"))),
        )
    }

    fn make_child_context(&self) -> anyhow::Result<LaunchContext> {
        #[cfg(unix)]
        if let Self::Profile(t) = self
            && t.evidence.is_some()
        {
            let state = auto_switch::kit::State::new(&t.root)?;
            let dir = state.dir.join("rerun");
            state.ensure_sub(&dir)?;
            return self.write_context(&dir, 0);
        }
        Ok(LaunchContext::empty())
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
        let mut t: NamedTarget =
            serde_json::from_slice(&bytes).map_err(|_| invalid("launch context is malformed"))?;
        let name = ProfileName::parse(&t.profile)
            .map_err(|_| invalid("launch context profile is invalid"))?;
        let restricted =
            std::env::var_os(JOIN_ENV).as_deref() == Some(std::ffi::OsStr::new("never"));
        if t.format_version != 2
            // Only an actual explicit CLI flag may clear inherited limits;
            // launch_target ignores context when that flag is present.
            || t.origin == SelectionOrigin::Explicit
            || (restricted && (t.policy_source.is_none() || t.join_policy != JoinPolicy::Never))
            || (t.join_policy == JoinPolicy::ExistingSession
                && t.origin != SelectionOrigin::Inherited)
            || (t.policy_source.is_some() && t.join_policy != JoinPolicy::Never)
            || (t.fallback.is_some()
                && (t.policy_source.is_none()
                    || !matches!(
                        t.origin,
                        SelectionOrigin::Default | SelectionOrigin::Supervisor
                    )))
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
        // The hand-off carries no authority to join a lease or reuse an
        // earlier process's credential proof. Obtain fresh evidence here,
        // before acquiring either launch lock in prepare_launch.
        if let Some(source) = &t.policy_source {
            ProfileName::parse(source).map_err(|_| invalid("launch context source is invalid"))?;
            if t.origin == SelectionOrigin::Default
                && t.fallback.as_ref().is_some_and(|(host, _)| host != source)
            {
                return Err(invalid("launch context source is inconsistent"));
            }
            t.evidence = Some(
                auto_switch::launch_fallback::pin(&store, &name, source).map_err(
                    |error| match &t.fallback {
                        Some((profile, live)) => ProfileError::DefaultConflict {
                            profile: profile.clone(),
                            live: live.clone(),
                            source: Some(Box::new(error)),
                        },
                        None => error,
                    },
                )?,
            );
        }
        Ok(Some(Self::Profile(Box::new(t))))
    }

    #[cfg(unix)]
    pub(super) fn write_context(&self, dir: &Path, attempt: u32) -> anyhow::Result<LaunchContext> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Self::Profile(t) = self else {
            return Ok(LaunchContext::empty());
        };
        super::fsx::check_private_dir(dir)?;
        let path = dir.join(format!("{}-{attempt}.launch.json", std::process::id()));
        let bytes = serde_json::to_vec(t)?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        // Keep the create_new handle from creation through cleanup. Reopening
        // a path here could mistake a replacement file for one we own.
        let mut owned = ContextFile { path, file };
        owned.file.write_all(&bytes)?;
        owned.file.sync_all()?;
        Ok(LaunchContext(Some(owned)))
    }
}

#[cfg(all(test, unix))]
mod tests_context_ownership {
    use super::{ContextFile, LaunchContext};
    use std::fs;

    #[test]
    fn cleanup_removes_only_the_context_owned_by_this_launch() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launch.json");
        use std::io::Write;
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(b"owned").unwrap();
        let context = LaunchContext(Some(ContextFile {
            path: path.clone(),
            file,
        }));
        let parked = temp.path().join("parked.json");
        fs::rename(&path, &parked).unwrap();
        fs::write(&path, b"replacement").unwrap();
        drop(context);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(fs::read(&parked).unwrap(), b"owned");
        let fresh = temp.path().join("fresh.json");
        let file = fs::File::create(&fresh).unwrap();
        let context = LaunchContext(Some(ContextFile {
            path: fresh.clone(),
            file,
        }));
        drop(context);
        assert!(!fresh.exists());
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }
}
