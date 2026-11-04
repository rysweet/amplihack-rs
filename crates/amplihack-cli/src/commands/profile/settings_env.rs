//! Issue #1877: profiles inherit the `env` block of the shared
//! `settings.json` (`<shared_assets>/settings.json`, normally
//! `~/.claude/settings.json`) or of the file named by
//! `profile.settings_template`. Keys the profile already has win.
//!
//! Only `env` is read from the source and only `env` is written to the
//! profile. Nothing here fails a command: every problem becomes
//! [`Refresh::Skipped`] with one warning, and no warning ever shows an env
//! value. This module is the only writer of a profile's `env`: `env` is not
//! on the seeding allowlist, so `profile add` gets it from [`ensure_for`]
//! with the same checks as login. Unix only; elsewhere no `env` is copied.

use super::name::ProfileName;
use super::settings_refresh::Refresh;
use super::store::ProfileStore;
use std::path::Path;

/// One profile's backfill result and the warnings to print for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outcome {
    pub(super) result: Refresh,
    pub(super) warnings: Vec<String>,
}

impl Outcome {
    fn unchanged() -> Self {
        Self {
            result: Refresh::Unchanged,
            warnings: Vec::new(),
        }
    }
}

/// The warning when nothing was inherited. `reason` is an I/O error, a JSON
/// position or a fixed string, never an env value.
pub(super) fn warning(name: &str, path: &Path, reason: &str) -> String {
    format!(
        "profile {name}: env not inherited from {}: {reason}",
        path.display()
    )
}

/// Add, login, switch, repair, profile launches and rotator adoption: adds
/// missing source `env` keys to `name`'s `settings.json`, creating it when
/// missing. Existing entries of every type win, including `XPIA_ENABLED`.
/// The source is `profile.settings_template` when set, else the shared
/// `settings.json`. Returns `Unchanged` without rewriting when no keys are missing.
/// `Unchanged` with no I/O when `home` is unknown.
pub(super) fn ensure_for(store: &ProfileStore, name: &ProfileName, home: Option<&Path>) -> Outcome {
    #[cfg(unix)]
    {
        let Some(home) = home else {
            return Outcome::unchanged();
        };
        let dir = match store.resolve(name) {
            Ok(dir) => dir,
            Err(error) => {
                let path = store.root().join(name.as_str());
                return imp::skipped(name.as_str(), &path, &error.to_string());
            }
        };
        imp::backfill(&dir, name.as_str(), store.shared_assets(), home, true)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, home);
        tracing::debug!(profile = %name, "env backfill is Unix-only; skipped");
        Outcome::unchanged()
    }
}

/// [`ensure_for`] for every profile in the root. `store.list()` already
/// omits names that do not parse. `Err` is the one `Skipped` outcome for a
/// root that cannot be listed, kept apart from the per-profile results.
pub(super) fn ensure_all(
    store: &ProfileStore,
    home: Option<&Path>,
) -> Result<Vec<(ProfileName, Outcome)>, Outcome> {
    let names = store.list().map_err(|error| {
        let reason = error.to_string();
        Outcome {
            warnings: vec![warning("(all)", store.root(), &reason)],
            result: Refresh::Skipped(reason),
        }
    })?;
    Ok(names
        .into_iter()
        .filter_map(|name| ProfileName::parse(&name).ok())
        .map(|name| {
            let outcome = ensure_for(store, &name, home);
            (name, outcome)
        })
        .collect())
}

/// Prints `outcome`'s warnings to stderr, for the commands that run a
/// single profile's backfill.
pub(super) fn report(outcome: &Outcome) {
    for line in &outcome.warnings {
        eprintln!("{line}");
    }
}

// For the unit tests, which are Unix-only.
#[cfg(all(test, unix))]
pub(super) use imp::{key_warning, merge_env};

#[cfg(all(test, not(unix)))]
mod platform_tests {
    use super::*;

    #[test]
    fn backfill_ignores_the_template_and_performs_no_io_off_unix() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let config = home.join(".amplihack");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[profile]\nsettings_template = \"not-an-absolute-path\"\n",
        )
        .unwrap();
        let root = tmp.path().join("profiles-must-remain-absent");
        let store = ProfileStore::new(root.clone())
            .with_shared_assets(tmp.path().join("shared-must-remain-absent"));
        let name = ProfileName::parse("work").unwrap();
        let outcome = ensure_for(&store, &name, Some(&home));
        assert_eq!(outcome.result, Refresh::Unchanged);
        assert!(outcome.warnings.is_empty());
        assert!(!root.exists());
    }
}

#[cfg(unix)]
mod imp {
    use super::{Outcome, warning};
    use crate::commands::profile::settings_refresh::Refresh;
    use crate::commands::profile::store::MAX_SETTINGS_BYTES;
    use crate::user_config;
    use amplihack_utils::secure_files::{read_bounded_nofollow, write_private_atomic};
    use serde_json::{Map, Value};
    use std::io::{self, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    const MAX_KEY_BYTES: usize = 256;
    const CREDENTIAL_FILES: &[&str] = &[".credentials.json", ".claude.json"];
    /// Credential variables are never inherited: a shared one would
    /// override every profile's own login, which defeats rotation, and
    /// would put another copy of the secret in each profile.
    const CREDENTIAL_KEYS: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ];
    const SETTINGS: &str = "settings.json";

    /// Adds each `source` entry the profile's `env` lacks. Profile keys win;
    /// keys outside `env` and a non-object profile `env` are left alone. An empty
    /// `source` changes nothing, so no empty `env` is created. Returns whether
    /// any key was added.
    pub(in crate::commands::profile) fn merge_env(
        settings: &mut Value,
        source: &Map<String, Value>,
    ) -> bool {
        if source.is_empty() {
            return false;
        }
        let Some(settings) = settings.as_object_mut() else {
            return false;
        };
        let env = settings
            .entry("env")
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(env) = env else {
            return false;
        };
        let mut added = false;
        for (key, value) in source {
            if !env.contains_key(key) {
                env.insert(key.clone(), value.clone());
                added = true;
            }
        }
        added
    }

    /// The warning for one dropped source key. The key is escaped and cut to
    /// 64 characters, because a key that failed the name rule may hold anything.
    pub(in crate::commands::profile) fn key_warning(
        name: &str,
        key: &str,
        path: &Path,
        reason: &str,
    ) -> String {
        let shown: String = key.escape_debug().take(64).collect();
        format!(
            "profile {name}: env key {shown} not inherited from {}: {reason}",
            path.display()
        )
    }

    pub(super) fn skipped(name: &str, path: &Path, reason: &str) -> Outcome {
        Outcome {
            result: Refresh::Skipped(reason.to_string()),
            warnings: vec![warning(name, path, reason)],
        }
    }

    /// The source's usable entries, plus each dropped key and its reason.
    struct SourceEnv {
        env: Map<String, Value>,
        dropped: Vec<(String, &'static str)>,
    }

    fn euid() -> u32 {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }

    /// Shared settings may be owned by the effective user or root.
    fn source_owner_allowed(owner: u32, effective: u32) -> bool {
        owner == effective || owner == 0
    }

    /// Reads `path` without following a final symlink, at most 1 MiB, and
    /// parses it into a JSON value. `Ok(None)` when it does not exist.
    /// `owner_ok` judges the owner's uid.
    fn read_json(
        path: &Path,
        owner_ok: &dyn Fn(u32) -> bool,
        check_mode: bool,
    ) -> Result<Option<Value>, String> {
        let (raw, meta) = match read_bounded_nofollow(path, MAX_SETTINGS_BYTES) {
            Ok(Some(found)) => found,
            Ok(None) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                return Err(classify(path));
            }
            Err(error) => return Err(error.to_string()),
        };
        if !owner_ok(meta.uid()) {
            return Err("file is not owned by the current user".into());
        }
        if check_mode && meta.mode() & 0o022 != 0 {
            return Err(format!(
                "file is writable by group or others; fix it with `chmod 600 {}`",
                path.display()
            ));
        }
        serde_json::from_slice(&raw).map(Some).map_err(|error| {
            format!(
                "invalid JSON at line {} column {}",
                error.line(),
                error.column()
            )
        })
    }

    /// The fixed reason for a file `read_bounded_nofollow` refused.
    fn classify(path: &Path) -> String {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => "file is a symlink".into(),
            Ok(meta) if !meta.is_file() => "file is not a regular file".into(),
            Ok(_) => "file is larger than 1 MiB".into(),
            Err(error) => error.to_string(),
        }
    }

    /// The source's `env`: `Ok(None)` when there is nothing to inherit.
    /// A template must exist; a missing default source is not an error.
    fn source_env(path: &Path, template: bool) -> Result<Option<SourceEnv>, String> {
        if template
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| CREDENTIAL_FILES.contains(&n))
        {
            return Err("refusing credential file".into());
        }
        let owner_ok = |uid: u32| source_owner_allowed(uid, euid());
        let Some(settings) = read_json(path, &owner_ok, true)? else {
            return if template {
                Err("file does not exist".into())
            } else {
                Ok(None)
            };
        };
        let Value::Object(mut settings) = settings else {
            return Err("source settings.json is not a JSON object".into());
        };
        let env = match settings.remove("env") {
            None => return Ok(None),
            Some(Value::Object(env)) => env,
            Some(_) => return Err("source env is not an object".into()),
        };
        let mut out = SourceEnv {
            env: Map::new(),
            dropped: Vec::new(),
        };
        for (key, value) in env {
            if !valid_key(&key) {
                out.dropped.push((key, "invalid variable name"));
            } else if CREDENTIAL_KEYS.contains(&key.as_str()) {
                out.dropped
                    .push((key, "credential variable; set it in the profile itself"));
            } else if !value.is_string() {
                out.dropped.push((key, "value is not a string"));
            } else {
                out.env.insert(key, value);
            }
        }
        Ok(Some(out))
    }

    /// `^[A-Za-z_][A-Za-z0-9_]*$`, at most 256 bytes.
    fn valid_key(key: &str) -> bool {
        let mut bytes = key.bytes();
        key.len() <= MAX_KEY_BYTES
            && bytes
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The profile's own `settings.json`: `Ok(None)` when missing; an object
    /// otherwise, whose `env` (if any) is an object.
    fn read_profile(path: &Path) -> Result<Option<Map<String, Value>>, String> {
        let owner_ok = |uid: u32| uid == euid();
        match read_json(path, &owner_ok, false)? {
            None => Ok(None),
            Some(Value::Object(settings)) => {
                if settings.get("env").is_some_and(|env| !env.is_object()) {
                    return Err("profile env is not an object".into());
                }
                Ok(Some(settings))
            }
            Some(_) => Err("profile settings.json is not a JSON object".into()),
        }
    }

    /// Replaces the profile's `settings.json` atomically with mode 0600.
    fn write_profile(dir: &Path, settings: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        write_private_atomic(dir, SETTINGS, &bytes).map_err(|e| e.to_string())
    }

    /// Creates a missing `settings.json` holding only `env`. `AlreadyExists`
    /// is returned as is, so the caller can retry as an update. A partial
    /// write removes the file this call created. This uses create-new rather
    /// than `write_private_atomic`, because an atomic rename would overwrite
    /// a file another process created at the same moment.
    fn create_with_env(dir: &Path, env: &Map<String, Value>) -> io::Result<()> {
        let mut root = Map::new();
        root.insert("env".into(), Value::Object(env.clone()));
        let mut bytes = serde_json::to_vec_pretty(&root).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let path = dir.join(SETTINGS);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        // Creation mode is filtered by umask. Set exact permissions on the
        // opened descriptor before writing; failures use partial-file cleanup.
        let written = file
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .and_then(|()| file.write_all(&bytes))
            .and_then(|()| file.sync_all());
        if let Err(error) = written {
            drop(file);
            if let Err(cleanup) = std::fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), %cleanup, "could not remove partial settings.json");
            }
            return Err(error);
        }
        Ok(())
    }

    /// The configured source: the template when set, else the shared
    /// `settings.json`. `Err` holds the warning path and reason; `Ok(None)`
    /// means there is no source.
    fn source_path(
        shared: Option<&Path>,
        home: &Path,
    ) -> Result<Option<(PathBuf, bool)>, (PathBuf, String)> {
        match user_config::profile_settings_template(home) {
            Ok(Some(template)) => Ok(Some((template, true))),
            Ok(None) => Ok(shared.map(|dir| (dir.join(SETTINGS), false))),
            Err(reason) => Err((config_file(home), reason)),
        }
    }

    fn config_file(home: &Path) -> PathBuf {
        home.join(".amplihack").join("config.toml")
    }

    fn key_warnings(name: &str, path: &Path, src: &SourceEnv) -> Vec<String> {
        src.dropped
            .iter()
            .map(|(key, reason)| key_warning(name, key, path, reason))
            .collect()
    }

    /// One profile's backfill. `may_retry` allows one retry as an update
    /// when another process creates `settings.json` first.
    pub(super) fn backfill(
        dir: &Path,
        name: &str,
        shared: Option<&Path>,
        home: &Path,
        may_retry: bool,
    ) -> Outcome {
        let profile_path = dir.join(SETTINGS);
        let profile = match read_profile(&profile_path) {
            Ok(profile) => profile,
            Err(reason) => return skipped(name, &profile_path, &reason),
        };
        let (path, template) = match source_path(shared, home) {
            Ok(Some(found)) => found,
            Ok(None) => return Outcome::unchanged(),
            Err((path, reason)) => return skipped(name, &path, &reason),
        };
        let src = match source_env(&path, template) {
            Ok(Some(src)) => src,
            Ok(None) => return Outcome::unchanged(),
            Err(reason) => return skipped(name, &path, &reason),
        };
        let warnings = key_warnings(name, &path, &src);
        let done = |result| Outcome {
            result,
            warnings: warnings.clone(),
        };
        let Some(settings) = profile else {
            if src.env.is_empty() {
                return done(Refresh::Unchanged);
            }
            return match create_with_env(dir, &src.env) {
                Ok(()) => done(Refresh::Updated),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists && may_retry => {
                    backfill(dir, name, shared, home, false)
                }
                Err(error) => skipped(name, &profile_path, &error.to_string()),
            };
        };
        let mut merged = Value::Object(settings);
        if !merge_env(&mut merged, &src.env) {
            return done(Refresh::Unchanged);
        }
        match write_profile(dir, &merged) {
            Ok(()) => done(Refresh::Updated),
            Err(reason) => skipped(name, &profile_path, &reason),
        }
    }

    #[cfg(test)]
    mod ownership_tests {
        #[test]
        fn source_owner_policy_accepts_only_effective_user_or_root() {
            // Synthetic UIDs exercise the production predicate without chown.
            for (owner, effective, allowed) in [
                (501, 501, true),
                (0, 501, true),
                (502, 501, false),
                (u32::MAX, 501, false),
                (0, 0, true),
                (501, 0, false),
            ] {
                assert_eq!(
                    super::source_owner_allowed(owner, effective),
                    allowed,
                    "owner={owner}, effective={effective}"
                );
            }
        }
    }
}
