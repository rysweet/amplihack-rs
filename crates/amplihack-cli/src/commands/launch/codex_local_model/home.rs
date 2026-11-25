//! The generated Codex home on disk: reading the inputs, building or
//! refreshing the home, and verifying a home a nested launch reuses. Unix
//! only.
//!
//! Layout: `<home>/.amplihack/codex-local/<name>/` holding the four files
//! amplihack owns (`config.toml`, `hooks.json`, `instructions.md`,
//! `rules/amplihack-dangerous-commands.rules`). Everything else in the home
//! (sessions, databases, logs, an `auth.json` from a login there) belongs to
//! Codex and is never listed, changed or deleted.
//!
//! A refresh checks and computes everything before its first write: the
//! hooks binary, `~/.amplihack`, the directories it would create, the
//! previous `config.toml` and the current state of each owned file. A file
//! is rewritten only when its bytes differ or it is not a private regular
//! file, through a new 0600 temporary file renamed into place, so an
//! unchanged home keeps every inode. No lock is taken: the output depends
//! only on the inputs, so concurrent launches write the same bytes.
//!
//! Paths in messages go through `sanitize_path`, so a path holding a newline
//! or an escape sequence cannot forge a line or drive the terminal. No
//! message quotes a value from the profile, the user's `hooks.json` or a
//! generated file.

use super::{LocalHome, ProfileName, config, hooks_json};
use crate::commands::install::paths::{home_dir, sanitize_path};
use amplihack_utils::secure_files::{
    check_private_dir, check_private_file, dir_writable_by_others, ensure_private_dir, euid,
    owned_by_euid, read_bounded, read_bounded_nofollow, writable_by_others, write_private_atomic,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use toml::Table;

/// The model instructions every generated home points Codex at.
const INSTRUCTIONS: &str = include_str!("instructions.md");
/// The most amplihack reads from any input or owned file.
const READ_LIMIT: u64 = 1024 * 1024;
const CONFIG: &str = "config.toml";
const HOOKS: &str = "hooks.json";
const INSTRUCTIONS_FILE: &str = "instructions.md";
const RULES_DIR: &str = "rules";
const RULES_FILE: &str = "amplihack-dangerous-commands.rules";
const HOOKS_OVERRIDE: &str = "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH";

/// Where a local-model launch's files are.
#[derive(Debug)]
struct Layout {
    /// `<home>/.amplihack`.
    amplihack: PathBuf,
    /// `<home>/.amplihack/codex-local`.
    codex_local: PathBuf,
    /// `<home>/.amplihack/codex-local/<name>`, the `CODEX_HOME` Codex gets.
    generated: PathBuf,
}

impl Layout {
    fn new(home: &Path, name: &ProfileName) -> Self {
        let amplihack = home.join(".amplihack");
        let codex_local = amplihack.join("codex-local");
        let generated = codex_local.join(name.as_str());
        Self {
            amplihack,
            codex_local,
            generated,
        }
    }

    fn rules(&self) -> PathBuf {
        self.generated.join(RULES_DIR)
    }
}

/// What [`Plan::refresh`] does.
#[derive(Debug)]
enum Kind {
    /// A nested launch inside a session on this home: verify, write nothing.
    Reuse,
    /// Build or refresh the home from the profile and the user's groups.
    Build {
        profile: Table,
        user_groups: Vec<Value>,
    },
}

/// A local-model launch's inputs, read and checked. Nothing written.
#[derive(Debug)]
pub(super) struct Plan {
    layout: Layout,
    kind: Kind,
}

impl Plan {
    /// Resolves the user Codex home and decides on reuse; otherwise
    /// reads, parses and checks the profile, then reads and filters the
    /// user's `hooks.json`. Nothing is written.
    pub(super) fn prepare(name: &ProfileName) -> Result<Self> {
        let home = home_dir()?;
        let layout = Layout::new(&home, name);
        if reuses(&layout.generated) {
            return Ok(Self {
                layout,
                kind: Kind::Reuse,
            });
        }
        let user_codex = user_codex_home(&home, &layout.codex_local);

        let profile_path = user_codex.join(format!("{}.config.toml", name.as_str()));
        let profile_text = read_followed(&profile_path)?.ok_or_else(|| {
            anyhow!(
                "no profile for local model \"{}\": expected {}",
                name.as_str(),
                sanitize_path(&profile_path)
            )
        })?;
        let profile = config::parse(&profile_text, &profile_path)?;
        config::check_shape(&profile, &profile_path)?;

        let hooks_path = user_codex.join(HOOKS);
        let user_groups = match read_followed(&hooks_path)? {
            None => Vec::new(),
            Some(text) => {
                hooks_json::user_pre_tool_use(&text, &hooks_path, &layout.amplihack.join("codex"))?
            }
        };
        Ok(Self {
            layout,
            kind: Kind::Build {
                profile,
                user_groups,
            },
        })
    }

    /// Builds, refreshes or verifies the home: the last step before Codex
    /// is spawned.
    pub(super) fn refresh(&self, name: &ProfileName) -> Result<LocalHome> {
        let home = self
            .layout
            .generated
            .to_str()
            .ok_or_else(|| {
                anyhow!(
                    "the local-model Codex home {} is not valid UTF-8",
                    sanitize_path(&self.layout.generated)
                )
            })?
            .to_owned();
        let (user_handlers, sandbox_mode) = match &self.kind {
            Kind::Reuse => self.verify_reuse()?,
            Kind::Build {
                profile,
                user_groups,
            } => {
                self.build(profile, user_groups)?;
                (
                    hooks_json::handler_count(user_groups),
                    config::sandbox_mode(profile),
                )
            }
        };
        Ok(LocalHome {
            name: name.clone(),
            home,
            user_handlers,
            sandbox_mode,
        })
    }

    fn build(&self, profile: &Table, user_groups: &[Value]) -> Result<()> {
        let layout = &self.layout;
        let hooks_bin = crate::commands::install::find_hooks_binary().context(
            "a local-model launch needs the amplihack-hooks binary for its command check",
        )?;
        validate_hooks_binary(&hooks_bin)?;
        let amplihack_exists = check_amplihack_root(&layout.amplihack)?;
        for dir in [&layout.codex_local, &layout.generated, &layout.rules()] {
            check_existing_private_dir(dir)?;
        }

        // Compute everything before the first write.
        let config_path = layout.generated.join(CONFIG);
        let previous_config = read_owned(&config_path)?;
        let previous = match &previous_config {
            Some((bytes, _)) => {
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| anyhow!("{} is not valid UTF-8", sanitize_path(&config_path)))?;
                Some(config::parse(text, &config_path)?)
            }
            None => None,
        };
        let config_text = config::render(
            profile,
            previous.as_ref(),
            &layout.generated.join(INSTRUCTIONS_FILE),
        )?;
        let hooks_text = hooks_json::render(user_groups, &hooks_bin);
        let rules_text = current_rules();

        let rules_dir = layout.rules();
        let mut writes: Vec<(&Path, &str, &[u8])> = Vec::new();
        for (dir, file, bytes, current) in [
            (
                layout.generated.as_path(),
                CONFIG,
                config_text.as_bytes(),
                previous_config,
            ),
            (
                layout.generated.as_path(),
                HOOKS,
                hooks_text.as_bytes(),
                read_owned(&layout.generated.join(HOOKS))?,
            ),
            (
                layout.generated.as_path(),
                INSTRUCTIONS_FILE,
                INSTRUCTIONS.as_bytes(),
                read_owned(&layout.generated.join(INSTRUCTIONS_FILE))?,
            ),
            (
                rules_dir.as_path(),
                RULES_FILE,
                rules_text.as_bytes(),
                read_owned(&rules_dir.join(RULES_FILE))?,
            ),
        ] {
            let unchanged = current.is_some_and(|(existing, meta)| {
                check_private_file(&meta).is_ok() && existing == bytes
            });
            if !unchanged {
                writes.push((dir, file, bytes));
            }
        }

        // Now write.
        if !amplihack_exists {
            create_amplihack_root(&layout.amplihack)?;
        }
        for dir in [&layout.codex_local, &layout.generated, &rules_dir] {
            ensure_private_dir(dir).map_err(|error| private_dir_error(dir, &error))?;
        }
        for (dir, file, bytes) in writes {
            write_private_atomic(dir, file, bytes).map_err(|error| {
                anyhow!(
                    "{} could not be written: {error}",
                    sanitize_path(&dir.join(file))
                )
            })?;
        }
        Ok(())
    }

    /// The checks a reused home must pass, without writing to it. Returns
    /// the number of user handlers carried into its `hooks.json` and the
    /// `sandbox_mode` its `config.toml` sets, if any.
    fn verify_reuse(&self) -> Result<(usize, Option<String>)> {
        let layout = &self.layout;
        let refuse = |reason: String| {
            anyhow!(
                "the local-model Codex home {} that this nested launch would reuse failed a \
                 check: {reason}; relaunch amplihack codex from outside the local session",
                sanitize_path(&layout.generated)
            )
        };
        if !check_amplihack_root(&layout.amplihack).map_err(|e| refuse(format!("{e:#}")))? {
            return Err(refuse(format!(
                "{} does not exist",
                sanitize_path(&layout.amplihack)
            )));
        }
        let rules_dir = layout.rules();
        for dir in [&layout.codex_local, &layout.generated, &rules_dir] {
            check_private_dir(dir)
                .map_err(|error| refuse(private_dir_error(dir, &error).to_string()))?;
        }
        let read = |path: PathBuf| -> Result<Vec<u8>> {
            let (bytes, meta) = read_owned(&path)
                .map_err(|e| refuse(format!("{e:#}")))?
                .ok_or_else(|| refuse(format!("{} is missing", sanitize_path(&path))))?;
            check_private_file(&meta)
                .map_err(|error| refuse(format!("{} {error}", sanitize_path(&path))))?;
            Ok(bytes)
        };
        let hooks = read(layout.generated.join(HOOKS))?;
        let rules = read(rules_dir.join(RULES_FILE))?;
        let config_path = layout.generated.join(CONFIG);
        let config_bytes = read(config_path.clone())?;

        let config_text = std::str::from_utf8(&config_bytes).map_err(|_| {
            refuse(format!(
                "{} is not valid UTF-8",
                sanitize_path(&config_path)
            ))
        })?;
        let document =
            config::parse(config_text, &config_path).map_err(|e| refuse(format!("{e:#}")))?;
        let hooks_on = document
            .get("features")
            .and_then(|features| features.get("hooks"))
            .and_then(toml::Value::as_bool);
        if hooks_on != Some(true) {
            return Err(refuse(format!(
                "{} does not set [features] hooks = true",
                sanitize_path(&config_path)
            )));
        }
        // The session may have switched amplihack's handler off since the
        // last refresh. Codex keys its state by the hooks.json path, either
        // as CODEX_HOME gives it or canonicalised, so both are checked, and
        // a path that cannot be checked refuses the reuse.
        let hooks_path = layout.generated.join(HOOKS);
        let canonical = fs::canonicalize(&hooks_path).map_err(|error| {
            refuse(format!(
                "{} could not be resolved: {error}",
                sanitize_path(&hooks_path)
            ))
        })?;
        let mut paths = Vec::with_capacity(2);
        for path in [hooks_path.as_path(), canonical.as_path()] {
            paths
                .push(path.to_str().ok_or_else(|| {
                    refuse(format!("{} is not valid UTF-8", sanitize_path(path)))
                })?);
        }
        if config::guard_disabled(&document, &paths) {
            return Err(refuse(format!(
                "{} disables amplihack's PreToolUse handler in [hooks.state]",
                sanitize_path(&config_path)
            )));
        }

        let hooks_text = std::str::from_utf8(&hooks)
            .map_err(|_| refuse("hooks.json is not valid UTF-8".to_string()))?;
        let hooks_bin = crate::commands::install::find_hooks_binary()
            .map_err(|e| refuse(format!("the amplihack-hooks binary was not found: {e:#}")))?;
        let user_groups = hooks_json::verify_generated(hooks_text, &hooks_bin)
            .map_err(|e| refuse(format!("{e:#}")))?;

        if rules != current_rules().as_bytes() {
            return Err(refuse(
                "the rules file differs from what the current dangerous-command policy renders"
                    .to_string(),
            ));
        }
        Ok((
            hooks_json::handler_count(&user_groups),
            config::sandbox_mode(&document),
        ))
    }
}

/// The dangerous-command rules file for the current policy, the one
/// `amplihack install` writes for an ordinary Codex home.
fn current_rules() -> String {
    crate::policy::codex::rules_file(&crate::user_config::policy_deny_commands())
}

/// A nested launch inside a session on this home: `CODEX_HOME` is the
/// generated home for the same name, and it holds `config.toml` and
/// `hooks.json` (whatever their shape; verification decides).
fn reuses(generated: &Path) -> bool {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .is_some_and(|value| Path::new(&value) == generated)
        && [CONFIG, HOOKS]
            .iter()
            .all(|file| fs::symlink_metadata(generated.join(file)).is_ok())
}

/// `$CODEX_HOME` when set, non-empty and not inside `codex-local`, else
/// `<home>/.codex`, so that a nested launch under another name reads its
/// profile from the user's own Codex home, never from a generated one.
fn user_codex_home(home: &Path, codex_local: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| !path.starts_with(codex_local))
        .unwrap_or_else(|| home.join(".codex"))
}

/// Reads an input that may be reached through a symbolic link, which must
/// end at a regular file of at most [`READ_LIMIT`] bytes of UTF-8 that
/// passes [`check_input_owner`]. The open never blocks on a FIFO. `Ok(None)`
/// when the file does not exist.
fn read_followed(path: &Path) -> Result<Option<String>> {
    let shown = || sanitize_path(path);
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => bail!("{} could not be opened: {error}", shown()),
    };
    let meta = file
        .metadata()
        .map_err(|error| anyhow!("{} could not be checked: {error}", shown()))?;
    if !meta.is_file() {
        bail!("{} is not a regular file", shown());
    }
    check_input_owner(path, &meta)?;
    let bytes = read_bounded(&file, READ_LIMIT)
        .map_err(|error| anyhow!("{} could not be read: {error}", shown()))?;
    if bytes.len() as u64 > READ_LIMIT {
        bail!("{} is larger than 1 MiB", shown());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| anyhow!("{} is not valid UTF-8", shown()))
}

/// The profile decides which programs a session starts (its provider,
/// `notify` and MCP settings) and the user's `hooks.json` supplies handlers
/// that run on every tool call, so both follow the rule `~/.amplihack`
/// follows: the file (`meta`, read through the open descriptor) and the
/// directory holding it, and when `path` is a symbolic link the directory
/// holding its target too, must be owned by this user or by root and not
/// writable by others. Group write is accepted only for the user's own
/// private group, as elsewhere in amplihack.
fn check_input_owner(path: &Path, meta: &fs::Metadata) -> Result<()> {
    let trusted_owner = |meta: &fs::Metadata| meta.uid() == euid() || meta.uid() == 0;
    if !trusted_owner(meta) || writable_by_others(meta) {
        bail!(
            "{} must be owned by you and not writable by others, because it decides what runs \
             in a local-model session; amplihack does not change it",
            sanitize_path(path)
        );
    }
    // Both directories are canonical, so the metadata and the ACL check
    // below are about the directory itself, never a link to it.
    let resolve = |p: &Path| {
        fs::canonicalize(p)
            .map_err(|error| anyhow!("{} could not be resolved: {error}", sanitize_path(p)))
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut dirs = vec![resolve(parent)?];
    if let Some(target_dir) = resolve(path)?.parent()
        && dirs[0] != target_dir
    {
        dirs.push(target_dir.to_path_buf());
    }
    for dir in dirs {
        let dir_meta = fs::symlink_metadata(&dir)
            .map_err(|error| anyhow!("{} could not be checked: {error}", sanitize_path(&dir)))?;
        if !trusted_owner(&dir_meta) || dir_writable_by_others(&dir, &dir_meta) {
            bail!(
                "{}, which holds {}, must be owned by you and not writable by others; amplihack \
                 does not change it",
                sanitize_path(&dir),
                sanitize_path(path)
            );
        }
    }
    Ok(())
}

/// An owned file's bytes and metadata, without following a symbolic link:
/// `Ok(None)` when absent; an error for a link, a non-regular file or a file
/// over [`READ_LIMIT`].
fn read_owned(path: &Path) -> Result<Option<(Vec<u8>, fs::Metadata)>> {
    read_bounded_nofollow(path, READ_LIMIT).map_err(|error| {
        anyhow!(
            "{} {error}; amplihack refreshes only regular files it owns",
            sanitize_path(path)
        )
    })
}

/// The hooks binary amplihack's entry runs. It must be an absolute path
/// (a relative one is refused, never made absolute from the working
/// directory, so a checkout cannot supply the guard), valid UTF-8 with no
/// control character, an executable regular file, and named
/// `amplihack-hooks`, which is what a nested launch checks for.
fn validate_hooks_binary(path: &Path) -> Result<()> {
    let refuse = |problem: &str| {
        anyhow!(
            "the amplihack-hooks binary {} {problem}; set {HOOKS_OVERRIDE} to the absolute \
             path of an executable amplihack-hooks",
            sanitize_path(path)
        )
    };
    if !path.is_absolute() {
        return Err(refuse("is not an absolute path"));
    }
    let Some(text) = path.to_str() else {
        return Err(refuse("is not valid UTF-8"));
    };
    if text.chars().any(char::is_control) {
        return Err(refuse("contains a control character"));
    }
    let meta = fs::metadata(path).map_err(|error| refuse(&format!("cannot be read ({error})")))?;
    if !meta.is_file() {
        return Err(refuse("is not a regular file"));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(refuse("is not executable"));
    }
    if path.file_name().and_then(|name| name.to_str()) != Some(hooks_json::HOOKS_BINARY) {
        return Err(refuse(&format!(
            "is not named {}",
            hooks_json::HOOKS_BINARY
        )));
    }
    Ok(())
}

/// `~/.amplihack`, followed if it is a link, must be a directory owned
/// by this user that others cannot write. It is never changed. `Ok(false)`
/// when it does not exist yet.
fn check_amplihack_root(path: &Path) -> Result<bool> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => bail!("{} could not be checked: {error}", sanitize_path(path)),
    };
    if !meta.is_dir() || !owned_by_euid(&meta) || dir_writable_by_others(path, &meta) {
        bail!(
            "{} must be a directory owned by you and not writable by others; amplihack does \
             not change it",
            sanitize_path(path)
        );
    }
    Ok(true)
}

/// Creates a missing `~/.amplihack` at 0700, one level only. One that
/// appeared meanwhile is checked as an existing one is.
fn create_amplihack_root(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            check_amplihack_root(path).map(|_| ())
        }
        Err(error) => bail!("{} could not be created: {error}", sanitize_path(path)),
    }
}

/// A directory amplihack would create in the home that already exists must
/// already be private and real: amplihack neither loosens nor repairs a
/// mode, and never writes through a link.
fn check_existing_private_dir(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => bail!("{} could not be checked: {error}", sanitize_path(path)),
        Ok(_) => check_private_dir(path).map_err(|error| private_dir_error(path, &error)),
    }
}

fn private_dir_error(path: &Path, error: &io::Error) -> anyhow::Error {
    anyhow!(
        "{} {error}; it must be a real directory owned by you with mode 0700, and amplihack \
         does not change it",
        sanitize_path(path)
    )
}
