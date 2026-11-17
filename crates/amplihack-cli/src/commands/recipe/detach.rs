//! `amplihack recipe run --detach` (issue #1606): run a recipe in a new tmux
//! session that amplihack creates, with a private log file.
//!
//! Callers used to hand-write `tmux new-session -d "... $(amplihack
//! agent-binary --shell) amplihack recipe run ..."` and get the quoting, the
//! agent hand-off or the log wrong. Here:
//!
//! 1. The parent resolves the agent binary as `recipe run` would, and writes
//!    the run's argv, working directory and environment (this shell's, minus
//!    every agent session marker, plus the tagged agent binary) to a 0600
//!    spec file in the 0700 log directory. Nothing travels through a shell or
//!    on a command line, so no value needs quoting and no secret shows in
//!    `ps`.
//! 2. `tmux new-session -d -s <session> -- amplihack recipe detached-child
//!    --log <log> <spec>` starts the pane. tmux runs a multi-argument command
//!    directly, without a shell; the working directory comes from the spec,
//!    not from `-c`, which tmux would format-expand.
//! 3. In the pane, [`run_detached_child`] reads and deletes the spec, starts
//!    the real `recipe run` with exactly that environment, writes a `started`
//!    line to the 0600 log, and copies the run's output to the pane and the
//!    log. Any failure before that is written to the log instead.
//! 4. The launcher waits for one of those two lines before it reports
//!    success, and removes the spec and context files on every way out.
//!
//! Killing the session sends SIGHUP to the pane's process group, and `recipe
//! run` then tears down its runner's tree (`run/stop_signal.rs`);
//! `amplihack recipe stop <session>` does the same and waits for it.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use amplihack_utils::agent_binary::{
    BINARY_ENV, Resolution, SESSION_MARKERS, SOURCE_ENV, export_tag,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::user_config::safe::read_bounded;

/// Variables the pane gets from tmux itself, kept from the pane's own
/// environment rather than the caller's.
const TMUX_VARS: &[&str] = &["TMUX", "TMUX_PANE"];

/// How long the pane has to start the run before the launch counts as
/// failed. Tests shorten it with [`START_TIMEOUT_ENV`].
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// Test-only override of [`START_TIMEOUT`], in seconds.
const START_TIMEOUT_ENV: &str = "AMPLIHACK_TEST_DETACH_START_SECS";

fn start_timeout() -> Duration {
    std::env::var(START_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        // An hour at most, so an absurd value cannot overflow the deadline.
        .map(|secs| Duration::from_secs(secs.min(3600)))
        .unwrap_or(START_TIMEOUT)
}

/// The first line a pane writes once the run is running.
const STARTED: &str = "amplihack: detached run started";

/// What the pane needs to start the run.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Spec {
    /// Arguments after the `amplihack` executable.
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub log: PathBuf,
    /// The private context file the run reads; removed when the run ends.
    pub context_file: PathBuf,
}

/// The line a pane writes to the log when the run cannot start.
const START_ERROR: &str = "amplihack: the detached run could not start:";

/// Inputs for a detached launch, from `recipe run`'s parsed arguments.
pub(crate) struct Launch<'a> {
    /// `recipe run` arguments to repeat in the pane, without `--detach` or
    /// any context flag.
    pub args: Vec<String>,
    /// The run's context, resolved here from `-c`, `--context-file` and
    /// `--context-from-file` (`KEY=VALUE`). It reaches the pane in a private
    /// file, never on a command line.
    pub context: &'a [String],
    pub recipe_path: &'a str,
    pub working_dir: &'a Path,
    pub agent_binary: &'a Resolution,
}

/// Start the run in a new tmux session and print where it is.
pub(crate) fn launch(request: Launch<'_>) -> Result<()> {
    let tmux_ok = Command::new("tmux")
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !tmux_ok {
        bail!("recipe run --detach needs tmux on PATH");
    }
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let context = context_object(request.context)?;
    let pattern = crate::user_config::tmux_session_pattern(request.working_dir);
    let base = session_name(
        &pattern,
        &recipe_label(request.recipe_path),
        &repo_label(request.working_dir),
        &chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string(),
    );
    let log_dir = log_dir()?;
    ensure_private_dir(&log_dir)?;
    // Trap SIGINT/SIGTERM/SIGHUP before any secret file exists, so an
    // interrupted launch still runs the guard below instead of dying.
    crate::commands::recipe::run::stop_signal::arm();
    let (session, log) = claim_session(&base, &log_dir)?;
    let mut secrets = SecretFiles {
        spec: log_dir.join(format!("{session}.spec.json")),
        context: log_dir.join(format!("{session}.context.json")),
        keep_context: false,
    };
    create_private(&secrets.context)?.write_all(&serde_json::to_vec(&context)?)?;
    let exe = std::env::current_exe().context("cannot find the amplihack executable")?;
    let mut args = request.args;
    args.push("--context-file".into());
    args.push(secrets.context.to_string_lossy().into_owned());
    let spec = Spec {
        args,
        cwd,
        env: child_env(request.agent_binary, &log),
        log: log.clone(),
        context_file: secrets.context.clone(),
    };
    create_private(&secrets.spec)?.write_all(&serde_json::to_vec(&spec)?)?;

    let mut tmux = Command::new("tmux");
    // Issue #1811: a tmux server started here keeps this environment for
    // every later session; the governor's parent and recipe markers must not
    // reach them. Issue #1976: nor the parent-watch and state-dir variables.
    for name in PANE_ENV_REMOVED {
        tmux.env_remove(name);
    }
    let started = tmux
        .args(["new-session", "-d", "-s", &session, "--"])
        .arg(&exe)
        .args(["recipe", "detached-child", "--log"])
        .arg(&log)
        .arg(&secrets.spec)
        .stdin(Stdio::null())
        .output()
        .context("failed to run tmux new-session")?;
    if !started.status.success() {
        bail!(
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&started.stderr).trim()
        );
    }

    match wait_for_start(&log, start_timeout()) {
        Start::Started => {
            // The run owns the context file now, and removes it when it ends.
            secrets.keep_context = true;
        }
        Start::Failed(reason) => {
            kill_session(&session);
            bail!("{reason}");
        }
        Start::Stopped(signal) => {
            kill_session(&session);
            return Err(anyhow::Error::new(
                crate::commands::recipe::run::stop_signal::StoppedBySignal(signal),
            ));
        }
        Start::TimedOut(after) => {
            kill_session(&session);
            bail!(
                "the run in tmux session {session} did not start within {after:?}; see {}",
                log.display()
            );
        }
    }
    drop(secrets);
    println!("session: {session}");
    println!("log: {}", log.display());
    eprintln!(
        "amplihack: detached. Follow with `tail -f {}` or `tmux attach -t {session}`; \
         stop with `amplihack recipe stop {session}`.",
        log.display()
    );
    Ok(())
}

/// The spec and context files of a launch. Both hold secrets (the caller's
/// environment, the task text), so dropping this removes them on every way
/// out of [`launch`]: an error, a signal, a timeout. Only a run that started
/// keeps the context file, which it removes itself.
struct SecretFiles {
    spec: PathBuf,
    context: PathBuf,
    keep_context: bool,
}

impl Drop for SecretFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.spec);
        if !self.keep_context {
            let _ = fs::remove_file(&self.context);
        }
    }
}

/// How a pane's start went, as its log tells it.
enum Start {
    Started,
    Failed(String),
    Stopped(i32),
    TimedOut(Duration),
}

/// What the log says about the start, decided by whichever of the `started`
/// line and a start-up error comes first: once the run has started, its own
/// output may contain anything, including a line that looks like the error.
fn start_in(log: &str) -> Option<Start> {
    log.lines().find_map(|line| {
        if line.starts_with(STARTED) {
            Some(Start::Started)
        } else if line.starts_with(START_ERROR) {
            Some(Start::Failed(line.to_string()))
        } else {
            None
        }
    })
}

/// Wait for the pane's `started` line or its start-up error in `log`.
fn wait_for_start(log: &Path, timeout: Duration) -> Start {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(start) = start_in(&fs::read_to_string(log).unwrap_or_default()) {
            return start;
        }
        if let Some(signal) = crate::commands::recipe::run::stop_signal::pending() {
            return Start::Stopped(signal);
        }
        if std::time::Instant::now() >= deadline {
            return Start::TimedOut(timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn kill_session(session: &str) {
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &format!("={session}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `KEY=VALUE` pairs as the JSON object `--context-file` reads.
fn context_object(pairs: &[String]) -> Result<serde_json::Map<String, serde_json::Value>> {
    pairs
        .iter()
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            )),
            _ => bail!("invalid context entry (expected KEY=VALUE): {pair}"),
        })
        .collect()
}

/// The first of `base`, `base-2`, `base-3`, ... that has no live tmux session
/// and whose log this call created. A pattern without `{ts}`, a stale log from
/// an earlier run, or a concurrent launch in the same second then moves on to
/// the next name instead of failing.
fn claim_session(base: &str, log_dir: &Path) -> Result<(String, PathBuf)> {
    for n in 1..=1000 {
        let session = if n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        if session_exists(&session) {
            continue;
        }
        let log = log_dir.join(format!("{session}.log"));
        match create_private(&log) {
            Ok(_) => return Ok((session, log)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    bail!("no free tmux session name for {base} after 1000 tries")
}

fn session_exists(session: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", &format!("={session}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Variables a detached pane never inherits. A pane root is a top-level run
/// (issue #1976, D8): it must not watch the launching run as its parent, nor
/// write into another drive's state directory. The governor markers are
/// issue #1811's.
pub(crate) const PANE_ENV_REMOVED: [&str; 8] = [
    "AMPLIHACK_RECOVERY_ATTACHMENT",
    "AMPLIHACK_RECOVERY_DESCRIPTOR",
    crate::commands::governor::GOVERNOR_PARENT_ENV,
    crate::commands::governor::GOVERNOR_RECIPE_ENV,
    super::run::parent_watch::PARENT_WATCH_ENV[0],
    super::run::parent_watch::PARENT_WATCH_ENV[1],
    super::run::parent_watch::PARENT_WATCH_ENV[2],
    // Issue #2535: a new top-level run reads only its own guidance.
    crate::run_guidance::GUIDANCE_DIRS_ENV,
];

/// Where a detached run's steps append their usage-limit wait lines (issue
/// #2731). One marker, named in one place, so the writer and the exporter
/// cannot drift.
pub(crate) const RECIPE_LOG_ENV: &str = crate::commands::profile::limit_wait::RECIPE_LOG_ENV;

/// Points `env` at this run's log. Set, never defaulted: a detached run
/// started from inside another one must append to its own log, so an
/// inherited value is replaced rather than kept.
fn with_recipe_log(env: &mut BTreeMap<String, String>, log: &Path) {
    env.insert(
        RECIPE_LOG_ENV.to_string(),
        log.to_string_lossy().into_owned(),
    );
}

/// Whether the pane may inherit `name`.
pub(crate) fn keeps_in_pane(name: &str) -> bool {
    !PANE_ENV_REMOVED.contains(&name)
}

/// The caller's environment for the run: every agent session marker removed,
/// and the resolved agent binary set with the tag `recipe run` would export.
/// The capacity governor's parent and recipe markers are removed too (issue
/// #1811): a detached run is independent work and asks for a slot of its own.
fn child_env(agent_binary: &Resolution, log: &Path) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| {
            !SESSION_MARKERS.iter().any(|(marker, _)| marker == k)
                && !TMUX_VARS.contains(&k.as_str())
                && keeps_in_pane(k)
        })
        .collect();
    env.insert(BINARY_ENV.to_string(), agent_binary.binary.clone());
    with_recipe_log(&mut env, log);
    match export_tag(&agent_binary.binary, agent_binary.source) {
        Some(tag) => env.insert(SOURCE_ENV.to_string(), tag),
        None => env.remove(SOURCE_ENV),
    };
    env
}

/// `recipe.log_dir`, or `~/.amplihack/recipe-logs`.
pub(crate) fn log_dir() -> Result<PathBuf> {
    if let Some(dir) = crate::user_config::recipe_log_dir() {
        return Ok(dir);
    }
    let home = crate::user_config::home_dir().context("HOME is not set; set recipe.log_dir")?;
    Ok(home.join(".amplihack").join("recipe-logs"))
}

/// Create `dir` (and missing parents) with mode 0700. An existing directory
/// must be owned by this user, and loses group and other access.
fn ensure_private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create the log directory {}", dir.display()))?;
    let meta = fs::metadata(dir)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        bail!("{} is not a directory you own", dir.display());
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(meta.mode() & 0o700))?;
    }
    Ok(())
}

/// Create a new file with mode 0600. Never opens an existing file or follows
/// a symlink planted at `path`.
fn create_private(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

/// Expand a validated pattern, then keep only characters tmux accepts in a
/// session name and nothing needs quoting for.
pub(crate) fn session_name(pattern: &str, recipe: &str, repo: &str, ts: &str) -> String {
    let expanded = pattern
        .replace("{recipe}", recipe)
        .replace("{repo}", repo)
        .replace("{ts}", ts);
    let name: String = expanded
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    if name.is_empty() {
        "amp".to_string()
    } else {
        name
    }
}

/// The recipe's file stem (or the name it was given by).
fn recipe_label(recipe_path: &str) -> String {
    Path::new(recipe_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("recipe")
        .to_string()
}

/// The basename of the git root above `dir`, or of `dir`.
fn repo_label(dir: &Path) -> String {
    let root = crate::user_config::files::git_root(dir).unwrap_or_else(|| dir.to_path_buf());
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo")
        .to_string()
}

/// The pane side: read and delete the spec, run `recipe run` with its
/// environment, and copy the output to the pane and the log. Returns the
/// run's exit code.
pub(crate) fn run_detached_child(spec_path: &Path, log_path: &Path) -> Result<i32> {
    // The log first: it is where a start-up error goes, and where the
    // launcher looks for one.
    let mut log = OpenOptions::new()
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log_path)
        .with_context(|| format!("cannot open the log {}", log_path.display()))?;
    let spec = match read_spec(spec_path).and_then(|spec| {
        if spec.log == log_path {
            Ok(spec)
        } else {
            Err(anyhow::anyhow!("the run spec names a different log"))
        }
    }) {
        Ok(spec) => spec,
        Err(error) => {
            start_failed(&mut log, spec_path, &error);
            return Err(error);
        }
    };
    let _ = fs::remove_file(spec_path);

    // Stay alive through SIGHUP/SIGINT/SIGTERM so the log keeps what
    // `recipe run` prints while it tears the tree down. The run itself gets
    // the default dispositions back at exec and arms its own handling.
    let ignored = Arc::new(AtomicBool::new(false));
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
        let _ = signal_hook::flag::register(signal, Arc::clone(&ignored));
    }

    let (mut child, mut reader) = match start_run(&spec) {
        Ok(started) => started,
        Err(error) => {
            // Every failure before the run is running goes to the log, where
            // the launcher is waiting for it.
            let _ = writeln!(log, "{START_ERROR} {error:#}");
            let _ = log.sync_all();
            let _ = fs::remove_file(&spec.context_file);
            return Err(error);
        }
    };
    let _ = writeln!(log, "{STARTED} (pid {})", child.id());
    let _ = log.sync_all();

    let copier = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        copy_output(&mut reader, &mut stdout, &mut log);
    });
    let status = child.wait().context("failed to wait for recipe run");
    let _ = fs::remove_file(&spec.context_file);
    let status = status?;
    // A process that outlived the run may still hold the pipe; do not wait
    // for it forever.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !copier.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    use std::os::unix::process::ExitStatusExt;
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
}

/// The most unterminated output the copier holds back before giving up on
/// the line boundary and writing what it has. The runner's line discipline
/// is not this code's to assume, and losing the log is worse than losing a
/// boundary, so the hold is bounded and the bytes are written, never
/// dropped.
const MAX_HELD: usize = 1 << 20;

/// Copies the run's output from `reader` to `stdout` and to `log` until the
/// pipe closes.
///
/// Stdout is byte for byte and flushed on every read, so interactive output
/// is unchanged. The recipe log is different: it is a shared sink, because a
/// step waiting out a usage limit appends its own `start` and `end` lines to
/// the same file (issue #2731). A write that leaves the log in the middle of
/// a line puts the next writer's line inside a runner line, which reads as
/// nonsense and is invisible to a line-oriented `grep`, so the wait stays
/// hidden in exactly the case it was added for. Each write to the log
/// therefore carries whole lines only, with any unterminated tail held until
/// the newline arrives, or until the pipe closes and the tail is written with
/// one.
///
/// The single exception is [`MAX_HELD`]: a tail that reaches it is written
/// as it stands, still unterminated. Holding it for ever would cost the log
/// and the memory both, and no newline is invented for it: one here would
/// split a single runner line into two lines the run never ended, and leave
/// a blank line behind whenever the run's own newline is the very next byte.
/// A line over a mebibyte long can therefore still be interleaved with;
/// nothing shorter can.
fn copy_output(reader: &mut impl Read, stdout: &mut impl Write, log: &mut impl Write) {
    let mut buf = [0u8; 8192];
    let mut held: Vec<u8> = Vec::new();
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        let _ = stdout.write_all(&buf[..n]);
        let _ = stdout.flush();
        held.extend_from_slice(&buf[..n]);
        match held.iter().rposition(|b| *b == b'\n') {
            Some(last) => {
                let _ = log.write_all(&held[..=last]);
                held.drain(..=last);
            }
            // Output that never ends a line must not be held for ever.
            None if held.len() >= MAX_HELD => {
                let _ = log.write_all(&held);
                held.clear();
            }
            None => {}
        }
    }
    if !held.is_empty() {
        held.push(b'\n');
        let _ = log.write_all(&held);
    }
}

/// Start the real `recipe run` with the spec's argv, directory and
/// environment, its output on a pipe this process copies.
fn start_run(spec: &Spec) -> Result<(std::process::Child, std::io::PipeReader)> {
    let (reader, writer) = std::io::pipe().context("cannot create the output pipe")?;
    let exe = std::env::current_exe().context("cannot find the amplihack executable")?;
    let mut command = Command::new(exe);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(
            writer
                .try_clone()
                .context("cannot duplicate the output pipe")?,
        )
        .stderr(writer);
    for name in TMUX_VARS {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to start recipe run in {}", spec.cwd.display()))?;
    Ok((child, reader))
}

/// Record why the run could not start, then delete the spec: the error line
/// is in the log before the launcher sees the spec gone.
fn start_failed(log: &mut File, spec_path: &Path, error: &anyhow::Error) {
    let _ = writeln!(log, "{START_ERROR} {error:#}");
    let _ = log.sync_all();
    if fs::symlink_metadata(spec_path).is_ok_and(|meta| meta.is_file()) {
        let _ = fs::remove_file(spec_path);
    }
}

/// The spec must be a regular file this user owns that only this user can
/// read: it holds the caller's environment.
fn read_spec(path: &Path) -> Result<Spec> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("cannot read the run spec {}", path.display()))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        bail!(
            "{} is not a private run spec owned by you; refusing it",
            path.display()
        );
    }
    let text = read_bounded(path)
        .map_err(|reason| anyhow::anyhow!("{}: {reason}", path.display()))?
        .unwrap_or_default();
    serde_json::from_str(&text).with_context(|| format!("{} is not a run spec", path.display()))
}

/// The `recipe run` arguments to repeat in the pane: everything the caller
/// passed except `--detach` and the context flags, plus `--no-detach`. The
/// context goes in a private file (see [`Launch::context`]).
pub(crate) fn pane_args(
    recipe_path: &str,
    dry_run: bool,
    verbose: bool,
    format: &str,
    working_dir: Option<&str>,
    step_timeout: Option<u64>,
) -> Vec<String> {
    let mut args: Vec<String> = vec!["recipe".into(), "run".into(), recipe_path.into()];
    if dry_run {
        args.push("--dry-run".into());
    }
    if verbose {
        args.push("--verbose".into());
    }
    args.extend(["--format".into(), format.into()]);
    if let Some(dir) = working_dir {
        args.extend(["--working-dir".into(), dir.into()]);
    }
    if let Some(seconds) = step_timeout {
        args.extend(["--step-timeout".into(), seconds.to_string()]);
    }
    args.push("--no-detach".into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_expand_and_stay_tmux_safe() {
        assert_eq!(
            session_name(
                "amp-{recipe}-{repo}-{ts}",
                "smart-orchestrator",
                "my.repo",
                "20261020-101500"
            ),
            "amp-smart-orchestrator-my-repo-20261020-101500"
        );
        assert_eq!(session_name("{repo}", "r", "a b:c'$(x)", "t"), "a-b-c---x-");
        assert_eq!(session_name("{recipe}", "", "", ""), "amp");
    }

    #[test]
    fn pane_args_repeat_everything_but_detach_and_context() {
        let args = pane_args("r.yaml", false, true, "table", Some("."), Some(30));
        assert_eq!(
            args,
            [
                "recipe",
                "run",
                "r.yaml",
                "--verbose",
                "--format",
                "table",
                "--working-dir",
                ".",
                "--step-timeout",
                "30",
                "--no-detach"
            ]
        );
    }

    #[test]
    fn the_first_of_started_and_error_decides() {
        let started = format!("{STARTED} (pid 7)\n{START_ERROR} printed by the run\n");
        assert!(matches!(start_in(&started), Some(Start::Started)));
        let failed = format!("{START_ERROR} no such directory\n");
        assert!(matches!(start_in(&failed), Some(Start::Failed(r)) if r.contains("no such")));
        assert!(start_in("nothing yet\n").is_none());
    }

    #[test]
    fn context_pairs_become_a_json_object() {
        let object =
            context_object(&["task_description=a = b".into(), "repo_path=.".into()]).unwrap();
        assert_eq!(object["task_description"], "a = b");
        assert_eq!(object["repo_path"], ".");
        assert!(context_object(&["novalue".into()]).is_err());
    }

    /// A pattern without `{ts}` leaves `<session>.log` behind after every
    /// run; the next launch takes the next free name instead of failing.
    #[test]
    fn a_leftover_log_moves_the_claim_to_the_next_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("fixed.log"), "old run").unwrap();
        fs::write(dir.path().join("fixed-2.log"), "old run").unwrap();
        let (session, log) = claim_session("fixed", dir.path()).unwrap();
        assert_eq!(session, "fixed-3");
        assert_eq!(log, dir.path().join("fixed-3.log"));
        assert_eq!(
            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("fixed.log")).unwrap(),
            "old run"
        );
    }

    #[test]
    fn a_spec_readable_by_others_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spec.json");
        let spec = Spec {
            args: vec!["recipe".into()],
            cwd: dir.path().to_path_buf(),
            env: BTreeMap::new(),
            log: dir.path().join("log"),
            context_file: dir.path().join("context.json"),
        };
        fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_spec(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_spec(&path).unwrap(), spec);
    }

    /// Issue #2582 (D9): the arguments a detached launch repeats in the pane
    /// carry no context flag and no context value. The context reaches the
    /// pane only in the private `<session>.context.json`, so the tmux argv
    /// holds paths only. The args are assembled as `commands::recipe` does.
    #[test]
    fn launch_args_hold_no_context_flag_or_value() {
        let token = "ZQXV2582DETACHWORD";
        let task = format!("task={token} fix it");
        let cli = crate::Cli::try_parse_from([
            "amplihack",
            "recipe",
            "run",
            "r.yaml",
            "-c",
            task.as_str(),
            "--context=repo_note=ZQXV2582NOTEWORD",
            "--detach",
            "--fresh",
            "-w",
            ".",
        ])
        .expect("recipe run arguments parse");
        let crate::Commands::Recipe {
            command:
                crate::RecipeCommands::Run {
                    recipe_path,
                    context,
                    dry_run,
                    verbose,
                    format,
                    working_dir,
                    step_timeout,
                    governor,
                    fresh,
                    ..
                },
        } = cli.command
        else {
            panic!("expected recipe run");
        };
        assert!(
            context.iter().any(|c| c.contains(token)),
            "the token was parsed"
        );
        let mut args = pane_args(
            &recipe_path,
            dry_run,
            verbose,
            &format,
            working_dir.as_deref(),
            step_timeout,
        );
        args.extend(governor.to_args());
        if fresh {
            args.push("--fresh".into());
        }
        for flag in ["-c", "--set", "--context", "--context-from-file"] {
            assert!(!args.iter().any(|a| a == flag), "{flag} in {args:?}");
        }
        assert!(
            !args.iter().any(|a| a.contains("ZQXV2582")),
            "a context value reached the pane argv: {args:?}"
        );
    }

    /// Issue #2582 (D11): the folder holding the spec and context files is
    /// private, even when it existed with wider modes.
    #[test]
    fn the_log_dir_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("a").join("recipe-logs");
        ensure_private_dir(&fresh).unwrap();
        assert_eq!(
            fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let wide = dir.path().join("wide");
        fs::create_dir(&wide).unwrap();
        fs::set_permissions(&wide, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&wide).unwrap();
        assert_eq!(
            fs::metadata(&wide).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    /// Issue #1976 (D8, C-S12): a detached run is a top-level run. Its pane
    /// must never watch the launching run as a parent, nor write into another
    /// drive's state directory. `child_env` and the `tmux new-session`
    /// command both drop every name in `PANE_ENV_REMOVED`, through the one
    /// predicate `keeps_in_pane`.
    #[test]
    fn the_pane_drops_the_parent_watch_and_state_dir_variables() {
        for name in [
            "AMPLIHACK_PARENT_RECIPE_PID",
            "AMPLIHACK_PARENT_RECIPE_STARTTIME",
            "AMPLIHACK_AUTODRIVE_STATE_DIR",
        ] {
            assert!(PANE_ENV_REMOVED.contains(&name), "{name} missing");
            assert!(!keeps_in_pane(name), "{name} reaches the pane");
        }
        assert!(!keeps_in_pane(
            crate::commands::governor::GOVERNOR_PARENT_ENV
        ));
        assert!(!keeps_in_pane(
            crate::commands::governor::GOVERNOR_RECIPE_ENV
        ));
        for kept in [
            "PATH",
            "HOME",
            "AMPLIHACK_HOME",
            "AMPLIHACK_PARENT_RECIPE_PIDX",
        ] {
            assert!(keeps_in_pane(kept), "{kept} must reach the pane");
        }
    }

    /// Issue #2731: a detached run tells its steps where its log is, so a
    /// step that waits out a usage limit can say so where the run is being
    /// watched. The value is set, never defaulted: a detached run started
    /// from inside another one must append to its own log, not the
    /// enclosing run's.
    #[test]
    fn the_recipe_log_marker_is_set_for_the_pane_and_overrides_any_inherited_one() {
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        env.insert("PATH".into(), "/usr/bin".into());
        env.insert(
            RECIPE_LOG_ENV.into(),
            "/tmp/an-enclosing-run.log".to_string(),
        );
        with_recipe_log(&mut env, Path::new("/logs/amp-run.log"));
        assert_eq!(env[RECIPE_LOG_ENV], "/logs/amp-run.log");
        assert_eq!(env["PATH"], "/usr/bin", "nothing else is touched");
    }

    #[test]
    fn the_recipe_log_marker_reaches_the_pane() {
        assert!(
            keeps_in_pane(RECIPE_LOG_ENV),
            "a pane that strips it would write no wait lines at all"
        );
        assert_eq!(
            RECIPE_LOG_ENV,
            crate::commands::profile::limit_wait::RECIPE_LOG_ENV,
            "both sides must name the same marker"
        );
    }

    // --- the recipe log carries whole lines (issue #2731) -----------------
    //
    // Two writers share a detached run's log: this copier, with the runner's
    // own output, and each waiting step, with its `start` and `end` lines.
    // The copier used to write each pipe read straight through, so a read
    // that ended mid-line left the log mid-line, and the step's append then
    // landed inside a runner line:
    //
    //   amplihack.recipe.step_1 working (30s eamplihack.recipe.usage_limit_wait
    //   start reason=spend_limit account=work ...lapsed)
    //
    // which is unreadable by eye and invisible to a line-oriented grep, so
    // the wait stayed hidden in exactly the case it was added for. The
    // copier therefore holds an unterminated tail and writes only up to the
    // last newline of each read.

    /// A reader that hands out `chunks` one `read` at a time, running
    /// `between` before each one. The hook is how a test puts another
    /// writer's append between two of the copier's writes without any
    /// timing.
    struct Chunks<'a> {
        chunks: Vec<Vec<u8>>,
        between: Box<dyn FnMut(usize) + 'a>,
        taken: usize,
    }

    impl Chunks<'_> {
        fn new<const N: usize>(chunks: [&str; N]) -> Self {
            Self {
                chunks: chunks.iter().map(|c| c.as_bytes().to_vec()).collect(),
                between: Box::new(|_| {}),
                taken: 0,
            }
        }
    }

    impl Read for Chunks<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            (self.between)(self.taken);
            let Some(chunk) = self.chunks.get_mut(self.taken) else {
                return Ok(0);
            };
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            chunk.drain(..n);
            // A chunk larger than the copier's own buffer arrives over
            // several reads, as a pipe's would.
            if chunk.is_empty() {
                self.taken += 1;
            }
            Ok(n)
        }
    }

    /// A sink that keeps each `write_all` separately, so a test can ask
    /// what the log looked like between two of them and not only at the end.
    #[derive(Default)]
    struct Writes(Vec<Vec<u8>>);

    impl Writes {
        fn joined(&self) -> String {
            String::from_utf8(self.0.concat()).unwrap()
        }
    }

    impl Write for Writes {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.push(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_log_is_never_left_in_the_middle_of_a_line() {
        let mut reader = Chunks::new([
            "amplihack.recipe.step_1 working (30s e",
            "lapsed)\namplihack.recipe.step_1 wo",
            "rking (60s elapsed)\n",
        ]);
        let (mut out, mut log) = (Writes::default(), Writes::default());
        copy_output(&mut reader, &mut out, &mut log);
        for write in &log.0 {
            assert_eq!(
                write.last(),
                Some(&b'\n'),
                "a write that does not end a line leaves the log open for \
                 the next writer to land inside: {:?}",
                String::from_utf8_lossy(write)
            );
        }
        assert_eq!(
            log.joined(),
            "amplihack.recipe.step_1 working (30s elapsed)\n\
             amplihack.recipe.step_1 working (60s elapsed)\n",
            "every byte still reaches the log, and in order"
        );
    }

    #[test]
    fn a_waiting_steps_line_cannot_land_inside_a_runner_line() {
        // The step appends after the copier's first read, while the runner's
        // heartbeat is half written.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.log");
        fs::write(&path, "").unwrap();
        let wait_line = "amplihack.recipe.usage_limit_wait start reason=spend_limit \
                         wait=login_change account=work reset=none \
                         until=2026-10-18 14:00:00 -07:00 bound=max_wait";
        let appender = path.clone();
        let line = wait_line.to_string();
        let mut reader = Chunks::new([
            "amplihack.recipe.step_1 working (30s e",
            "lapsed)\n",
            "amplihack.recipe.step_1 working (60s elapsed)\n",
        ]);
        reader.between = Box::new(move |taken| {
            if taken == 1 {
                let mut f = OpenOptions::new().append(true).open(&appender).unwrap();
                f.write_all(format!("{line}\n").as_bytes()).unwrap();
            }
        });
        let mut log = OpenOptions::new().append(true).open(&path).unwrap();
        let mut out = Writes::default();
        copy_output(&mut reader, &mut out, &mut log);
        drop(log);

        let body = fs::read_to_string(&path).unwrap();
        assert!(
            body.lines().any(|l| l == wait_line),
            "the wait's line must stand whole on a line of its own: {body:?}"
        );
        for l in body.lines() {
            assert!(
                !(l.contains("working (") && l.contains("usage_limit_wait")),
                "a wait line landed inside a runner line: {l:?}"
            );
        }
    }

    #[test]
    fn stdout_stays_byte_for_byte() {
        // Interactive output must not be held back a line at a time; only
        // the write to the log moves.
        let chunks = ["a prompt with no newline yet", " and the rest\n", "partial"];
        let mut reader = Chunks::new(chunks);
        let (mut out, mut log) = (Writes::default(), Writes::default());
        copy_output(&mut reader, &mut out, &mut log);
        assert_eq!(out.joined(), chunks.concat());
        assert_eq!(
            out.0.len(),
            chunks.len(),
            "one write out per read in, so nothing waits for a newline"
        );
    }

    #[test]
    fn an_unterminated_tail_is_written_whole_when_the_pipe_closes() {
        // The run's last line may have no newline. Dropping it would lose
        // the runner's final message; writing it without one would leave the
        // log open for whatever writes next.
        let mut reader = Chunks::new(["done\nno newline at the end"]);
        let (mut out, mut log) = (Writes::default(), Writes::default());
        copy_output(&mut reader, &mut out, &mut log);
        assert_eq!(log.joined(), "done\nno newline at the end\n");
        assert_eq!(log.0.last().unwrap().last(), Some(&b'\n'));
    }

    #[test]
    fn a_pipe_that_closes_on_a_line_boundary_adds_nothing() {
        let mut reader = Chunks::new(["done\n"]);
        let (mut out, mut log) = (Writes::default(), Writes::default());
        copy_output(&mut reader, &mut out, &mut log);
        assert_eq!(log.joined(), "done\n", "no empty line at the end");
    }

    #[test]
    fn output_that_never_ends_a_line_is_flushed_rather_than_held_for_ever() {
        // The held tail comes from the pinned external runner, so its line
        // discipline is not this code's to assume. Past `MAX_HELD` the hold
        // is given up and the bytes are written: the log is worth more than
        // the line boundary, and dropping them is never the answer.
        // Two mebibytes, so the input is past `MAX_HELD` whatever rounding
        // the reads happen to fall on.
        let long = "x".repeat(2 << 20);
        let mut reader = Chunks::new([long.as_str(), "\n"]);
        let (mut out, mut log) = (Writes::default(), Writes::default());
        copy_output(&mut reader, &mut out, &mut log);
        assert_eq!(
            log.joined(),
            format!("{long}\n"),
            "held bytes are flushed, never discarded"
        );
    }

    #[test]
    fn an_empty_run_writes_nothing() {
        let mut reader = Chunks::new([]);
        let (mut out, mut log) = (Writes::default(), Writes::default());
        copy_output(&mut reader, &mut out, &mut log);
        assert!(log.0.is_empty() && out.0.is_empty());
    }
}
