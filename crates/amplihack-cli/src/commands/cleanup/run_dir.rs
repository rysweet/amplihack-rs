//! The per-run folder `<work root>/runs/<run id>/` (issue #1808, D2, D3, D4, R3).
//!
//! It holds the run report `run.json`, the `owned-paths` list that helpers
//! append to, `cleanup-record.json` after a sweep removed something, and the
//! ephemeral `runtime/` folder that [`RuntimeGuard`] removes when the run ends.

use super::work_root::{WorkRoot, create_private_dir_all};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) const RUN_JSON: &str = "run.json";
pub(crate) const OWNED_PATHS: &str = "owned-paths";
pub(crate) const CLEANUP_RECORD: &str = "cleanup-record.json";
pub(crate) const RUN_SCHEMA_VERSION: u32 = 1;

/// Longest `runtime/tmp` path the default layout may produce, leaving room
/// under the 108-byte Unix socket path limit.
const MAX_RUNTIME_TMP_BYTES: usize = 80;
const OWNED_PATHS_MAX_BYTES: u64 = 1024 * 1024;
const OWNED_PATHS_MAX_LINES: usize = 10_000;
const RUN_JSON_MAX_BYTES: u64 = 1024 * 1024;

/// A new run id: a lowercase UUID v4.
pub(crate) fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `<root>/runs/<id>/runtime`, or `<root>/r/<first 12 hex digits>` when
/// `<that>/tmp` would be longer than 80 bytes.
pub(crate) fn runtime_dir_for(work_root: &Path, run_id: &str) -> PathBuf {
    let default = work_root.join("runs").join(run_id).join("runtime");
    if default.join("tmp").as_os_str().len() <= MAX_RUNTIME_TMP_BYTES {
        default
    } else {
        work_root.join("r").join(short_id(run_id))
    }
}

/// The first 12 hex digits of a run id.
pub(crate) fn short_id(run_id: &str) -> String {
    run_id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(12)
        .collect()
}

/// Removes a runtime folder when dropped, including during unwinding.
#[derive(Debug)]
pub(crate) struct RuntimeGuard {
    path: Option<PathBuf>,
    identity: (u64, u64),
}

impl RuntimeGuard {
    pub(crate) fn new(path: PathBuf) -> Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("runtime resource is not a private directory");
        }
        Ok(Self {
            path: Some(path),
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().unwrap_or(Path::new(""))
    }

    /// Explicit release is fallible and never deletes a substituted path.
    pub(crate) fn remove(mut self) -> Result<()> {
        self.release()
    }

    fn release(&mut self) -> Result<()> {
        let Some(path) = self.path.take() else {
            return Ok(());
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("inspect runtime ownership before release"),
        };
        if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != self.identity {
            bail!(
                "runtime resource identity changed; replacement retained at {}",
                path.display()
            );
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("release owned runtime directory"),
        }
    }
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            eprintln!("amplihack: runtime retained: {error:#}");
        }
    }
}

/// Field 22 of `/proc/<pid>/stat` (start time in clock ticks), read from the
/// fields after the last `)` so a process name holding `)` cannot shift it.
pub(crate) fn parse_start_ticks(stat: &str) -> Option<u64> {
    let close = stat.rfind(')')?;
    // After ")" come fields 3.. ; field 22 is the 20th of them.
    stat[close + 1..].split_whitespace().nth(19)?.parse().ok()
}

/// The start ticks of `pid`, or `None` when `/proc` cannot tell.
pub(crate) fn pid_start_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_ticks(&stat)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedKind {
    Worktree,
    Target,
}

impl OwnedKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::Target => "target",
        }
    }
}

/// Whether the pre-commit hook built a target first or found it already
/// there (issue #2039, D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetOrigin {
    Created,
    Reused,
}

impl TargetOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Reused => "reused",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "created" => Some(Self::Created),
            "reused" => Some(Self::Reused),
            _ => None,
        }
    }
}

/// The extra fields of a five-field target line (issue #2039, D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetRecord {
    pub origin: TargetOrigin,
    /// The absolute checkout the target belongs to. Only ever compared,
    /// checked with `symlink_metadata` or hashed; never opened or removed.
    pub checkout: PathBuf,
}

/// One `owned-paths` line. `target` is `Some` only for a five-field target
/// line, which also has `kind == Target` and `branch == None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedPath {
    pub kind: OwnedKind,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub target: Option<TargetRecord>,
}

/// Parses `owned-paths`. A line is either the legacy
/// `kind<TAB>path[<TAB>branch]` (the branch may be empty or missing), or the
/// five-field target line `target<TAB>path<TAB><TAB>origin<TAB>checkout`
/// written by the pre-commit hook since issue #2039. Reads at most 10 000
/// lines and drops (never repairs) every other shape: relative paths, unknown
/// kinds or origins, four or six or more fields, and control characters.
pub(crate) fn parse_owned_paths(bytes: &[u8]) -> Vec<OwnedPath> {
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n').take(OWNED_PATHS_MAX_LINES) {
        if let Some(parsed) = parse_owned_line(line) {
            out.push(parsed);
        }
    }
    out
}

fn parse_owned_line(line: &[u8]) -> Option<OwnedPath> {
    if line.is_empty() {
        return None;
    }
    // Tabs separate fields; every other control character makes the line invalid.
    if line
        .iter()
        .any(|b| *b != b'\t' && (*b < 0x20 || *b == 0x7f))
    {
        return None;
    }
    let text = std::str::from_utf8(line).ok()?;
    let fields: Vec<&str> = text.split('\t').collect();
    let kind = match fields[0] {
        "worktree" => OwnedKind::Worktree,
        "target" => OwnedKind::Target,
        _ => return None,
    };
    let path = PathBuf::from(fields.get(1)?);
    if !path.is_absolute() {
        return None;
    }
    match fields.len() {
        2 | 3 => Some(OwnedPath {
            kind,
            path,
            branch: fields
                .get(2)
                .filter(|b| !b.is_empty())
                .map(|b| (*b).to_string()),
            target: None,
        }),
        5 => {
            if kind != OwnedKind::Target || !fields[2].is_empty() {
                return None;
            }
            let origin = TargetOrigin::parse(fields[3])?;
            let checkout = PathBuf::from(fields[4]);
            if !checkout.is_absolute() {
                return None;
            }
            Some(OwnedPath {
                kind,
                path,
                branch: None,
                target: Some(TargetRecord { origin, checkout }),
            })
        }
        _ => None,
    }
}

/// Formats one `owned-paths` line, in the same form it was read: three
/// fields for a legacy line, five for a target line with a record.
pub(crate) fn format_owned_line(item: &OwnedPath) -> Vec<u8> {
    let mut line = item.kind.as_str().as_bytes().to_vec();
    line.push(b'\t');
    line.extend_from_slice(item.path.as_os_str().as_bytes());
    line.push(b'\t');
    match &item.target {
        Some(record) => {
            line.push(b'\t');
            line.extend_from_slice(record.origin.as_str().as_bytes());
            line.push(b'\t');
            line.extend_from_slice(record.checkout.as_os_str().as_bytes());
        }
        None => {
            if let Some(branch) = &item.branch {
                line.extend_from_slice(branch.as_bytes());
            }
        }
    }
    line.push(b'\n');
    line
}

/// Reads a file of at most `max` bytes without following a symlink.
pub(crate) fn read_capped_nofollow(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    read_capped_nofollow_with_metadata(path, max).map(|(bytes, _)| bytes)
}

/// [`read_capped_nofollow`], also returning the metadata of the handle that
/// was read, so owner and mode describe that file and not a later one.
pub(crate) fn read_capped_nofollow_with_metadata(
    path: &Path,
    max: u64,
) -> std::io::Result<(Vec<u8>, fs::Metadata)> {
    // O_NONBLOCK: opening a FIFO with no writer returns at once instead of
    // blocking; it has no effect on a regular file.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let mut buf = Vec::new();
    file.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("{} is larger than {max} bytes", path.display()),
        ));
    }
    Ok((buf, metadata))
}

/// The `owned-paths` entries of a run folder (empty when the file is missing).
pub(crate) fn read_owned_paths(run_dir: &Path) -> std::io::Result<Vec<OwnedPath>> {
    match read_capped_nofollow(&run_dir.join(OWNED_PATHS), OWNED_PATHS_MAX_BYTES) {
        Ok(bytes) => Ok(parse_owned_paths(&bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Writes `bytes` to `path` atomically, with mode 0600 and no symlink
/// following.
pub(crate) fn write_atomic_0600(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .context("atomic write target has no file name")?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot remove stale {}", tmp.display()));
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("cannot create {}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    drop(file);
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        let _ = fs::remove_file(&tmp);
        bail!("refusing to replace symbolic link {}", path.display());
    }
    fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))?;
    fs::File::open(path.parent().context("atomic write parent")?)?.sync_all()?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Attempt {
    pub correlation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StepStatus {
    pub id: String,
    pub status: String,
}

/// `run.json`: status data only, never environment, prompts or step output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RunReport {
    pub schema_version: u32,
    pub run_id: String,
    #[serde(default)]
    pub recipe: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub pid_start_ticks: Option<u64>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub steps: Vec<StepStatus>,
    #[serde(default)]
    pub log_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_recording: Option<serde_json::Value>,
}

impl RunReport {
    pub(crate) fn read(run_dir: &Path) -> Result<Self> {
        let path = run_dir.join(RUN_JSON);
        let bytes = read_capped_nofollow(&path, RUN_JSON_MAX_BYTES)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("cannot parse {}", path.display()))
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success,
    Failure,
    Interrupted,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Interrupted => "interrupted",
        }
    }
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// One run's folder and report, created when `amplihack recipe run` starts.
#[derive(Debug)]
pub(crate) struct RunScope {
    pub work_root: WorkRoot,
    pub run_id: String,
    pub run_dir: PathBuf,
    report: RunReport,
}

impl RunScope {
    /// Creates the run folder, writes the first `run.json` and creates
    /// `runtime/tmp` and `runtime/artifacts`. The returned guard removes
    /// `runtime/` when dropped.
    pub(crate) fn begin(
        work_root: WorkRoot,
        recipe: &str,
        cwd: &Path,
    ) -> Result<(Self, RuntimeGuard)> {
        work_root.ensure_created()?;
        let runs = work_root.runs_dir();
        create_private_dir_all(&runs)?;
        let run_id = new_run_id();
        let run_dir = runs.join(&run_id);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&run_dir)
            .with_context(|| format!("cannot create run folder {}", run_dir.display()))?;
        fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot set mode 0700 on {}", run_dir.display()))?;
        let pid = std::process::id();
        let report = RunReport {
            schema_version: RUN_SCHEMA_VERSION,
            run_id: run_id.clone(),
            recipe: recipe.to_string(),
            pid,
            pid_start_ticks: pid_start_ticks(pid),
            cwd: cwd.display().to_string(),
            started_at: now_rfc3339(),
            ended_at: None,
            outcome: Outcome::Interrupted.as_str().to_string(),
            attempts: Vec::new(),
            steps: Vec::new(),
            log_path: None,
            git_recording: None,
        };
        let runtime_dir = runtime_dir_for(&work_root.path, &run_id);
        let scope = Self {
            work_root,
            run_id,
            run_dir,
            report,
        };
        scope.write_report()?;
        create_private_dir_all(runtime_dir.parent().context("runtime parent")?)?;
        fs::DirBuilder::new().mode(0o700).create(&runtime_dir)?;
        let guard = RuntimeGuard::new(runtime_dir.clone())?;
        create_private_dir_all(&runtime_dir.join("tmp"))?;
        create_private_dir_all(&runtime_dir.join("artifacts"))?;
        Ok((scope, guard))
    }

    fn write_report(&self) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(&self.report)?;
        bytes.push(b'\n');
        write_atomic_0600(&self.run_dir.join(RUN_JSON), &bytes)
    }
}

/// The runtime space of one `amplihack recipe run`: a run folder under the
/// work root, or (when the work root cannot be used) a private temp folder as
/// in earlier releases.
#[derive(Debug)]
pub(crate) enum RunWorkspace {
    Scoped {
        scope: Box<RunScope>,
        guard: RuntimeGuard,
    },
    Legacy {
        dir: tempfile::TempDir,
    },
}

impl RunWorkspace {
    /// Appends one attempt to `run.json` and rewrites it, propagating any
    /// write error. A dry-run workspace (`dry_runtime`) has no run folder and
    /// records nothing, so this is `Ok(())` for it. Real runs are always
    /// `Scoped`: `begin_durable` never falls back to a temporary folder.
    pub(crate) fn record_attempt_checked(&mut self, correlation_id: &str) -> Result<()> {
        let Self::Scoped { scope, .. } = self else {
            return Ok(());
        };
        scope.report.attempts.push(Attempt {
            correlation_id: correlation_id.to_string(),
        });
        scope.write_report()
    }
    /// Production finalization propagates report publication failures.
    pub(crate) fn finish_checked(
        self,
        outcome: Outcome,
        steps: Vec<StepStatus>,
        log_path: Option<String>,
        sweep: bool,
    ) -> Result<()> {
        match self {
            Self::Scoped { mut scope, guard } => {
                let recording = scope.run_dir.join("git-recording.json");
                match read_capped_nofollow(&recording, 4096) {
                    Ok(bytes) => scope.report.git_recording = Some(serde_json::from_slice(&bytes)?),
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).context("read required recording finalization");
                    }
                }
                let runtime = guard.remove();
                scope.report.ended_at = Some(now_rfc3339());
                scope.report.outcome = if runtime.is_err() {
                    Outcome::Failure
                } else {
                    outcome
                }
                .as_str()
                .to_string();
                scope.report.steps = steps;
                scope.report.log_path = log_path;
                let report = scope.write_report();
                match (runtime, report) {
                    (Err(runtime), Err(report)) => {
                        return Err(runtime).context(format!("run report also failed: {report:#}"));
                    }
                    (Err(error), _) | (_, Err(error)) => return Err(error),
                    (Ok(()), Ok(())) => {}
                }
                if sweep && super::cleanup_on_exit_enabled() {
                    super::sweep::end_of_run(&scope.work_root);
                }
                Ok(())
            }
            Self::Legacy { dir } => dir.close().context("release legacy recipe runtime"),
        }
    }
    /// Recovery never silently executes without durable admission.
    pub(crate) fn begin_durable(recipe: &str, cwd: &Path) -> Result<Self> {
        let root = super::work_root::resolve_from_env()?;
        recipe_runner_rs::recovery::private_fs::Directory::open(&root.path, true)?;
        let (scope, guard) = RunScope::begin(root, recipe, cwd)?;
        Ok(Self::Scoped {
            scope: Box::new(scope),
            guard,
        })
    }
    /// A dry run owns only temporary scratch space and never a run scope.
    pub(crate) fn dry_runtime() -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("amplihack-dry-")
            .tempdir()?;
        create_private_dir_all(&dir.path().join("tmp"))?;
        create_private_dir_all(&dir.path().join("artifacts"))?;
        Ok(Self::Legacy { dir })
    }
    /// The durable run folder, or `None` for a dry run's temporary space.
    ///
    /// #2329: auto-drive keeps its private state in this folder, so it is
    /// checked once here: a real directory (not a symlink) owned by the
    /// current user and not writable by group or others.
    pub(crate) fn durable_run_dir(&self) -> Result<Option<&Path>> {
        let Self::Scoped { scope, .. } = self else {
            return Ok(None);
        };
        let run_dir = scope.run_dir.as_path();
        let metadata = fs::symlink_metadata(run_dir)
            .with_context(|| format!("cannot inspect run folder {}", run_dir.display()))?;
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "run folder {} is not a directory",
            run_dir.display()
        );
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o022 == 0,
            "run folder {} is not owned by the current user or is writable by group or others",
            run_dir.display()
        );
        Ok(Some(run_dir))
    }

    pub(crate) fn runtime_dir(&self) -> &Path {
        match self {
            Self::Scoped { guard, .. } => guard.path(),
            Self::Legacy { dir } => dir.path(),
        }
    }

    /// The work root of a durable run, or `None` for a dry run.
    pub(crate) fn work_root_path(&self) -> Option<PathBuf> {
        match self {
            Self::Scoped { scope, .. } => Some(scope.work_root.path.clone()),
            Self::Legacy { .. } => None,
        }
    }

    /// The durable run folder `<work root>/runs/<run id>/`; `None` for a dry
    /// run, which has only a temporary folder (issue #2096) and so writes no
    /// process record either (issue #2175).
    pub(crate) fn run_dir(&self) -> Option<&Path> {
        match self {
            Self::Scoped { scope, .. } => Some(&scope.run_dir),
            Self::Legacy { .. } => None,
        }
    }

    /// The variables every step sees.
    pub(crate) fn env(&self) -> Vec<(&'static str, std::ffi::OsString)> {
        let runtime = self.runtime_dir().to_path_buf();
        let tmp = runtime.join("tmp").into_os_string();
        let mut vars = vec![
            (
                "AMPLIHACK_WORKFLOW_RUNTIME_DIR",
                runtime.clone().into_os_string(),
            ),
            ("AMPLIHACK_RUNTIME_ROOT", runtime.clone().into_os_string()),
            (
                "AMPLIHACK_WORKFLOW_ARTIFACT_DIR",
                runtime.join("artifacts").into_os_string(),
            ),
            ("TMPDIR", tmp.clone()),
            ("TMP", tmp.clone()),
            ("TEMP", tmp),
        ];
        if let Self::Scoped { scope, .. } = self {
            vars.push((
                super::work_root::WORK_ROOT_ENV,
                scope.work_root.path.clone().into_os_string(),
            ));
            vars.push((
                super::work_root::WORK_ROOT_EXPLICIT_ENV,
                if scope.work_root.explicit { "1" } else { "0" }.into(),
            ));
            vars.push(("AMPLIHACK_RUN_ID", scope.run_id.clone().into()));
            vars.push(("AMPLIHACK_RUN_DIR", scope.run_dir.clone().into_os_string()));
        } else {
            // Fallback mode: do not let steps inherit a parent run's folder
            // (they would record into its owned-paths) or a refused explicit
            // work root (worktrees would still go there). Empty means unset
            // to every consumer.
            vars.push((super::work_root::WORK_ROOT_EXPLICIT_ENV, "0".into()));
            vars.push(("AMPLIHACK_RUN_ID", "".into()));
            vars.push(("AMPLIHACK_RUN_DIR", "".into()));
        }
        vars
    }
}

#[cfg(test)]
#[path = "tests_run_dir.rs"]
mod tests;
