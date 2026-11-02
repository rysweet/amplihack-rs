//! Core launcher orchestrator for Claude CLI.
//!
//! Matches Python `amplihack/launcher/core.py`:
//! - Pre-launch preparation (prerequisites, repo checkout, directory detection)
//! - Claude command building with managed environment
//! - Runtime directory creation and settings patching
//! - Environment detection (non-interactive, sandboxed)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

/// Launcher configuration, created once at startup.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LauncherConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout_repo: Option<String>,
    #[serde(default)]
    pub claude_args: Vec<String>,
    pub verbose: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<PathBuf>,
}

const RUNTIME_DIRS: &[&str] = &["locks", "reflection", "logs", "metrics", "provenance"];
const PROJECT_DIR_PLACEHOLDER: &str = "$CLAUDE_PROJECT_DIR";

fn shared_runtime_root() -> PathBuf {
    if let Ok(root) = std::env::var("AMPLIHACK_RUNTIME_ROOT")
        && !root.trim().is_empty()
    {
        return PathBuf::from(root);
    }
    if let Ok(root) = std::env::var("XDG_RUNTIME_DIR")
        && !root.trim().is_empty()
    {
        return PathBuf::from(root).join("amplihack-runtime");
    }
    PathBuf::from("/tmp/amplihack-runtime")
}

/// Core launcher orchestrator.
pub struct ClaudeLauncher {
    config: LauncherConfig,
    path_cache: HashMap<PathBuf, PathBuf>,
}

impl ClaudeLauncher {
    pub fn new(config: LauncherConfig) -> Self {
        Self {
            config,
            path_cache: HashMap::new(),
        }
    }

    /// Full pre-launch preparation. Returns `true` if launch can proceed.
    pub fn prepare_launch(&mut self) -> Result<bool> {
        if is_noninteractive() && !self.check_prerequisites_noninteractive()? {
            return Ok(false);
        }
        if !self.handle_repo_checkout()? {
            return Ok(false);
        }
        let target_dir = self.find_target_directory();
        if let Some(ref dir) = target_dir {
            self.ensure_runtime_directories(dir)?;
            self.fix_hook_paths_in_settings(dir)?;
        }
        self.config.target_directory = target_dir;
        info!("Pre-launch preparation complete");
        Ok(true)
    }

    /// Build the full Claude CLI command.
    pub fn build_claude_command(&self) -> Result<Command> {
        let cli_path = get_claude_cli_path()?;
        let mut cmd = Command::new(&cli_path);
        if let Some(ref pf) = self.config.append_system_prompt {
            if pf.exists() {
                // `--append-system-prompt` takes a PROMPT STRING. This field is
                // a PathBuf, so it must go through the path-shaped flag —
                // passing it to the string form hands claude a filename and
                // calls it a prompt. The launch path in `amplihack-cli` is the
                // string-shaped sibling and correspondingly emits the contents.
                cmd.args(["--append-system-prompt-file", &pf.to_string_lossy()]);
            } else {
                warn!(path = %pf.display(), "System prompt file not found");
            }
        }
        if let Some(ref target) = self.config.target_directory
            && !paths_are_same(target, &std::env::current_dir().unwrap_or_default())
        {
            cmd.args(["--add-dir", &target.to_string_lossy()]);
        }
        for arg in &self.config.claude_args {
            cmd.arg(arg);
        }
        debug!(cli = %cli_path.display(), "Built Claude command");
        Ok(cmd)
    }

    pub fn launch(&mut self) -> Result<i32> {
        if !self.prepare_launch()? {
            return Ok(1);
        }
        let mut cmd = self.build_claude_command()?;
        info!("Launching Claude...");
        let status = cmd.status().context("failed to launch Claude process")?;
        let code = status.code().unwrap_or(1);
        info!(exit_code = code, "Claude process exited");
        Ok(code)
    }

    pub fn launch_interactive(&mut self) -> Result<i32> {
        if !self.prepare_launch()? {
            return Ok(1);
        }
        let mut cmd = self.build_claude_command()?;
        cmd.stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        info!("Launching Claude (interactive)...");
        Ok(cmd
            .status()
            .context("failed to launch")?
            .code()
            .unwrap_or(1))
    }

    pub fn invalidate_path_cache(&mut self) {
        self.path_cache.clear();
    }
    pub fn config(&self) -> &LauncherConfig {
        &self.config
    }

    fn check_prerequisites_noninteractive(&self) -> Result<bool> {
        if get_claude_cli_path().is_err() {
            warn!("Claude CLI not found in non-interactive mode");
            return Ok(false);
        }
        Ok(true)
    }

    fn handle_repo_checkout(&self) -> Result<bool> {
        let Some(ref repo) = self.config.checkout_repo else {
            return Ok(true);
        };
        // SEC: Validate repo doesn't start with `-` to prevent git option injection,
        // and use `--` terminator to separate options from the repository argument.
        if repo.starts_with('-') {
            warn!(repo = %repo, "Rejecting repository: must not start with '-'");
            return Ok(false);
        }
        info!(repo = %repo, "Checking out repository...");
        // Issue #1808: inside a recipe run, clone into the work root rather
        // than the current folder, and allow only network transports.
        if let Some(dest) = run_clone_destination(
            repo,
            std::env::var_os("AMPLIHACK_RUN_DIR").as_deref(),
            std::env::var_os("AMPLIHACK_WORK_ROOT").as_deref(),
        ) {
            if let Err(error) = checkout_run_clone(repo, &dest, RUN_CLONE_PROTOCOLS) {
                warn!(repo = %repo, error = %format!("{error:#}"), "Repository checkout failed");
                eprintln!("amplihack: repository checkout failed: {error:#}");
                return Ok(false);
            }
            // Not ./<name> as outside a run: say where it is.
            eprintln!("amplihack: {repo} is checked out at {}", dest.display());
            return Ok(true);
        }
        let mut command = amplihack_git::command();
        command.args(["clone", "--depth", "1", "--", repo]);
        let status = command.status().context("failed to run git clone")?;
        if !status.success() {
            warn!(repo = %repo, "Repository checkout failed");
            return Ok(false);
        }
        Ok(true)
    }

    fn find_target_directory(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.config.target_directory
            && dir.exists()
        {
            return Some(dir.clone());
        }
        let mut current = std::env::current_dir().ok()?;
        loop {
            if current.join(".claude").exists() {
                return Some(current);
            }
            if !current.pop() {
                break;
            }
        }
        // PR #3916 port: fall back to git repo root, then cwd — never empty.
        detect_repo_root().or_else(|| std::env::current_dir().ok())
    }

    fn ensure_runtime_directories(&self, target_dir: &Path) -> Result<()> {
        let _ = target_dir;
        let base = shared_runtime_root();
        for name in RUNTIME_DIRS {
            let path = base.join(name);
            if !path.exists() {
                std::fs::create_dir_all(&path)
                    .with_context(|| format!("create {}", path.display()))?;
                debug!(dir = %path.display(), "Created runtime directory");
            }
        }
        Ok(())
    }

    fn fix_hook_paths_in_settings(&self, target_dir: &Path) -> Result<()> {
        let sp = target_dir.join(".claude").join("settings.json");
        if !sp.exists() {
            return Ok(());
        }
        let content =
            std::fs::read_to_string(&sp).with_context(|| format!("read {}", sp.display()))?;
        if !content.contains(PROJECT_DIR_PLACEHOLDER) {
            return Ok(());
        }
        let updated = content.replace(PROJECT_DIR_PLACEHOLDER, &target_dir.to_string_lossy());
        std::fs::write(&sp, &updated).with_context(|| format!("write {}", sp.display()))?;
        info!("Replaced {} in settings.json", PROJECT_DIR_PLACEHOLDER);
        Ok(())
    }
}

pub fn is_noninteractive() -> bool {
    std::env::var("AMPLIHACK_NONINTERACTIVE").is_ok() || std::env::var("CI").is_ok() || !is_tty()
}

pub fn is_sandboxed() -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "DOCKER_CONTAINER",
        "KUBERNETES_SERVICE_HOST",
    ]
    .iter()
    .any(|v| std::env::var(v).is_ok())
        || std::env::var("HOME").is_err()
        || std::env::var("PATH").is_err()
}

/// Resolve the claude binary through the single repo-wide resolver.
///
/// Issue #1266: this used to be a third independent answer to "which claude?",
/// alongside `bootstrap.rs` and `claude_cli.rs`. Three resolvers is how they
/// drifted apart in the first place, and it also makes the single-resolver
/// contract test un-writable.
///
/// It also used to short-circuit on a `CLAUDE_CLI_PATH` that merely `exists()`
/// — which accepts a directory, a file with no executable bit, and a relative
/// path, and hands it straight to `Command::new`. `CLAUDE_CLI_PATH=claude`
/// became an `execvp` lookup against the child's `$PATH`; `./claude` became
/// the current directory. That was the one place in the repo where a candidate
/// reached exec without passing `cheap_reject`, sitting directly beneath a
/// comment claiming the opposite. The supported override is
/// `CLAUDE_BINARY_PATH`, which `candidate_paths` reads as a user-supplied
/// [`ExplicitOverride`] and the funnel gates: a broken one is a hard error
/// naming the path, rather than a silent exec of something else.
///
/// [`ExplicitOverride`]: amplihack_utils::launch_target::TargetSource::ExplicitOverride
fn get_claude_cli_path() -> Result<PathBuf> {
    // Issue #1276: the origin travels with the call. Nothing on this path
    // sets an override on amplihack's behalf, so an override in the
    // environment is the user's instruction and a broken one is a hard error.
    let resolution = amplihack_utils::launch_target::resolve(
        "claude",
        amplihack_utils::launch_target::OverrideOrigin::User,
    );
    match &resolution.target {
        Some(target) => Ok(target.path.clone()),
        None => Err(anyhow::anyhow!(
            "{}",
            resolution
                .rejection_report("claude", amplihack_utils::claude_native::CLAUDE_NPM_PACKAGE)
        )),
    }
}

fn paths_are_same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

/// Detect the repository root via `git rev-parse --show-toplevel`.
///
/// Falls back to the current working directory when git is unavailable or when
/// the working directory is not inside a git repository (PR #3916 parity).
pub fn detect_repo_root() -> Option<PathBuf> {
    match amplihack_git::command()
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if root.is_empty() {
                debug!("git rev-parse --show-toplevel returned empty; falling back to cwd");
                std::env::current_dir().ok()
            } else {
                Some(PathBuf::from(root))
            }
        }
        _ => {
            debug!("git rev-parse --show-toplevel failed; falling back to cwd");
            std::env::current_dir().ok()
        }
    }
}

#[cfg(unix)]
fn is_tty() -> bool {
    unsafe { libc::isatty(0) != 0 }
}
#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}

pub fn launch_claude(args: &[String]) -> Result<i32> {
    ClaudeLauncher::new(LauncherConfig {
        claude_args: args.to_vec(),
        ..Default::default()
    })
    .launch()
}

/// Issue #1808: where the launcher clones a repository inside a recipe run:
/// `<work root>/clones/<sanitised basename>-<first 8 hex of sha256(repo)>`.
/// `None` outside a run or when the work root is not an absolute path.
pub(crate) fn run_clone_destination(
    repo: &str,
    run_dir: Option<&std::ffi::OsStr>,
    work_root: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    run_dir.filter(|v| !v.is_empty())?;
    let work_root = PathBuf::from(work_root.filter(|v| !v.is_empty())?);
    if !work_root.is_absolute() {
        return None;
    }
    let trimmed = repo.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let base = trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let base = match base.as_str() {
        "" | "." | ".." => "repo".to_string(),
        _ => base,
    };
    let digest = Sha256::digest(repo.as_bytes());
    let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    Some(work_root.join("clones").join(format!("{base}-{hash}")))
}

/// Transports a clone inside a recipe run may use.
/// Plain `git://` is left out: it is neither encrypted nor authenticated.
const RUN_CLONE_PROTOCOLS: &str = "https:ssh";

/// git with no system config, no prompts, no fsmonitor and no hooks. The
/// repository's own `.git/config` can still name filters that `status` and
/// `reset` run, so a clone is used only after `check_private_dir` shows that
/// it and the folders above it belong to the current user.
fn hardened_git(dir: &Path) -> Command {
    let mut command = amplihack_git::command();
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .arg("-C")
        .arg(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn git_stdout(mut command: Command) -> Option<String> {
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Issue #1808: clones `repo` into the run clone folder `dest`, or reuses it.
///
/// The folder depends only on the URL and outlives the run, so a second
/// checkout of the same repository finds it. It is reused, and fetched, only
/// when it is a real folder at the top of a git checkout whose
/// `remote.origin.url` is `repo`; anything else is refused with a message
/// rather than replaced. A reused clone is moved to the fetched tip of its
/// branch, and only when it has no uncommitted changes and no local commits;
/// otherwise the run stops rather than work on an old commit.
///
/// The work root and `clones/` must belong to the current user and be
/// writable by nobody else, and an existing clone must belong to the current
/// user, so another user cannot plant a clone.
pub(crate) fn checkout_run_clone(repo: &str, dest: &Path, protocols: &str) -> Result<()> {
    let clones = dest.parent();
    for dir in [clones.and_then(Path::parent), clones]
        .into_iter()
        .flatten()
    {
        check_private_dir(dir, true)?;
    }
    match std::fs::symlink_metadata(dest) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                create_private_dir_all(parent)?;
            }
            info!(dest = %dest.display(), "Cloning into the run's work root");
            let status = amplihack_git::command()
                .args(["clone", "--depth", "1", "--", repo])
                .arg(dest)
                .env("GIT_ALLOW_PROTOCOL", protocols)
                .env("GIT_TERMINAL_PROMPT", "0")
                .status()
                .context("failed to run git clone")?;
            anyhow::ensure!(status.success(), "git clone of {repo} failed");
            exclude_worktrees(dest)
        }
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", dest.display())),
        Ok(meta) => {
            anyhow::ensure!(
                meta.file_type().is_dir(),
                "{} exists and is not a folder; remove it to check out {repo}",
                dest.display()
            );
            check_private_dir(dest, false)?;
            let real = std::fs::canonicalize(dest)
                .with_context(|| format!("cannot resolve {}", dest.display()))?;
            let mut top = hardened_git(dest);
            top.args(["rev-parse", "--show-toplevel"]);
            let is_checkout = git_stdout(top)
                .and_then(|t| std::fs::canonicalize(t).ok())
                .is_some_and(|t| t == real);
            anyhow::ensure!(
                is_checkout,
                "{} exists but is not a git checkout; remove it to check out {repo}",
                dest.display()
            );
            let mut origin = hardened_git(dest);
            origin.args(["config", "--get", "remote.origin.url"]);
            let url = git_stdout(origin).unwrap_or_default();
            anyhow::ensure!(
                url == repo,
                "{} is a checkout of {url:?}, not {repo}; remove it to check out {repo}",
                dest.display()
            );
            update_run_clone(dest, protocols)
        }
    }
}

/// Fetches into an existing run clone and moves its branch to the fetched
/// tip. Refuses, changing nothing, when the clone has uncommitted changes, a
/// detached HEAD, an upstream that is not a branch of origin, or commits its
/// upstream does not have.
fn update_run_clone(dest: &Path, protocols: &str) -> Result<()> {
    let shown = dest.display();
    // Clones made before this rule have no exclude line yet.
    exclude_worktrees(dest)?;
    let mut status = hardened_git(dest);
    status.args(["status", "--porcelain", "--untracked-files=normal"]);
    let changes = git_stdout(status)
        .with_context(|| format!("cannot read the state of the existing clone {shown}"))?;
    anyhow::ensure!(
        changes.is_empty(),
        "the existing clone {shown} has uncommitted changes; commit or remove them, or remove the clone"
    );
    let mut upstream = hardened_git(dest);
    upstream.args([
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    ]);
    let upstream = git_stdout(upstream).filter(|u| !u.is_empty()).with_context(|| {
        format!("the existing clone {shown} is not on a branch with an upstream; remove it to check out again")
    })?;
    let rev = |spec: &str| {
        let mut command = hardened_git(dest);
        command.args(["rev-parse", "--verify", "--quiet", spec]);
        git_stdout(command)
    };
    anyhow::ensure!(
        rev("HEAD").is_some() && rev("HEAD") == rev("@{upstream}"),
        "the existing clone {shown} has commits that {upstream} does not; push or remove them, or remove the clone"
    );
    // `git fetch origin` updates only refs under `refs/remotes/origin/`, so an
    // upstream elsewhere would be reset to stale code (PR #1843 review).
    let mut tracking = hardened_git(dest);
    tracking.args(["rev-parse", "--symbolic-full-name", "@{upstream}"]);
    anyhow::ensure!(
        git_stdout(tracking).is_some_and(|t| t.starts_with("refs/remotes/origin/")),
        "the existing clone {shown} tracks {upstream}, which is not a branch of origin; switch it back or remove the clone"
    );
    info!(dest = %shown, "Reusing the run clone; fetching");
    let mut fetch = hardened_git(dest);
    fetch
        .args(["fetch", "--depth", "1", "origin"])
        .env("GIT_ALLOW_PROTOCOL", protocols);
    let fetched = fetch.status().context("failed to run git fetch")?;
    anyhow::ensure!(
        fetched.success(),
        "git fetch in the existing clone {shown} failed"
    );
    // Clean and with no local commits (checked above), so nothing is lost.
    let mut reset = hardened_git(dest);
    reset.args(["reset", "--quiet", "--hard", "@{upstream}"]);
    let moved = reset.status().context("failed to run git reset")?;
    anyhow::ensure!(
        moved.success(),
        "cannot move the existing clone {shown} to the fetched {upstream}"
    );
    Ok(())
}

/// Adds `/worktrees/` to the clone's `info/exclude`. Without a work root set
/// explicitly, runs put their worktrees under `<clone>/worktrees/`, which
/// would otherwise show as untracked and make the clone look changed.
fn exclude_worktrees(dest: &Path) -> Result<()> {
    const LINE: &str = "/worktrees/";
    let mut path = hardened_git(dest);
    path.args(["rev-parse", "--git-path", "info/exclude"]);
    let path = git_stdout(path)
        .map(PathBuf::from)
        .with_context(|| format!("cannot find the exclude file of {}", dest.display()))?;
    let path = if path.is_absolute() {
        path
    } else {
        dest.join(path)
    };
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    if current.lines().any(|l| l.trim() == LINE) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(LINE);
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// Refuses an existing `dir` that is a symbolic link, is not a folder, or is
/// owned by another user; with `private`, also one writable by group or
/// others.
#[cfg(unix)]
fn check_private_dir(dir: &Path, private: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", dir.display()));
        }
    };
    anyhow::ensure!(
        meta.file_type().is_dir() && !meta.file_type().is_symlink(),
        "{} is not a real folder; refusing to clone there",
        dir.display()
    );
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    anyhow::ensure!(
        meta.uid() == euid,
        "{} belongs to another user; refusing to clone there",
        dir.display()
    );
    anyhow::ensure!(
        !private || meta.mode() & 0o022 == 0,
        "{} is writable by other users; refusing to clone there (chmod go-w it)",
        dir.display()
    );
    Ok(())
}

#[cfg(not(unix))]
fn check_private_dir(_dir: &Path, _private: bool) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_private_dir_all(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("create {}", dir.display()))
}

#[cfg(not(unix))]
fn create_private_dir_all(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))
}

#[cfg(test)]
mod issue_1808_clone_destination {
    use super::{checkout_run_clone, run_clone_destination};
    use std::ffi::OsStr;
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) {
        let status = amplihack_git::command_in(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn origin(root: &Path, name: &str) -> String {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("f"), name).unwrap();
        git(&dir, &["add", "f"]);
        git(&dir, &["commit", "-q", "-m", "c"]);
        format!("file://{}", dir.display())
    }

    /// P1-4: a second checkout of the same URL reuses the clone.
    #[test]
    fn second_checkout_of_the_same_url_reuses_the_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let dest = tmp.path().join("work/clones/a-12345678");
        checkout_run_clone(&url, &dest, "file").unwrap();
        assert!(dest.join("f").is_file());
        checkout_run_clone(&url, &dest, "file").unwrap();
        assert!(dest.join("f").is_file());
    }

    /// A reused clone is moved to the origin's new tip, not left on the old one.
    #[test]
    fn reused_clone_is_updated_to_the_fetched_tip() {
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let dest = tmp.path().join("work/clones/a-12345678");
        checkout_run_clone(&url, &dest, "file").unwrap();
        let upstream = tmp.path().join("a");
        std::fs::write(upstream.join("f"), "newer").unwrap();
        git(&upstream, &["commit", "-q", "-am", "newer"]);
        checkout_run_clone(&url, &dest, "file").unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("f")).unwrap(), "newer");
    }

    /// A reused clone left on another branch of origin is moved to that
    /// branch's new tip: tracking it requires a refspec, which the fetch uses.
    #[test]
    fn reused_clone_on_another_branch_is_updated_to_its_tip() {
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let upstream = tmp.path().join("a");
        git(&upstream, &["branch", "feat"]);
        let dest = tmp.path().join("work/clones/a-12345678");
        checkout_run_clone(&url, &dest, "file").unwrap();
        // A single-branch clone can track `feat` only once its refspec covers it.
        git(
            &dest,
            &["remote", "set-branches", "--add", "origin", "feat"],
        );
        git(&dest, &["fetch", "-q", "origin"]);
        git(
            &dest,
            &["checkout", "-q", "-b", "feat", "--track", "origin/feat"],
        );
        git(&upstream, &["checkout", "-q", "feat"]);
        std::fs::write(upstream.join("f"), "feat newer").unwrap();
        git(&upstream, &["commit", "-q", "-am", "feat newer"]);
        checkout_run_clone(&url, &dest, "file").unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("f")).unwrap(),
            "feat newer"
        );
    }

    /// A reused clone whose branch tracks another remote is refused, since the
    /// fetch from origin would not update it.
    #[test]
    fn reused_clone_tracking_another_remote_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let other = origin(tmp.path(), "b");
        let dest = tmp.path().join("work/clones/a-12345678");
        checkout_run_clone(&url, &dest, "file").unwrap();
        git(&dest, &["remote", "add", "other", &other]);
        git(&dest, &["fetch", "-q", "other"]);
        git(
            &dest,
            &["checkout", "-q", "-b", "o", "--track", "other/main"],
        );
        let error = checkout_run_clone(&url, &dest, "file").unwrap_err();
        assert!(
            format!("{error:#}").contains("not a branch of origin"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(dest.join("f")).unwrap(), "b");
    }

    /// Worktrees a run made under `<clone>/worktrees/` do not make the clone
    /// look changed, so the next checkout still reuses it.
    #[test]
    fn worktrees_under_the_clone_do_not_block_reuse() {
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let dest = tmp.path().join("work/clones/a-12345678");
        checkout_run_clone(&url, &dest, "file").unwrap();
        std::fs::create_dir_all(dest.join("worktrees/feat")).unwrap();
        std::fs::write(dest.join("worktrees/feat/x"), "x").unwrap();
        checkout_run_clone(&url, &dest, "file").unwrap();
        assert!(dest.join("worktrees/feat/x").is_file());
    }

    /// A reused clone with uncommitted changes or local commits is refused
    /// and left as it was.
    #[test]
    fn reused_clone_with_local_work_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let dest = tmp.path().join("work/clones/a-12345678");
        checkout_run_clone(&url, &dest, "file").unwrap();
        std::fs::write(dest.join("f"), "mine").unwrap();
        let error = checkout_run_clone(&url, &dest, "file").unwrap_err();
        assert!(
            format!("{error:#}").contains("uncommitted changes"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(dest.join("f")).unwrap(), "mine");

        git(&dest, &["commit", "-q", "-am", "local"]);
        let error = checkout_run_clone(&url, &dest, "file").unwrap_err();
        assert!(format!("{error:#}").contains("does not"), "{error:#}");
        assert_eq!(std::fs::read_to_string(dest.join("f")).unwrap(), "mine");
    }

    /// Security M2: a clones folder other users can write to is refused.
    #[cfg(unix)]
    #[test]
    fn shared_clones_folder_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let url = origin(tmp.path(), "a");
        let clones = tmp.path().join("work/clones");
        std::fs::create_dir_all(&clones).unwrap();
        std::fs::set_permissions(&clones, std::fs::Permissions::from_mode(0o777)).unwrap();
        let error = checkout_run_clone(&url, &clones.join("a-12345678"), "file").unwrap_err();
        assert!(
            format!("{error:#}").contains("writable by other users"),
            "{error:#}"
        );
        assert!(!clones.join("a-12345678").exists());
    }

    /// P1-4: a folder holding another remote, or no checkout, is refused.
    #[test]
    fn mismatched_remote_or_non_checkout_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let a = origin(tmp.path(), "a");
        let b = origin(tmp.path(), "b");
        let dest = tmp.path().join("work/clones/x-12345678");
        checkout_run_clone(&b, &dest, "file").unwrap();
        let error = checkout_run_clone(&a, &dest, "file").unwrap_err();
        assert!(
            format!("{error:#}").contains("is a checkout of"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(dest.join("f")).unwrap(), "b");

        let plain = tmp.path().join("work/clones/plain");
        std::fs::create_dir_all(&plain).unwrap();
        let error = checkout_run_clone(&a, &plain, "file").unwrap_err();
        assert!(
            format!("{error:#}").contains("not a git checkout"),
            "{error:#}"
        );
    }

    #[test]
    fn outside_a_run_or_with_a_relative_root_there_is_no_destination() {
        let url = "https://github.com/rysweet/amplihack-rs.git";
        assert_eq!(
            run_clone_destination(url, None, Some(OsStr::new("/w"))),
            None
        );
        assert_eq!(
            run_clone_destination(url, Some(OsStr::new("/w/runs/x")), Some(OsStr::new("rel"))),
            None
        );
    }

    #[test]
    fn inside_a_run_clones_go_under_work_root_clones_with_a_hash() {
        let url = "https://github.com/rysweet/amplihack-rs.git";
        let dest =
            run_clone_destination(url, Some(OsStr::new("/w/runs/x")), Some(OsStr::new("/w")))
                .unwrap();
        assert_eq!(dest.parent(), Some(Path::new("/w/clones")));
        let name = dest.file_name().unwrap().to_str().unwrap();
        let hash = name.strip_prefix("amplihack-rs-").unwrap();
        assert_eq!(hash.len(), 8);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        let odd = run_clone_destination(
            "git@host:org/my repo;x",
            Some(OsStr::new("/w/runs/x")),
            Some(OsStr::new("/w")),
        )
        .unwrap();
        assert!(
            odd.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("my_repo_x-")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let c = LauncherConfig::default();
        assert!(!c.verbose);
        assert!(c.claude_args.is_empty());
    }
    #[test]
    fn config_serializes() {
        let c = LauncherConfig {
            verbose: true,
            claude_args: vec!["--model".into(), "opus".into()],
            ..Default::default()
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["verbose"], true);
        assert_eq!(j["claude_args"][0], "--model");
    }
    #[test]
    fn launcher_stores_config() {
        assert!(
            ClaudeLauncher::new(LauncherConfig {
                verbose: true,
                ..Default::default()
            })
            .config()
            .verbose
        );
    }
    #[test]
    fn is_sandboxed_basic() {
        let in_ci = std::env::var("CI").is_ok();
        assert_eq!(is_sandboxed(), in_ci || std::env::var("HOME").is_err());
    }
    #[test]
    fn paths_same_identical() {
        assert!(paths_are_same(Path::new("/usr"), Path::new("/usr")));
    }
    #[test]
    fn paths_same_different() {
        assert!(!paths_are_same(Path::new("/usr"), Path::new("/var")));
    }
    #[test]
    fn ensure_runtime_dirs() {
        let dir = tempfile::tempdir().unwrap();
        ClaudeLauncher::new(LauncherConfig::default())
            .ensure_runtime_directories(dir.path())
            .unwrap();
        let runtime_root = shared_runtime_root();
        for n in RUNTIME_DIRS {
            assert!(runtime_root.join(n).exists(), "Missing: {n}");
        }
    }
    #[test]
    fn fix_hook_paths_replaces() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let sp = dir.path().join(".claude/settings.json");
        std::fs::write(
            &sp,
            r#"{"hooks": {"path": "$CLAUDE_PROJECT_DIR/hooks/pre.sh"}}"#,
        )
        .unwrap();
        ClaudeLauncher::new(LauncherConfig::default())
            .fix_hook_paths_in_settings(dir.path())
            .unwrap();
        let c = std::fs::read_to_string(&sp).unwrap();
        assert!(!c.contains("$CLAUDE_PROJECT_DIR"));
        assert!(c.contains(&dir.path().to_string_lossy().to_string()));
    }
    #[test]
    fn fix_hook_paths_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let sp = dir.path().join(".claude/settings.json");
        let orig = r#"{"hooks": {"path": "/absolute/hooks/pre.sh"}}"#;
        std::fs::write(&sp, orig).unwrap();
        ClaudeLauncher::new(LauncherConfig::default())
            .fix_hook_paths_in_settings(dir.path())
            .unwrap();
        assert_eq!(std::fs::read_to_string(&sp).unwrap(), orig);
    }
    #[test]
    fn find_target_from_claude_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        let l = ClaudeLauncher::new(LauncherConfig {
            target_directory: Some(dir.path().into()),
            ..Default::default()
        });
        assert_eq!(l.find_target_directory().unwrap(), dir.path());
    }
    #[test]
    fn detect_repo_root_never_empty() {
        // Even outside a git repo, detect_repo_root falls back to cwd.
        let result = detect_repo_root();
        assert!(result.is_some(), "detect_repo_root must never return None");
    }

    // ------------------------------------------------------------------
    // Issue #1265: the path-shaped sibling of the --append-system-prompt
    // feature must emit the flag that actually takes a path.
    // ------------------------------------------------------------------

    #[test]
    fn append_system_prompt_path_uses_the_file_flag() {
        // `--append-system-prompt` takes a PROMPT STRING. Passing a PathBuf to
        // it hands claude a filename and calls it a prompt. The path form is
        // `--append-system-prompt-file`.
        let src = include_str!("launcher_core.rs");
        let start = src
            .find("fn build_claude_command(")
            .expect("build_claude_command must exist");
        let body = &src[start..];
        let mut depth = 0;
        let mut end = body.len();
        for (i, ch) in body.char_indices() {
            if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
        }
        let body = &body[..end];
        assert!(
            body.contains("--append-system-prompt-file"),
            "a PathBuf must be emitted through the -file form:\n{body}"
        );
    }

    #[test]
    fn missing_system_prompt_file_warns_and_still_builds_a_command() {
        // Graceful degradation, same contract as the launch-path feature: a
        // missing fragment never fails the launch.
        let config = LauncherConfig {
            append_system_prompt: Some(PathBuf::from("/nonexistent/prompt.md")),
            ..Default::default()
        };
        let launcher = ClaudeLauncher::new(config);
        if let Ok(cmd) = launcher.build_claude_command() {
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert!(
                !args.iter().any(|a| a.starts_with("--append-system-prompt")),
                "a missing file must not be passed through: {args:?}"
            );
        }
        // An Err here means no claude binary on this host, which is not what
        // this test is about.
    }
}
