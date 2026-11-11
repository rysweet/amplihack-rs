//! Required Linux descendant recording for recipe execution.
//!
//! Production uses successful ptrace exec events and per-attempt journals,
//! including scoped acknowledgements for independently owned nested runs.
//! Collection and publication errors fail execution. Evidence covers named
//! Git executables in the observed runner tree, not unrelated host processes,
//! arbitrary non-Git file writes or tamper resistance against the same user.
//!
//! The cooperative Trace2 parser below remains a diagnostic compatibility
//! API for the retained experiments; production never uses it as coverage.

use crate::commands::cleanup::run_dir::write_atomic_0600;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
#[path = "../../../run_isolation/observer/linux.rs"]
pub(crate) mod observer;

/// The raw Trace2 folder inside the run folder.
pub(crate) const TRACE_DIR: &str = "git-trace2";
/// The summary written into the run folder.
pub(crate) const SUMMARY_FILE: &str = "git-commands.jsonl";

/// Exactly the variables git reports with each process.
const ENV_VARS: &str = "GIT_DIR,GIT_INDEX_FILE,GIT_WORK_TREE,PWD";
/// Trace targets or settings the caller may have set; never passed on.
const CALLER_TRACE_VARS: &[&str] = &[
    "GIT_TRACE2",
    "GIT_TRACE2_PERF",
    "GIT_TRACE2_CONFIG_PARAMS",
    "GIT_TRACE2_REDACT",
];
const EVENT_VAR: &str = "GIT_TRACE2_EVENT";
const ENV_VARS_VAR: &str = "GIT_TRACE2_ENV_VARS";

const MAX_LINE_BYTES: u64 = 1024 * 1024;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Subcommands that can write an index.
const INDEX_WRITING: &[&str] = &[
    "add",
    "am",
    "apply",
    "checkout",
    "cherry-pick",
    "commit",
    "merge",
    "mv",
    "pull",
    "read-tree",
    "rebase",
    "reset",
    "restore",
    "revert",
    "rm",
    "stash",
    "switch",
    "update-index",
    "worktree",
];

/// Global options whose value is the next argument when not joined by `=`.
const VALUE_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--super-prefix",
];

/// One recorded git process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Entry {
    pub time: Option<String>,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub git_dir: Option<String>,
    pub index_file: Option<String>,
    pub sid: String,
}

/// The run's git command record.
#[derive(Debug)]
pub(crate) struct GitRecord {
    state: State,
    required: bool,
}

#[derive(Debug)]
enum State {
    Disabled,
    Enabled {
        run_dir: PathBuf,
        trace_dir: PathBuf,
    },
}

impl GitRecord {
    /// Production recording is required. Absence of durable storage or a
    /// supported observer must reject execution before the runner starts.
    pub(crate) fn begin(run_dir: Option<&Path>) -> Result<Self> {
        if !cfg!(target_os = "linux") {
            bail!(
                "required recipe process observation is supported on Linux only; run this recipe on Linux"
            );
        }
        let mut record = Self::prepare(run_dir);
        if matches!(record.state, State::Disabled) {
            bail!(
                "cannot initialize required Git recording; provide a writable absolute AMPLIHACK_WORK_ROOT and retry"
            );
        }
        record.required = true;
        Ok(record)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn spawn(
        &self,
        command: Command,
        attempt_id: &str,
    ) -> Result<(std::process::Child, observer::Observer)> {
        let State::Enabled { run_dir, trace_dir } = &self.state else {
            bail!("required Git recorder is unavailable");
        };
        let path = trace_dir.join(format!("observer-{attempt_id}.jsonl"));
        let run_id = run_dir
            .file_name()
            .context("recording run id")?
            .to_string_lossy()
            .into_owned();
        observer::Observer::spawn(command, path, run_id, attempt_id.to_string())
    }
    /// Creates `<run_dir>/git-trace2/` with mode 0700. With no run folder, or
    /// when the folder cannot be created (including when anything already
    /// exists at that path), the run continues without a record.
    pub(crate) fn prepare(run_dir: Option<&Path>) -> Self {
        let Some(run_dir) = run_dir else {
            eprintln!(
                "amplihack: warning: this run has no run folder, so it keeps no record of \
                 its git commands"
            );
            return Self::disabled();
        };
        let trace_dir = run_dir.join(TRACE_DIR);
        let created = fs::DirBuilder::new()
            .mode(0o700)
            .create(&trace_dir)
            .and_then(|()| fs::set_permissions(&trace_dir, fs::Permissions::from_mode(0o700)));
        match created {
            Ok(()) => Self {
                required: false,
                state: State::Enabled {
                    run_dir: run_dir.to_path_buf(),
                    trace_dir,
                },
            },
            Err(error) => {
                eprintln!(
                    "amplihack: warning: cannot create {} ({:?}); this run keeps no record of \
                     its git commands",
                    trace_dir.display(),
                    error.kind()
                );
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            state: State::Disabled,
            required: false,
        }
    }

    /// Points the runner's git processes at the record's folder, or, with no
    /// record, removes every trace target. The caller's other trace
    /// variables are always removed, so no step's command line reaches a file
    /// the caller chose.
    ///
    /// Call after `RunCheckout::apply_to` and before the workspace variables
    /// are added.
    pub(crate) fn apply_to(&self, command: &mut Command) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self.required {
            let parent = observer::parent_observer()?.unwrap_or_else(std::process::id);
            command.env("AMPLIHACK_OBSERVER_PID", parent.to_string());
            for key in [
                "AMPLIHACK_OBSERVER_JOURNAL",
                "AMPLIHACK_OBSERVER_RUN",
                "AMPLIHACK_OBSERVER_ATTEMPT",
                "AMPLIHACK_OBSERVER_ACK_FD",
            ] {
                command.env_remove(key);
            }
        }
        for var in CALLER_TRACE_VARS {
            command.env_remove(var);
        }
        match &self.state {
            State::Enabled { trace_dir, .. } => {
                if self.required {
                    // Native supervision does not rely on variables surviving
                    // the external runner's bounded environment adapter.
                    command.env_remove(EVENT_VAR).env_remove(ENV_VARS_VAR);
                    return Ok(());
                }
                // A trailing slash makes git write one file per process.
                let mut target = trace_dir.clone().into_os_string();
                target.push("/");
                command.env(EVENT_VAR, target);
                command.env(ENV_VARS_VAR, ENV_VARS);
            }
            State::Disabled => {
                command.env_remove(EVENT_VAR);
                command.env_remove(ENV_VARS_VAR);
            }
        }
        Ok(())
    }

    /// Writes `<run_dir>/git-commands.jsonl` (empty when nothing matched)
    /// and deletes the raw folder. If the summary cannot be written the raw
    /// folder is kept and a warning says so.
    pub(crate) fn finish(self) -> Result<()> {
        let State::Enabled { run_dir, trace_dir } = self.state else {
            return Ok(());
        };
        let mut text = String::new();
        if self.required {
            let metadata = fs::symlink_metadata(&trace_dir)
                .context("required collector storage disappeared")?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!("required collector storage was replaced; partial evidence retained");
            }
            let mut attempts = 0;
            for entry in fs::read_dir(&trace_dir)? {
                let entry = entry?;
                if !entry.file_name().to_string_lossy().starts_with("observer-")
                    || entry.path().extension().is_none_or(|e| e != "jsonl")
                {
                    continue;
                }
                let receipt: Value = serde_json::from_slice(
                    &crate::commands::cleanup::run_dir::read_capped_nofollow(
                        &entry.path().with_extension("receipt"),
                        4096,
                    )
                    .context("required observer did not finalize; partial evidence retained")?,
                )?;
                let filename = entry.file_name();
                let filename = filename
                    .to_str()
                    .context("non-UTF-8 observer journal name")?;
                let attempt = filename
                    .strip_prefix("observer-")
                    .and_then(|name| name.strip_suffix(".jsonl"))
                    .context("invalid observer attempt journal name")?;
                let run = run_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("invalid recording run identity")?;
                if receipt["schema_version"] != 1
                    || receipt["coverage"] != "established"
                    || receipt["complete"] != true
                    || receipt["run_id"] != run
                    || receipt["attempt_id"] != attempt
                    || receipt["backend"] != "linux-ptrace-exec"
                    || receipt["scope"] != "runner-descendants"
                {
                    bail!(
                        "required observer coverage receipt is incomplete or belongs to another attempt"
                    );
                }
                let file = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(entry.path())?;
                let metadata = file.metadata()?;
                if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                    bail!("invalid required observer journal; partial evidence retained");
                }
                let mut bytes = Vec::new();
                file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_FILE_BYTES {
                    bail!("required observer evidence exceeded its storage bound");
                }
                let lines = std::str::from_utf8(&bytes)?;
                for line in lines.lines() {
                    if line.len() as u64 > MAX_LINE_BYTES {
                        bail!("required observer command exceeded its line bound");
                    }
                    let value: Value =
                        serde_json::from_str(line).context("invalid required command evidence")?;
                    if value["schema_version"] != 1
                        || value["run_id"] != run
                        || value["attempt_id"] != attempt
                        || value["source"] != "linux-ptrace-exec"
                        || !value["argv"].is_array()
                        || !value["executable"].is_string()
                        || !value["cwd"].is_string()
                        || !value["git_dir"].is_string()
                        || !value["index_file"].is_string()
                    {
                        bail!("required command evidence is incomplete or misattributed");
                    }
                }
                text.push_str(lines);
                attempts += 1;
            }
            if attempts == 0 {
                bail!(
                    "required observer never established coverage; no completed observation attempt"
                );
            }
        } else {
            for entry in summarise(&trace_dir) {
                let line = serde_json::to_string(&entry)?;
                text.push_str(&line);
                text.push('\n');
            }
        }
        let summary = run_dir.join(SUMMARY_FILE);
        if let Err(error) = write_atomic_0600(&summary, text.as_bytes()) {
            eprintln!(
                "amplihack: warning: cannot write {} ({error:#}); keeping {}, which may \
                 contain command lines",
                summary.display(),
                trace_dir.display()
            );
            return Err(error).context("cannot publish required Git command evidence");
        }
        if self.required {
            let diagnostics = run_dir.join("diagnostics");
            fs::DirBuilder::new().mode(0o700).create(&diagnostics)?;
            let directory = diagnostics.join("git");
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
            write_atomic_0600(&directory.join("commands.jsonl"), text.as_bytes())?;
            write_atomic_0600(
                &run_dir.join("git-recording.json"),
                &serde_json::to_vec(
                    &serde_json::json!({"schema_version":1,"coverage":"established","complete":true,"scope":"runner-descendants","backend":"linux-ptrace-exec"}),
                )?,
            )?;
            fs::remove_dir_all(&trace_dir).context("cannot release finalized observer storage")?;
        } else {
            remove_trace_dir(&trace_dir);
        }
        Ok(())
    }

    /// Removes the still-empty folder without writing a summary, for a run
    /// that stopped before its runner started.
    pub(crate) fn discard(self) {
        let State::Enabled { trace_dir, .. } = self.state else {
            return;
        };
        match fs::remove_dir(&trace_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "amplihack: warning: cannot remove {} ({:?})",
                trace_dir.display(),
                error.kind()
            ),
        }
    }
}

/// `remove_dir_all` removes a symlink itself rather than following it.
fn remove_trace_dir(trace_dir: &Path) {
    match fs::remove_dir_all(trace_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "amplihack: warning: cannot remove {} ({:?}); it may contain command lines",
            trace_dir.display(),
            error.kind()
        ),
    }
}

/// What the events of one git process said.
#[derive(Default)]
struct Process {
    time: Option<String>,
    argv: Option<Vec<String>>,
    pwd: Option<String>,
    git_dir: Option<String>,
    index_file: Option<String>,
    worktree: Option<String>,
    resolved_subcommand: Option<String>,
}

/// The index-writing git processes recorded in `trace_dir`, sorted by start
/// time, then sid; a process with no start time sorts first. A missing or
/// symlinked `trace_dir` gives an empty list.
pub(crate) fn summarise(trace_dir: &Path) -> Vec<Entry> {
    match fs::symlink_metadata(trace_dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => {
            eprintln!(
                "amplihack: warning: {} is not a directory; its git command record is not read",
                trace_dir.display()
            );
            return Vec::new();
        }
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!(
                "amplihack: warning: cannot inspect {} ({:?})",
                trace_dir.display(),
                error.kind()
            );
            return Vec::new();
        }
    }
    let read = match fs::read_dir(trace_dir) {
        Ok(read) => read,
        Err(error) => {
            eprintln!(
                "amplihack: warning: cannot read {} ({:?})",
                trace_dir.display(),
                error.kind()
            );
            return Vec::new();
        }
    };

    let mut processes: BTreeMap<String, Process> = BTreeMap::new();
    for item in read {
        let path = match item {
            Ok(item) => item.path(),
            Err(error) => {
                eprintln!(
                    "amplihack: warning: cannot list {} ({:?})",
                    trace_dir.display(),
                    error.kind()
                );
                continue;
            }
        };
        if let Err(error) = read_event_file(&path, &mut processes) {
            eprintln!(
                "amplihack: warning: skipped git trace file {} ({:?})",
                path.display(),
                error.kind()
            );
        }
    }

    let mut entries: Vec<Entry> = processes
        .into_iter()
        .filter_map(|(sid, process)| {
            let argv = process.argv?;
            let sub = process
                .resolved_subcommand
                .as_deref()
                .or_else(|| subcommand(&argv))?;
            if !INDEX_WRITING.contains(&sub) {
                return None;
            }
            Some(Entry {
                time: process.time,
                argv: redact(&argv),
                cwd: process.pwd.or(process.worktree),
                git_dir: process.git_dir,
                index_file: process.index_file,
                sid,
            })
        })
        .collect();
    entries.sort_by(|a, b| (&a.time, &a.sid).cmp(&(&b.time, &b.sid)));
    entries
}

/// Reads one event file into `processes`. Anything that is not a regular
/// file is skipped silently: a step can put symlinks, folders or FIFOs here.
fn read_event_file(path: &Path, processes: &mut BTreeMap<String, Process>) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Ok(());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(std::io::Error::new(
            ErrorKind::FileTooLarge,
            "over the 64 MiB limit",
        ));
    }
    // O_NONBLOCK: a FIFO swapped in after the check above opens without
    // waiting for a writer, and is then refused by the type check below.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Ok(());
    }
    let mut reader = BufReader::new(file.take(MAX_FILE_BYTES));
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take(MAX_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') && line.len() as u64 > MAX_LINE_BYTES {
            skip_rest_of_line(&mut reader)?;
            continue;
        }
        if let Ok(event) = serde_json::from_slice::<Value>(&line) {
            apply_event(&event, processes);
        }
    }
}

fn skip_rest_of_line(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(());
        }
        if let Some(at) = buf.iter().position(|b| *b == b'\n') {
            reader.consume(at + 1);
            return Ok(());
        }
        let len = buf.len();
        reader.consume(len);
    }
}

fn apply_event(event: &Value, processes: &mut BTreeMap<String, Process>) {
    let Some(sid) = event["sid"].as_str() else {
        return;
    };
    let text = |key: &str| event[key].as_str().map(str::to_string);
    match event["event"].as_str() {
        Some("start") => {
            let Some(argv) = event["argv"].as_array() else {
                return;
            };
            let argv: Option<Vec<String>> = argv
                .iter()
                .map(|a| a.as_str().map(str::to_string))
                .collect();
            let process = processes.entry(sid.to_string()).or_default();
            process.argv = argv;
            process.time = text("time");
        }
        Some("def_param") => {
            let process = processes.entry(sid.to_string()).or_default();
            let value = text("value");
            match event["param"].as_str() {
                Some("PWD") => process.pwd = value,
                Some("GIT_DIR") => process.git_dir = value,
                Some("GIT_INDEX_FILE") => process.index_file = value,
                _ => {}
            }
        }
        Some("def_repo") => {
            processes.entry(sid.to_string()).or_default().worktree = text("worktree");
        }
        Some("cmd_name") => {
            processes
                .entry(sid.to_string())
                .or_default()
                .resolved_subcommand = text("name");
        }
        _ => {}
    }
}

/// The subcommand, after git's global options and their values.
fn subcommand(argv: &[String]) -> Option<&str> {
    let mut i = 1;
    while let Some(arg) = argv.get(i) {
        if VALUE_OPTIONS.contains(&arg.as_str()) {
            i += 2;
        } else if arg.starts_with('-') {
            i += 1;
        } else {
            return Some(arg);
        }
    }
    None
}

/// The argv as written to the record: `-c` values among the global options
/// become `<key>=<redacted>`, and URL user information anywhere becomes
/// `<redacted>`. Classification uses the raw argv.
fn redact(argv: &[String]) -> Vec<String> {
    let mut out: Vec<String> = argv.iter().map(|a| redact_url(a)).collect();
    let mut i = 1;
    while i < argv.len() {
        let arg = argv[i].as_str();
        if VALUE_OPTIONS.contains(&arg) {
            if arg == "-c"
                && let Some(value) = argv.get(i + 1)
            {
                out[i + 1] = match value.split_once('=') {
                    Some((key, _)) => format!("{key}=<redacted>"),
                    None => value.clone(),
                };
            }
            i += 2;
        } else if arg.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    out
}

/// `scheme://user:secret@host/path` becomes `scheme://<redacted>@host/path`.
fn redact_url(arg: &str) -> String {
    let Some(scheme_end) = arg.find("://") else {
        return arg.to_string();
    };
    let authority_start = scheme_end + 3;
    let rest = &arg[authority_start..];
    let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_len].rfind('@') {
        Some(at) => format!(
            "{}<redacted>{}",
            &arg[..authority_start],
            &arg[authority_start + at..]
        ),
        None => arg.to_string(),
    }
}
