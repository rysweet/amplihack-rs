//! The per-run folder `<work root>/runs/<run id>/` (issue #1808, D2, D3, D4, R3).
//!
//! It holds the run report `run.json`, the `owned-paths` list that helpers
//! append to, `cleanup-record.json` after a sweep removed something, and the
//! ephemeral `runtime/` folder that [`RuntimeGuard`] removes when the run ends.

use super::run_tmp_root::{
    RUN_TMP_ROOT_ENV, RuntimeChoice, TmpRootInputs, choose_runtime_dir, usable_explicit_root,
};
use super::work_root::{WORK_ROOT_ENV, WorkRoot, create_private_dir_all};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) const RUN_JSON: &str = "run.json";
pub(crate) const OWNED_PATHS: &str = "owned-paths";
pub(crate) const CLEANUP_RECORD: &str = "cleanup-record.json";
pub(crate) const RUN_SCHEMA_VERSION: u32 = 1;

/// Longest `runtime/tmp` path the default layout may produce, leaving room
/// under the 108-byte Unix socket path limit.
pub(crate) const MAX_RUNTIME_TMP_BYTES: usize = 80;
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
}

impl RuntimeGuard {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().unwrap_or(Path::new(""))
    }

    /// Removes the folder now and reports a failure as a warning.
    pub(crate) fn remove(mut self) {
        self.remove_inner();
    }

    fn remove_inner(&mut self) {
        let Some(path) = self.path.take() else {
            return;
        };
        match fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not remove the run's runtime folder");
                eprintln!(
                    "amplihack: warning: could not remove the run's runtime folder {}: {error}",
                    path.display()
                );
            }
        }
    }
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        self.remove_inner();
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
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut buf = Vec::new();
    file.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("{} is larger than {max} bytes", path.display()),
        ));
    }
    Ok(buf)
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
    fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
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
    /// The resolved per-run folder under an external root (issue #2291).
    /// Absent for the default layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_dir: Option<String>,
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
    /// The resolved external root the runtime folder lives in, if any
    /// (issue #2291); exported to steps as `AMPLIHACK_RUN_TMP_ROOT`.
    pub runtime_root: Option<PathBuf>,
    report: RunReport,
}

impl RunScope {
    /// Creates the run folder, chooses the runtime folder (issue #2291),
    /// writes the first `run.json` and creates `tmp/` and `artifacts/` in
    /// the runtime folder. The returned guard removes the runtime folder when
    /// dropped.
    pub(crate) fn begin(
        work_root: WorkRoot,
        recipe: &str,
        cwd: &Path,
    ) -> Result<(Self, RuntimeGuard)> {
        Self::begin_with(work_root, recipe, cwd, &TmpRootInputs::from_env())
    }

    pub(crate) fn begin_with(
        work_root: WorkRoot,
        recipe: &str,
        cwd: &Path,
        inputs: &TmpRootInputs,
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
        let chosen = choose_runtime_dir(&work_root.path, &run_id, inputs);
        if let Some(warning) = &chosen.warning {
            eprintln!("{warning}");
        }
        let (runtime_dir, runtime_root) = match chosen.choice {
            RuntimeChoice::Default(dir) => (dir, None),
            RuntimeChoice::External { dir, root } => (dir, Some(root)),
        };
        // Created before the first write, so a failed write still removes an
        // external folder the chooser has just made.
        let guard = RuntimeGuard::new(runtime_dir.clone());
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
            runtime_dir: runtime_root
                .as_ref()
                .map(|_| runtime_dir.display().to_string()),
        };
        let scope = Self {
            work_root,
            run_id,
            run_dir,
            runtime_root,
            report,
        };
        scope.write_report()?;
        create_private_dir_all(&runtime_dir.join("tmp"))?;
        create_private_dir_all(&runtime_dir.join("artifacts"))?;
        Ok((scope, guard))
    }

    fn write_report(&self) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(&self.report)?;
        bytes.push(b'\n');
        write_atomic_0600(&self.run_dir.join(RUN_JSON), &bytes)
    }

    /// Records the correlation id of a new attempt; also refreshes the mtime
    /// of `run.json`, which dates a killed run.
    pub(crate) fn record_attempt(&mut self, correlation_id: &str) {
        self.report.attempts.push(Attempt {
            correlation_id: correlation_id.to_string(),
        });
        if let Err(error) = self.write_report() {
            eprintln!("amplihack: warning: could not update the run report: {error:#}");
        }
    }

    /// Finalises `run.json` with the end time, outcome and step summary.
    pub(crate) fn finish(
        &mut self,
        outcome: Outcome,
        steps: Vec<StepStatus>,
        log_path: Option<String>,
    ) {
        self.report.ended_at = Some(now_rfc3339());
        self.report.outcome = outcome.as_str().to_string();
        self.report.steps = steps;
        self.report.log_path = log_path;
        if let Err(error) = self.write_report() {
            eprintln!("amplihack: warning: could not finalise the run report: {error:#}");
        }
    }
}

/// The runtime space of one `amplihack recipe run`: a run folder under the
/// work root, or, for a dry run, a private temp folder.
#[derive(Debug)]
pub(crate) enum RunWorkspace {
    Scoped {
        scope: Box<RunScope>,
        guard: RuntimeGuard,
    },
    Legacy {
        dir: tempfile::TempDir,
        /// The resolved `AMPLIHACK_RUN_TMP_ROOT` the folder was made in,
        /// when rule 1 accepted it (issue #2291, D12).
        explicit_root: Option<PathBuf>,
    },
}

impl RunWorkspace {
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
    /// It still honours `AMPLIHACK_RUN_TMP_ROOT` (issue #2291, D12).
    pub(crate) fn dry_runtime() -> Result<Self> {
        let inputs = TmpRootInputs::from_env();
        let explicit_root = match usable_explicit_root(&legacy_work_root(&inputs), &inputs) {
            Ok(root) => root,
            Err(warning) => {
                eprintln!("{warning}");
                None
            }
        };
        let mut builder = tempfile::Builder::new();
        builder.prefix("amplihack-dry-");
        let dir = match &explicit_root {
            Some(root) => builder.tempdir_in(root),
            None => builder.tempdir(),
        }?;
        create_private_dir_all(&dir.path().join("tmp"))?;
        create_private_dir_all(&dir.path().join("artifacts"))?;
        Ok(Self::Legacy { dir, explicit_root })
    }
    pub(crate) fn runtime_dir(&self) -> &Path {
        match self {
            Self::Scoped { guard, .. } => guard.path(),
            Self::Legacy { dir, .. } => dir.path(),
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
        // Issue #2291, D7: a sub-recipe applies rule 1 and makes a sibling
        // folder under the same root. Never the per-run folder itself.
        let runtime_root = match self {
            Self::Scoped { scope, .. } => scope.runtime_root.as_ref(),
            Self::Legacy { explicit_root, .. } => explicit_root.as_ref(),
        };
        if let Some(root) = runtime_root {
            vars.push((RUN_TMP_ROOT_ENV, root.clone().into_os_string()));
        }
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

    pub(crate) fn record_attempt(&mut self, correlation_id: &str) {
        if let Self::Scoped { scope, .. } = self {
            scope.record_attempt(correlation_id);
        }
    }

    /// Removes `runtime/`, finalises `run.json` and, when `sweep` is true and
    /// `AMPLIHACK_CLEANUP_ON_EXIT` is not `0`, runs the end-of-run sweep.
    pub(crate) fn finish(
        self,
        outcome: Outcome,
        steps: Vec<StepStatus>,
        log_path: Option<String>,
        sweep: bool,
    ) {
        match self {
            Self::Scoped { mut scope, guard } => {
                guard.remove();
                scope.finish(outcome, steps, log_path);
                if sweep && super::cleanup_on_exit_enabled() {
                    super::sweep::end_of_run(&scope.work_root);
                }
            }
            Self::Legacy { dir, .. } => {
                if let Err(error) = dir.close() {
                    eprintln!("amplihack: warning: could not remove the runtime folder: {error}");
                }
            }
        }
    }
}

/// The work root a legacy-mode run compares `AMPLIHACK_RUN_TMP_ROOT`
/// against: the one that was refused, as given, or the default under
/// `$HOME`. `$HOME/.amplihack` is checked separately.
fn legacy_work_root(inputs: &TmpRootInputs) -> PathBuf {
    match std::env::var_os(WORK_ROOT_ENV).filter(|v| !v.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => inputs
            .home
            .as_ref()
            .map(|h| h.join(".amplihack").join("work"))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
#[path = "tests_run_dir.rs"]
mod tests;
