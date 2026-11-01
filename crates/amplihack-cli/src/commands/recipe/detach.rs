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
        env: child_env(request.agent_binary),
        log: log.clone(),
        context_file: secrets.context.clone(),
    };
    create_private(&secrets.spec)?.write_all(&serde_json::to_vec(&spec)?)?;

    let started = Command::new("tmux")
        // Issue #1811: a tmux server started here keeps this environment
        // for every later session; the governor's parent marker must not
        // reach them.
        .env_remove(crate::commands::governor::GOVERNOR_PARENT_ENV)
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

/// The caller's environment for the run: every agent session marker removed,
/// and the resolved agent binary set with the tag `recipe run` would export.
/// The capacity governor's parent marker is removed too (issue #1811): a
/// detached run is independent work and asks for a slot of its own.
fn child_env(agent_binary: &Resolution) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| {
            !SESSION_MARKERS.iter().any(|(marker, _)| marker == k)
                && !TMUX_VARS.contains(&k.as_str())
                && k != crate::commands::governor::GOVERNOR_PARENT_ENV
        })
        .collect();
    env.insert(BINARY_ENV.to_string(), agent_binary.binary.clone());
    match export_tag(&agent_binary.binary, agent_binary.source) {
        Some(tag) => env.insert(SOURCE_ENV.to_string(), tag),
        None => env.remove(SOURCE_ENV),
    };
    env
}

/// `recipe.log_dir`, or `~/.amplihack/recipe-logs`.
fn log_dir() -> Result<PathBuf> {
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
        let mut buf = [0u8; 8192];
        let mut stdout = std::io::stdout();
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let _ = stdout.write_all(&buf[..n]);
            let _ = stdout.flush();
            let _ = log.write_all(&buf[..n]);
        }
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
}
