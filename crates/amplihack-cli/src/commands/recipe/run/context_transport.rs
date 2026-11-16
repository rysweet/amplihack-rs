//! Issue #2582: how recipe context reaches recipe-runner-rs.
//!
//! Every value passed as `--set key=value` is on the runner's argv, where any
//! process on the host can read it through `/proc/<pid>/cmdline` and a
//! pattern kill (`pkill -f <word>`) can match it. When the runner reports the
//! [`CONTEXT_FILE_CAPABILITY`] capability, the whole context goes into a 0600
//! file under the run's private runtime folder and the argv carries only
//! `--context-file <path>`.
//!
//! The pinned runner reports the capability, so the file is the path a normal
//! run takes. The `--set` path ([`super::execute::pass_context`]) remains for
//! a runner older than the pin supplied through `RECIPE_RUNNER_RS_PATH`, and
//! runs unchanged there. [`ContextTransport::record`] names the transport each
//! run took: every transport at info level, and the `--set` downgrade also on
//! stderr, unconditionally. The stderr line is the one that matters, because
//! the binary's log filter is `ERROR` unless `RUST_LOG` says otherwise — so
//! without it the downgrade would be visible only in `/proc`, which is what
//! this change takes the context out of.
//!
//! The two paths are never mixed, and a failed file write is an error: it
//! never falls back to `--set`.
//!
//! The file is 0600 in a 0700 folder, so no other account can read it. It is
//! still readable by a process running as this user, as the environment
//! mirror of the same values always has been; what this module removes is the
//! world-readable copy on argv.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::execute::{is_reserved_context_key, pass_context};

/// The capability a runner reports in `--capabilities` when it accepts
/// `--context-file <PATH>`.
pub(super) const CONTEXT_FILE_CAPABILITY: &str = "context_file_v1";

/// The argv flag that names the context file.
const CONTEXT_FILE_FLAG: &str = "--context-file";

/// The stderr line printed once per run that takes the `--set` transport.
///
/// It is the operator-visible evidence that this run's context is on the
/// runner's argv after all, and it must stay free of any key or value: it is
/// printed precisely because the context is already exposed.
/// The capability name is spelled out rather than interpolated, because a
/// `const` cannot be formatted; `the_notice_names_the_capability_and_the_override`
/// holds the two equal.
pub(super) const DOWNGRADE_NOTICE: &str = concat!(
    "amplihack: recipe context is on the runner's argv (--set): ",
    "the runner supplied through RECIPE_RUNNER_RS_PATH does not report ",
    "context_file_v1",
);

/// Folder under the runtime folder that holds the context file. It is not
/// `tmp/`, which every step gets as `TMPDIR`.
const CONTEXT_DIR_NAME: &str = "context";
const CONTEXT_FILE_NAME: &str = "context.json";

/// Whether a `--capabilities` report lists [`CONTEXT_FILE_CAPABILITY`].
/// The match is exact on the members of `capabilities[]`; anything else
/// (a missing field, a non-array, a near name) means "not supported".
pub(super) fn supports_context_file(report: &serde_json::Value) -> bool {
    report
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|c| c.as_str() == Some(CONTEXT_FILE_CAPABILITY))
        })
}

/// Refuses a reserved context key (issue #1567): the `AMPLIHACK_` namespace,
/// the Git routing and executable configuration the command policy removes
/// before launch, and the recipe-runner selector `RECIPE_RUNNER_RS_PATH`.
///
/// The rule itself is [`is_reserved_context_key`], so this and
/// [`super::execute::pass_context`] cannot drift apart. Both call it: this one
/// runs before a transport is chosen, so no file is written for a refused
/// map, and `pass_context` keeps its own call for the `--set` path. The
/// duplication is deliberate — either path alone refuses the key.
///
/// The error names the key, escaped, and never a value.
pub(super) fn refuse_reserved_keys(context: &BTreeMap<String, String>) -> Result<()> {
    if let Some(key) = context.keys().find(|k| is_reserved_context_key(k)) {
        anyhow::bail!(
            "recipe context key {} is reserved: the AMPLIHACK_ namespace, Git execution configuration and the RECIPE_RUNNER_RS_PATH runner selector cannot be set from recipe context",
            crate::commands::profile::escape_for_error(key)
        );
    }
    Ok(())
}

/// The context file of one run. Dropping it removes the file and then its
/// folder (never `remove_dir_all`), so an unexpected entry in the folder is
/// left alone. Errors during cleanup are ignored: the run's runtime folder
/// is removed by `RuntimeGuard` afterwards, and a SIGKILL leftover is
/// removed by the sweep of `runs/<id>/runtime`.
#[derive(Debug)]
pub(super) struct ContextFile {
    path: PathBuf,
    dir: PathBuf,
}

impl ContextFile {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ContextFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.dir);
    }
}

/// Writes `context` as a JSON object of strings to
/// `<runtime_dir>/context/context.json`.
///
/// The folder is created non-recursively at 0700; an existing entry named
/// `context` (a folder or a symbolic link) is refused, so nothing is ever
/// written through a planted link. The file is written through
/// `write_atomic_0600` (create_new, O_NOFOLLOW, 0600, fsync). Error text
/// holds paths and the I/O error only, never a value.
pub(super) fn write_context_file(
    runtime_dir: &Path,
    context: &BTreeMap<String, String>,
) -> Result<ContextFile> {
    let dir = runtime_dir.join(CONTEXT_DIR_NAME);
    let path = dir.join(CONTEXT_FILE_NAME);
    let failed = || format!("failed to write recipe context file {}", path.display());
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            anyhow::bail!(
                "failed to write recipe context file {}: {} already exists",
                path.display(),
                dir.display()
            );
        }
        Err(error) => {
            return Err(error).with_context(failed);
        }
    }
    // From here the folder is ours: the guard removes it on every exit path.
    let file = ContextFile {
        path: path.clone(),
        dir,
    };
    // The umask may have narrowed the mode; it never widens it, but set it
    // explicitly so the folder is exactly 0700.
    fs::set_permissions(&file.dir, fs::Permissions::from_mode(0o700)).with_context(failed)?;
    let bytes = serde_json::to_vec(context).with_context(failed)?;
    crate::commands::cleanup::run_dir::write_atomic_0600(&path, &bytes).with_context(failed)?;
    Ok(file)
}

/// How this run's context reaches the runner.
#[derive(Debug)]
pub(super) enum ContextTransport {
    /// The context is empty: neither flag is passed.
    None,
    /// The runner reports [`CONTEXT_FILE_CAPABILITY`]: `--context-file <path>`.
    File(ContextFile),
    /// The fallback: today's `--set key=value` pairs.
    Set,
}

impl ContextTransport {
    /// Chooses the transport for one run. Order: the reserved-key refusal,
    /// then an empty context gives [`ContextTransport::None`], then a report
    /// with the capability writes the file, otherwise `--set`. A write
    /// failure is returned as an error and never falls back to `--set`.
    pub(super) fn choose(
        report: &serde_json::Value,
        runtime_dir: &Path,
        context: &BTreeMap<String, String>,
    ) -> Result<Self> {
        refuse_reserved_keys(context)?;
        if context.is_empty() {
            return Ok(Self::None);
        }
        if supports_context_file(report) {
            return Ok(Self::File(write_context_file(runtime_dir, context)?));
        }
        Ok(Self::Set)
    }

    /// This transport's name, for the operator-visible record. It is derived
    /// from the variant alone, so it can never carry a key or a value.
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::File(_) => "file",
            Self::Set => "set",
        }
    }

    /// Records which transport this run took.
    ///
    /// Every transport is named at info level for a reader of the run's log.
    /// That line alone is not enough for the one case that matters: the
    /// binary installs `EnvFilter::from_default_env()` (`bins/amplihack`),
    /// whose default directive is `ERROR`, so with `RUST_LOG` unset an
    /// `info` record is written nowhere. The [`Self::Set`] arm therefore
    /// also prints to stderr, unconditionally.
    ///
    /// The pinned runner reports [`CONTEXT_FILE_CAPABILITY`], so `set` means
    /// a runner older than the pin was supplied through
    /// `RECIPE_RUNNER_RS_PATH` and this run's context is on the runner's
    /// argv after all. Without a line nobody has to opt into, that downgrade
    /// is visible only in `/proc`, which is the thing this change takes the
    /// context out of. The other two arms stay on `tracing` alone: `file` is
    /// what a normal run does, and a line on every run is noise that teaches
    /// an operator to skip the one line here that is worth reading.
    ///
    /// Both forms hold [`Self::name`] and fixed text and nothing else: no
    /// key, no value, and no count derived from one, since a count of a
    /// one-key context says which key it was. The stderr line goes to
    /// stderr and never stdout, which stays data-only (#1525).
    pub(super) fn record(&self) {
        tracing::info!(transport = %self.name(), "recipe context transport");
        if let Some(notice) = self.downgrade_notice() {
            eprintln!("{notice}");
        }
    }

    /// The stderr line [`Self::record`] prints for this transport, if any.
    ///
    /// The decision is split out from the printing because `eprintln!` under
    /// `cargo test` writes to libtest's per-test capture buffer rather than
    /// to fd 2, so an in-process test cannot observe the real stream and
    /// would pass whether or not the line were ever emitted. This function is
    /// what the unit tests assert; that the line actually reaches an
    /// operator's stderr, from the real binary with no `RUST_LOG` set, is
    /// asserted by
    /// `tests/issue_2582_degraded_runner_no_set.rs::a_valid_report_without_the_capability_still_reaches_the_runner_with_set`.
    pub(super) fn downgrade_notice(&self) -> Option<&'static str> {
        matches!(self, Self::Set).then_some(DOWNGRADE_NOTICE)
    }

    /// Adds this transport's arguments to `command` and returns the argv
    /// bytes they occupy (strings, NULs and argv pointers), which the caller
    /// takes out of the env-mirror budget (#1570).
    ///
    /// `context` is read only on the [`Self::Set`] arm, which still builds
    /// `--set key=value` pairs from it. Pass the same map that was given to
    /// [`Self::choose`]: a different one would be silently ignored on the
    /// [`Self::File`] arm and silently used on the `Set` arm.
    pub(super) fn apply(
        &self,
        command: &mut Command,
        context: &BTreeMap<String, String>,
    ) -> Result<usize> {
        match self {
            Self::None => Ok(0),
            Self::File(file) => {
                command.arg(CONTEXT_FILE_FLAG).arg(file.path());
                Ok(CONTEXT_FILE_FLAG.len()
                    + 1
                    + file.path().as_os_str().len()
                    + 1
                    + 2 * std::mem::size_of::<usize>())
            }
            Self::Set => pass_context(command, context),
        }
    }
}
