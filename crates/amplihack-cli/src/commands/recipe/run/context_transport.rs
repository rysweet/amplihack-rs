//! Issue #2582: how recipe context reaches recipe-runner-rs.
//!
//! Every value passed as `--set key=value` is on the runner's argv, where any
//! process on the host can read it through `/proc/<pid>/cmdline` and a
//! pattern kill (`pkill -f <word>`) can match it. When the runner reports the
//! [`CONTEXT_FILE_CAPABILITY`] capability, the whole context goes into a 0600
//! file under the run's private runtime folder and the argv carries only
//! `--context-file <path>`. Without the capability, today's `--set` path
//! ([`super::execute::pass_context`]) runs unchanged.
//!
//! The two paths are never mixed, and a failed file write is an error: it
//! never falls back to `--set`.

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

/// Refuses a context key in the `AMPLIHACK_` namespace (issue #1567). Runs
/// before any transport is chosen, so no file is written for a refused map.
/// The error names the key, escaped, and never a value.
pub(super) fn refuse_reserved_keys(context: &BTreeMap<String, String>) -> Result<()> {
    if let Some(key) = context.keys().find(|k| is_reserved_context_key(k)) {
        anyhow::bail!(
            "recipe context key {} is reserved: the AMPLIHACK_ namespace cannot be set from recipe context",
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
            return Err(error).with_context(|| {
                format!("failed to write recipe context file {}", path.display())
            });
        }
    }
    // From here the folder is ours: the guard removes it on every exit path.
    let file = ContextFile {
        path: path.clone(),
        dir: dir.clone(),
    };
    // The umask may have narrowed the mode; it never widens it, but set it
    // explicitly so the folder is exactly 0700.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to write recipe context file {}", path.display()))?;
    let bytes = serde_json::to_vec(context)
        .with_context(|| format!("failed to write recipe context file {}", path.display()))?;
    crate::commands::cleanup::run_dir::write_atomic_0600(&path, &bytes)
        .with_context(|| format!("failed to write recipe context file {}", path.display()))?;
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

    /// Adds this transport's arguments to `command` and returns the argv
    /// bytes they occupy (strings, NULs and argv pointers), which the caller
    /// takes out of the env-mirror budget (#1570).
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
