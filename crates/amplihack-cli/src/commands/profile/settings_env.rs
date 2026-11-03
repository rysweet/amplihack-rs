//! Issue #1877: profiles inherit the `env` block of the shared
//! `settings.json` (`<shared_assets>/settings.json`, normally
//! `~/.claude/settings.json`) or of the file named by
//! `profile.settings_template`. Keys the profile already has win.
//!
//! Only `env` is read from the source and only `env` is written to the
//! profile. Nothing here fails a command: every problem becomes
//! [`Refresh::Skipped`] with one warning, and no warning ever shows an env
//! value. Unix only; elsewhere the backfill is a logged no-op and seeding
//! alone copies `env`.

use super::name::ProfileName;
use super::settings_refresh::Refresh;
use super::store::ProfileStore;
use serde_json::{Map, Value};
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

/// Adds each `source` entry the profile's `env` lacks. Profile keys win;
/// keys outside `env` and a non-object profile `env` are left alone. An empty
/// `source` changes nothing, so no empty `env` is created.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn merge_env(settings: &mut Value, source: &Map<String, Value>) {
    if source.is_empty() {
        return;
    }
    let Some(settings) = settings.as_object_mut() else {
        return;
    };
    let env = settings
        .entry("env")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(env) = env else {
        return;
    };
    for (key, value) in source {
        if !env.contains_key(key) {
            env.insert(key.clone(), value.clone());
        }
    }
}

/// The warning when nothing was inherited. `reason` is an I/O error, a JSON
/// position or a fixed string, never an env value.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn warning(name: &str, path: &Path, reason: &str) -> String {
    format!(
        "profile {name}: env not inherited from {}: {reason}",
        path.display()
    )
}

/// The warning for one dropped source key. The key is escaped and cut to
/// 64 characters, because a key that failed the name rule may hold anything.
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) fn key_warning(name: &str, key: &str, path: &Path, reason: &str) -> String {
    let shown: String = key.escape_debug().take(64).collect();
    format!(
        "profile {name}: env key {shown} not inherited from {}: {reason}",
        path.display()
    )
}

/// Login, switch and repair: adds the missing source `env` keys to `name`'s
/// `settings.json`, creating it when missing. Runs only when the profile has
/// no `env.XPIA_ENABLED`; otherwise `Unchanged` without opening the source.
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
        imp::backfill(&dir, name.as_str(), store.shared_assets(), home)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, home);
        tracing::debug!(profile = %name, "env backfill is Unix-only; skipped");
        Outcome::unchanged()
    }
}

/// [`ensure_for`] for every profile in the root, by name. Names that do not
/// parse are left out.
pub(super) fn ensure_all(store: &ProfileStore, home: Option<&Path>) -> Vec<(String, Outcome)> {
    let names = match store.list() {
        Ok(names) => names,
        Err(error) => {
            tracing::debug!(%error, "env backfill: profile root unreadable");
            return Vec::new();
        }
    };
    names
        .into_iter()
        .filter_map(|name| {
            let parsed = ProfileName::parse(&name).ok()?;
            Some((name, ensure_for(store, &parsed, home)))
        })
        .collect()
}

/// `profile add`: when `profile.settings_template` is set, replaces the
/// seeded `env` of the profile at `dir` with the template's. Without a
/// template the seeded file is left alone. A template that cannot be used
/// leaves the profile with no `env` and one warning.
pub(super) fn after_add(dir: &Path, home: Option<&Path>) -> Outcome {
    #[cfg(unix)]
    {
        match home {
            Some(home) => imp::apply_template(dir, home),
            None => Outcome::unchanged(),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        tracing::debug!(dir = %dir.display(), "env template is Unix-only; skipped");
        Outcome::unchanged()
    }
}

/// Prints `outcome`'s warnings to stderr, for the commands that run a
/// single profile's backfill.
pub(super) fn report(outcome: &Outcome) {
    for line in &outcome.warnings {
        eprintln!("{line}");
    }
}

#[cfg(unix)]
mod imp {
    use super::{Outcome, key_warning, merge_env, warning};
    use crate::commands::profile::settings_refresh::Refresh;
    use crate::user_config;
    use amplihack_utils::secure_files::{read_bounded_nofollow, write_private_atomic};
    use serde_json::{Map, Value};
    use std::io::{self, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    const MAX_BYTES: u64 = 1024 * 1024;
    const MAX_KEY_BYTES: usize = 256;
    const CREDENTIAL_FILES: &[&str] = &[".credentials.json", ".claude.json"];
    const SETTINGS: &str = "settings.json";

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

    /// Reads `path` without following a final symlink, at most 1 MiB, and
    /// parses it into a JSON value. `Ok(None)` when it does not exist.
    /// `owner_ok` judges the owner's uid.
    fn read_json(
        path: &Path,
        owner_ok: &dyn Fn(u32) -> bool,
        check_mode: bool,
    ) -> Result<Option<Value>, String> {
        let (raw, meta) = match read_bounded_nofollow(path, MAX_BYTES) {
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
            return Err("file is writable by group or others".into());
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
        let owner_ok = |uid: u32| uid == euid() || uid == 0;
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
    fn write_profile(dir: &Path, settings: &Map<String, Value>) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        write_private_atomic(dir, SETTINGS, &bytes).map_err(|e| e.to_string())
    }

    /// Creates a missing `settings.json` holding only `env`. `AlreadyExists`
    /// is returned as is, so the caller can retry as an update. A partial
    /// write removes the file this call created.
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
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
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

    pub(super) fn backfill(dir: &Path, name: &str, shared: Option<&Path>, home: &Path) -> Outcome {
        backfill_once(dir, name, shared, home, true)
    }

    fn backfill_once(
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
        // The XPIA gate: a profile that already sets it is left alone, and
        // the source is not opened.
        if profile
            .as_ref()
            .and_then(|s| s.get("env"))
            .and_then(Value::as_object)
            .is_some_and(|env| env.contains_key("XPIA_ENABLED"))
        {
            return Outcome::unchanged();
        }
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
                    backfill_once(dir, name, shared, home, false)
                }
                Err(error) => skipped(name, &profile_path, &error.to_string()),
            };
        };
        let mut value = Value::Object(settings.clone());
        merge_env(&mut value, &src.env);
        let Value::Object(merged) = value else {
            return done(Refresh::Unchanged);
        };
        if merged == settings {
            return done(Refresh::Unchanged);
        }
        match write_profile(dir, &merged) {
            Ok(()) => done(Refresh::Updated),
            Err(reason) => skipped(name, &profile_path, &reason),
        }
    }

    /// `after_add`: with a template, the seeded `env` is removed and the
    /// template's merged in. A broken template still removes the seeded
    /// `env`, so a misconfiguration never passes as inherited.
    pub(super) fn apply_template(dir: &Path, home: &Path) -> Outcome {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (path, src) = match user_config::profile_settings_template(home) {
            Ok(None) => return Outcome::unchanged(),
            Ok(Some(template)) => {
                let src = source_env(&template, true);
                (template, src)
            }
            Err(reason) => (config_file(home), Err(reason)),
        };
        let profile_path = dir.join(SETTINGS);
        let profile = match read_profile(&profile_path) {
            Ok(profile) => profile,
            Err(reason) => return skipped(&name, &profile_path, &reason),
        };
        let (env, warnings, failure) = match src {
            Ok(Some(src)) => (src.env.clone(), key_warnings(&name, &path, &src), None),
            Ok(None) => (Map::new(), Vec::new(), None),
            Err(reason) => (Map::new(), Vec::new(), Some(reason)),
        };
        let written = match profile {
            None if env.is_empty() => Ok(Refresh::Unchanged),
            None => create_with_env(dir, &env)
                .map(|()| Refresh::Updated)
                .map_err(|e| e.to_string()),
            Some(before) => {
                let mut settings = before.clone();
                settings.remove("env");
                if !env.is_empty() {
                    settings.insert("env".into(), Value::Object(env));
                }
                if settings == before {
                    Ok(Refresh::Unchanged)
                } else {
                    write_profile(dir, &settings).map(|()| Refresh::Updated)
                }
            }
        };
        let result = match written {
            Ok(result) => result,
            Err(reason) => return skipped(&name, &profile_path, &reason),
        };
        if let Some(reason) = failure {
            return skipped(&name, &path, &reason);
        }
        Outcome { result, warnings }
    }
}
