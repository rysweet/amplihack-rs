//! The generated Codex home on disk: reading the inputs, building or
//! refreshing the home, and verifying a home a nested launch reuses. Unix
//! only.
//!
//! Layout: `<home>/.amplihack/codex-local/<name>/` holding the four files
//! amplihack owns (`config.toml`, `hooks.json`, `instructions.md`,
//! `rules/amplihack-dangerous-commands.rules`) and a read-only copy,
//! `rules/amplihack-user-<file>`, of each top-level `*.rules` file in the
//! user's own `rules/`. A refresh removes an `amplihack-user-*.rules` copy
//! whose source is gone, and no other file. Everything else in the home (sessions, databases, logs, an
//! `auth.json` from a login there) belongs to Codex and is never listed,
//! changed or deleted.
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
    owned_by_euid, read_bounded, read_bounded_nofollow, write_private_atomic,
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
/// The prefix of the copies of the user's own rules files.
const USER_RULES_PREFIX: &str = "amplihack-user-";
const RULES_SUFFIX: &str = ".rules";
const HOOKS_OVERRIDE: &str = "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH";

/// An owned file's bytes and metadata, as [`read_owned`] returns them.
type Owned = (Vec<u8>, fs::Metadata);

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
    /// Build or refresh the home from the profile and the user's own inputs.
    Build {
        profile: Table,
        /// The user's approval and sandbox settings ([`config::user_settings`]).
        user: Table,
        user_groups: Vec<Value>,
        /// The user's rules files: the copy's file name and the text.
        user_rules: Vec<(String, String)>,
        /// The deployed hooks binary, checked.
        hooks_bin: PathBuf,
    },
}

/// A local-model launch's inputs, read and checked. Nothing written.
#[derive(Debug)]
pub(super) struct Plan {
    layout: Layout,
    kind: Kind,
    /// Lines the launch prints on stderr about what its inputs leave out
    /// or rely on ([`build_tree_hooks_notice`], [`config::user_profile_notice`]).
    notices: Vec<String>,
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
                notices: Vec::new(),
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

        let (hooks_bin, hooks_notice) = deployed_hooks_binary()?;
        let mut notices: Vec<String> = hooks_notice.into_iter().collect();

        let user_config_path = user_codex.join(CONFIG);
        let user = match read_followed(&user_config_path)? {
            None => Table::new(),
            Some(text) => {
                let user_config = config::parse(&text, &user_config_path)?;
                notices.extend(config::user_profile_notice(&user_config, &user_config_path));
                config::user_settings(&user_config)
            }
        };

        let hooks_path = user_codex.join(HOOKS);
        let user_groups = match read_followed(&hooks_path)? {
            None => Vec::new(),
            Some(text) => hooks_json::user_pre_tool_use(
                &text,
                &hooks_path,
                &layout.amplihack.join("codex"),
                &hooks_bin,
            )?,
        };
        let user_rules = read_user_rules(&user_codex.join(RULES_DIR))?;
        Ok(Self {
            layout,
            kind: Kind::Build {
                profile,
                user,
                user_groups,
                user_rules,
                hooks_bin,
            },
            notices,
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
        let mut notices = self.notices.clone();
        let (user_handlers, sandbox_mode) = match &self.kind {
            Kind::Reuse => {
                let (handlers, sandbox_mode, hooks_notice) = self.verify_reuse()?;
                notices.extend(hooks_notice);
                (handlers, sandbox_mode)
            }
            Kind::Build {
                profile,
                user,
                user_groups,
                user_rules,
                hooks_bin,
            } => {
                self.build(profile, user, user_groups, user_rules, hooks_bin)?;
                (
                    hooks_json::handler_count(user_groups),
                    config::sandbox_mode(profile).or_else(|| config::sandbox_mode(user)),
                )
            }
        };
        Ok(LocalHome {
            name: name.clone(),
            home,
            user_handlers,
            sandbox_mode,
            notices,
        })
    }

    fn build(
        &self,
        profile: &Table,
        user: &Table,
        user_groups: &[Value],
        user_rules: &[(String, String)],
        hooks_bin: &Path,
    ) -> Result<()> {
        let layout = &self.layout;
        let amplihack_exists = check_amplihack_root(&layout.amplihack)?;
        for dir in [&layout.codex_local, &layout.generated, &layout.rules()] {
            check_existing_private_dir(dir)?;
        }

        // Codex keys hook trust by the canonical source path. The generated
        // directories need not exist yet, but every existing child of the
        // supported .amplihack link was checked above to be a real directory.
        let root = if amplihack_exists {
            fs::canonicalize(&layout.amplihack)?
        } else {
            fs::canonicalize(
                layout
                    .amplihack
                    .parent()
                    .context(".amplihack has no parent")?,
            )?
            .join(".amplihack")
        };
        let canonical_codex_local = root.join("codex-local");
        let hooks_source = canonical_codex_local
            .join(
                layout
                    .generated
                    .file_name()
                    .context("Codex home has no name")?,
            )
            .join(HOOKS);

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
            user,
            previous.as_ref(),
            &layout.generated.join(INSTRUCTIONS_FILE),
            (
                &format!("{}:pre_tool_use:0:0", hooks_source.display()),
                &hooks_json::command_check_hash(hooks_bin),
            ),
        )?;
        let hooks_text = hooks_json::render(user_groups, hooks_bin);
        let rules_text = current_rules();

        let rules_dir = layout.rules();
        let stale = stale_user_rules(&rules_dir, user_rules)?;
        let mut writes: Vec<(&Path, &str, &[u8])> = Vec::new();
        let mut planned: Vec<(&Path, &str, &[u8], Option<Owned>)> = Vec::new();
        for (file, text) in user_rules {
            planned.push((
                rules_dir.as_path(),
                file.as_str(),
                text.as_bytes(),
                read_owned(&rules_dir.join(file))?,
            ));
        }
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
        ]
        .into_iter()
        .chain(planned)
        {
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
        // Creating codex-local syncs its parent; open the resolved root so
        // fsync's no-follow check does not reject the supported root link.
        for dir in [&canonical_codex_local, &layout.generated, &rules_dir] {
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
        for file in stale {
            let path = rules_dir.join(&file);
            fs::remove_file(&path).map_err(|error| {
                anyhow!(
                    "{}, a copy of a user rules file that no longer exists, could not be \
                     removed: {error}",
                    sanitize_path(&path)
                )
            })?;
        }
        Ok(())
    }

    /// The checks a reused home must pass, without writing to it. Returns
    /// the number of user handlers carried into its `hooks.json`, the
    /// `sandbox_mode` its `config.toml` sets, if any, and the hooks binary's
    /// [`build_tree_hooks_notice`], if any.
    fn verify_reuse(&self) -> Result<(usize, Option<String>, Option<String>)> {
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
        let (hooks_bin, hooks_notice) =
            deployed_hooks_binary().map_err(|e| refuse(format!("{e:#}")))?;
        let (user_groups, expected_hash) = hooks_json::verify_generated(hooks_text, &hooks_bin)
            .map_err(|e| refuse(format!("{e:#}")))?;
        let key = format!("{}:pre_tool_use:0:0", canonical.display());
        if document
            .get("hooks")
            .and_then(|hooks| hooks.get("state"))
            .and_then(|state| state.get(&key))
            .and_then(|entry| entry.get("trusted_hash"))
            .and_then(toml::Value::as_str)
            != Some(expected_hash.as_str())
        {
            return Err(refuse(
                "amplihack's PreToolUse handler is not trusted for its current definition".into(),
            ));
        }

        if rules != current_rules().as_bytes() {
            return Err(refuse(
                "the rules file differs from what the current dangerous-command policy renders"
                    .to_string(),
            ));
        }
        Ok((
            hooks_json::handler_count(&user_groups),
            config::sandbox_mode(&document),
            hooks_notice,
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
        // A link whose target is missing is an error, never "no file": the
        // user meant something to be there.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
                bail!(
                    "{} is a symbolic link whose target does not exist; fix or remove the link",
                    shown()
                );
            }
            return Ok(None);
        }
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
/// that run on every tool call. The file (`meta`, read through the open
/// descriptor) and the directory holding it, and when `path` is a symbolic link the directory
/// holding its target too, must be owned by the effective user with no group
/// or other write bits. This is stricter than the shared directory rule,
/// which allows private-group writes. On POSIX ACL filesystems the group
/// write bit is the ACL mask, so rejecting it also excludes named writers.
fn check_input_owner(path: &Path, meta: &fs::Metadata) -> Result<()> {
    check_owner_for(path, meta, euid())
}

/// [`check_input_owner`] for the user `uid`, so a test can show that a file
/// owned by another user is rejected without running as root.
pub(super) fn check_owner_for(path: &Path, meta: &fs::Metadata, uid: u32) -> Result<()> {
    if meta.uid() != uid || meta.mode() & 0o022 != 0 {
        bail!(
            "{} must be owned by you and not writable by others, because it decides what runs \
             in a local-model session; amplihack does not change it",
            sanitize_path(path)
        );
    }
    // Both directories are canonical, so the metadata below describes the
    // directory itself, never a link to it.
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
        if dir_meta.uid() != uid || dir_meta.mode() & 0o022 != 0 {
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
fn read_owned(path: &Path) -> Result<Option<Owned>> {
    read_bounded_nofollow(path, READ_LIMIT).map_err(|error| {
        anyhow!(
            "{} {error}; amplihack refreshes only regular files it owns",
            sanitize_path(path)
        )
    })
}

/// The hooks binary amplihack's entry runs, chosen as the ordinary install
/// chooses the one it records: the deployed copy in the user's bin
/// directory, else the first of [`HOOKS_OVERRIDE`], the binary beside the
/// running executable and `PATH`. It must pass [`validate_hooks_binary`]
/// and [`build_tree_hooks_notice`], whose line, if any, is returned with it.
fn deployed_hooks_binary() -> Result<(PathBuf, Option<String>)> {
    let (path, is_deployed) = crate::commands::install::stable_hooks_binary_with_origin()
        .context("a local-model launch needs the amplihack-hooks binary for its command check")?;
    validate_hooks_binary(&path)?;
    let notice = build_tree_hooks_notice(
        &path,
        is_deployed,
        std::env::var_os(HOOKS_OVERRIDE).as_deref(),
    )?;
    Ok((path, notice))
}

/// The check on a hooks binary that is not the deployed copy but lies in a
/// build directory ([`looks_like_build_artifact`]), such as
/// `target/debug/amplihack-hooks` found beside the running executable. The
/// command-check entry would name a file that disappears when that
/// directory is cleaned, so such a path is refused, unless `override_value`
/// (the value of [`HOOKS_OVERRIDE`]) names it: a path the user set on
/// purpose is used, and the returned line says that it is a build output.
/// The deployed copy is never checked here.
///
/// [`looks_like_build_artifact`]: crate::commands::install::looks_like_build_artifact
pub(super) fn build_tree_hooks_notice(
    path: &Path,
    is_deployed: bool,
    override_value: Option<&std::ffi::OsStr>,
) -> Result<Option<String>> {
    if is_deployed || !crate::commands::install::looks_like_build_artifact(path) {
        return Ok(None);
    }
    if override_value.is_some_and(|value| Path::new(value) == path) {
        return Ok(Some(format!(
            "the command check runs {}, a build output named by {HOOKS_OVERRIDE}; it stops \
             working when that directory is cleaned; run `amplihack install` to deploy a \
             stable copy",
            sanitize_path(path)
        )));
    }
    bail!(
        "the amplihack-hooks binary {} is in a build directory and no deployed copy exists; \
         the command check would stop working when that directory is cleaned; deploy it with \
         `amplihack install`, or set {HOOKS_OVERRIDE} to its absolute path to use it anyway",
        sanitize_path(path)
    )
}

/// The hooks binary amplihack's entry runs. It must be an absolute path
/// (a relative one is refused, never made absolute from the working
/// directory, so a checkout cannot supply the guard), valid UTF-8 with no
/// control character, an executable regular file, and named
/// `amplihack-hooks`, which is what a nested launch checks for. It runs on
/// every tool call, so it must also pass [`check_input_owner`]: owned by
/// the user, with no group or other write bit on the file, its directory or
/// its link target's directory.
fn validate_hooks_binary(path: &Path) -> Result<()> {
    let refuse = |problem: &str| {
        anyhow!(
            "the amplihack-hooks binary {} {problem}; deploy it with `amplihack install`, or, \
             when no deployed copy exists, set {HOOKS_OVERRIDE} to the absolute path of an \
             executable amplihack-hooks",
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
    check_input_owner(path, &meta)
}

/// The user's own rules files: each top-level regular `*.rules` file in
/// `dir` (the user's `rules/`), read with [`read_followed`], as the file
/// name of its copy (`amplihack-user-<file>`) and its text, sorted by name.
/// Subdirectories and files without the suffix are not read. A missing
/// `dir` means no rules. A name that is not UTF-8 or contains a control
/// character is an error, so that no rule is left out without a word.
fn read_user_rules(dir: &Path) -> Result<Vec<(String, String)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => bail!("{} could not be listed: {error}", sanitize_path(dir)),
    };
    let mut rules = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| anyhow!("{} could not be listed: {error}", sanitize_path(dir)))?;
        let path = entry.path();
        let Some(name) = entry
            .file_name()
            .to_str()
            .filter(|name| !name.chars().any(char::is_control))
            .map(str::to_owned)
        else {
            bail!(
                "{} has a rules file name that is not UTF-8 or contains a control character; \
                 rename it so that a local-model launch can copy it",
                sanitize_path(dir)
            );
        };
        if !name.ends_with(RULES_SUFFIX) || fs::metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        let text = read_followed(&path)?
            .ok_or_else(|| anyhow!("{} disappeared while it was read", sanitize_path(&path)))?;
        rules.push((format!("{USER_RULES_PREFIX}{name}"), text));
    }
    rules.sort();
    Ok(rules)
}

/// The `amplihack-user-*.rules` copies in the generated `rules_dir` whose source is no
/// longer among `user_rules`. Nothing when the directory does not exist yet.
fn stale_user_rules(rules_dir: &Path, user_rules: &[(String, String)]) -> Result<Vec<String>> {
    let entries = match fs::read_dir(rules_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => bail!("{} could not be listed: {error}", sanitize_path(rules_dir)),
    };
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            anyhow!("{} could not be listed: {error}", sanitize_path(rules_dir))
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(USER_RULES_PREFIX)
            && name.ends_with(RULES_SUFFIX)
            && !user_rules.iter().any(|(file, _)| *file == name)
        {
            stale.push(name);
        }
    }
    stale.sort();
    Ok(stale)
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
