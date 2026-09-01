//! Framework source resolution: bundled-first, with network fallback.
//!
//! As of issue #254 the framework assets are bundled inside the amplihack-rs
//! source tree (`amplifier-bundle/`) and no longer fetched from the upstream
//! `rysweet/amplihack` repository at install time.
//!
//! Resolution order (fix #341 — prefer the user's actual checkout over any
//! baked-at-build-time path):
//! 1. **`AMPLIHACK_HOME`** — explicit user-configured override (highest
//!    priority).
//! 2. **Current-directory walk-up** — walks parent directories of the current
//!    working directory looking for `amplifier-bundle/`. This is the path that
//!    correctly identifies "the checkout the user is installing from" when
//!    they invoke `amplihack install` from inside a clone, even if the
//!    binary was built elsewhere.
//! 3. **Walk-up from executable** — walks parent directories of
//!    `current_exe()` looking for `amplifier-bundle/` (in-tree dev binary
//!    under `target/`).
//! 4. **Compile-time workspace root** — the `CARGO_MANIFEST_DIR` embedded at
//!    build time points two levels up to the workspace root that contains
//!    `amplifier-bundle/`. Only meaningful for `cargo run`-style invocations
//!    from the workspace itself; demoted because for an installed binary it
//!    pins the bundle to whatever was on disk at compile time (issue #341).
//! 5. **`~/.amplihack`** — staged install location from a prior run.
//! 6. **Network download** (legacy fallback) — `git clone` / tarball from
//!    upstream, only attempted when none of the above yields a usable root.
//!
//! # How far a walk-up may walk (issue #1275)
//!
//! Steps 2 and 3 used to walk to the filesystem root, accepting the first
//! ancestor that carried a structurally plausible `amplifier-bundle/`. That is
//! a surprising rule: it means the install source depends on what happens to
//! sit *above* the directory you are standing in. Keep a checkout at
//! `~/src/amplihack-rs`, unpack a second copy at `~/src/`, and every install
//! run from anywhere under `~/src` silently stages from the copy — the user
//! picked a working directory, not an install source.
//!
//! Both walk-ups are now bounded by [`walk_up_scope`]: the walk stops at the
//! repository that contains the starting directory, and when no repository
//! contains it only the starting directory itself is considered. A walk-up
//! candidate must additionally look like an amplihack source tree
//! ([`looks_like_amplihack_source`]) rather than merely carry a directory of
//! the right shape. When a bundle-carrying ancestor *is* refused, resolution
//! says so and names `amplihack install --local <path>`, the deliberate route
//! to the same outcome.
//!
//! This is a correctness and least-surprise rule, not a security boundary.
//! Anything running as this user can already rewrite both the staged assets
//! and the directories these steps read; the point is that the installer should
//! stage from a source the user actually pointed it at, and should say which
//! one it picked when the answer is not obvious.

use super::bundle_compat::validate_framework_bundle_compatibility;
use super::types::{REPO_ARCHIVE_URL, REPO_GIT_URL};
use crate::update::{extract_archive, http_get_with_retry, validate_download_url};
#[cfg(windows)]
use crate::util::run_with_timeout;
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(not(test))]
const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(300);
#[cfg(test)]
const GIT_CLONE_TIMEOUT: Duration = Duration::from_millis(250);
const GIT_CLONE_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(windows)]
const TERMINATE_PROCESS_TREE_TIMEOUT: Duration = Duration::from_secs(5);
const CAPTURE_LIMIT: usize = 8192;

/// Workspace-member path that identifies an amplihack-rs `Cargo.toml`.
const AMPLIHACK_WORKSPACE_MARKER: &str = "crates/amplihack-cli";

/// Bytes of a candidate `Cargo.toml` read when sniffing for the marker above.
const CARGO_TOML_SNIFF_LIMIT: u64 = 64 * 1024;

/// Which resolution step produced a framework root.
///
/// Carried out of resolution so callers can *say* where they are staging from.
/// Issue #1275's complaint is not only that the old walk-up could pick a
/// surprising directory, but that it picked one silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameworkRootOrigin {
    /// The `AMPLIHACK_HOME` override.
    AmplihackHome,
    /// The repository containing the current working directory.
    CurrentDirectory,
    /// The checkout containing the running binary (in-tree dev build).
    ExecutableParent,
    /// The workspace this binary was compiled from.
    CompileTimeWorkspaceRoot,
    /// A previously staged install under `~/.amplihack`.
    PriorStagedInstall,
}

impl FrameworkRootOrigin {
    /// Human-readable "why this directory" for install output.
    pub(super) fn describe(self) -> &'static str {
        match self {
            Self::AmplihackHome => "AMPLIHACK_HOME",
            Self::CurrentDirectory => "the amplihack checkout containing the current directory",
            Self::ExecutableParent => "the checkout containing the running amplihack binary",
            Self::CompileTimeWorkspaceRoot => "the workspace this binary was built from",
            Self::PriorStagedInstall => "a previously staged install under ~/.amplihack",
        }
    }

    /// Short label used in diagnostics about a rejected candidate.
    fn label(self) -> &'static str {
        match self {
            Self::AmplihackHome => "AMPLIHACK_HOME",
            Self::CurrentDirectory => "current directory",
            Self::ExecutableParent => "executable parent",
            Self::CompileTimeWorkspaceRoot => "compile-time workspace root",
            Self::PriorStagedInstall => "~/.amplihack",
        }
    }
}

/// A framework root and the step that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedFrameworkRoot {
    pub(super) root: PathBuf,
    pub(super) origin: FrameworkRootOrigin,
}

/// Something resolution declined to use, and why.
///
/// Returned as data rather than printed from deep inside the walk so the
/// resolution logic stays pure and the messages are assertable in tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResolutionNote {
    /// A bundle of the right shape whose compatibility check failed.
    IncompatibleBundle {
        path: PathBuf,
        label: &'static str,
        error: String,
    },
    /// A bundle-carrying directory the walk-up was not allowed to reach,
    /// because it sits outside the repository containing the start directory.
    OutsideStartingRepository { path: PathBuf, start: PathBuf },
    /// A bundle-carrying directory that does not look like an amplihack
    /// source tree.
    NotAmplihackSource { path: PathBuf },
}

impl std::fmt::Display for ResolutionNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompatibleBundle { path, label, error } => write!(
                f,
                "⚠️  Skipping incompatible framework bundle from {label}: {}: {error}",
                path.display()
            ),
            Self::OutsideStartingRepository { path, start } => write!(
                f,
                "ℹ️  Ignoring the framework bundle at {}: it is outside the repository \
                 containing {}, so amplihack will not stage from it implicitly. \
                 Run `amplihack install --local {}` to stage from it deliberately.",
                path.display(),
                start.display(),
                path.display()
            ),
            Self::NotAmplihackSource { path } => write!(
                f,
                "ℹ️  Ignoring the framework bundle at {}: that directory does not look like \
                 an amplihack source checkout (no `.claude/`, no amplihack cargo workspace). \
                 Run `amplihack install --local {}` to stage from it deliberately.",
                path.display(),
                path.display()
            ),
        }
    }
}

/// Everything resolution decided, including the candidates it turned down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct FrameworkRootResolution {
    pub(super) resolved: Option<ResolvedFrameworkRoot>,
    pub(super) notes: Vec<ResolutionNote>,
}

/// The environment resolution reads, gathered up front.
///
/// Passing these in rather than reading `std::env` inside the walk is what
/// makes [`resolve_framework_root`] a pure function of its inputs — and what
/// lets its tests build hostile directory layouts without touching the real
/// `$HOME`, `$AMPLIHACK_HOME` or the process working directory.
#[derive(Debug, Clone, Default)]
pub(super) struct FrameworkRootQuery {
    pub(super) amplihack_home: Option<PathBuf>,
    pub(super) current_dir: Option<PathBuf>,
    pub(super) current_exe: Option<PathBuf>,
    pub(super) compile_time_workspace_root: Option<PathBuf>,
    pub(super) home: Option<PathBuf>,
}

/// Does `dir` hold a repository marker?
///
/// `.git` is a directory in an ordinary clone and a *file* in a linked
/// worktree or submodule, so both count.
fn is_repository_root(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// The directories a walk-up starting at `start` is allowed to consider,
/// nearest first.
///
/// Bounded at the repository containing `start`: the first ancestor holding a
/// repository marker is the last directory considered. When no repository
/// contains `start` — an unpacked tarball, a scratch directory — only `start`
/// itself is considered, because there is no enclosing unit of "the thing the
/// user is working on" to justify climbing further (issue #1275).
fn walk_up_scope(start: &Path) -> Vec<PathBuf> {
    let mut scope = Vec::new();
    for dir in start.ancestors() {
        scope.push(dir.to_path_buf());
        if is_repository_root(dir) {
            return scope;
        }
    }
    scope.truncate(1);
    scope
}

/// Does `dir` look like an amplihack framework source tree?
///
/// Carrying an `amplifier-bundle/` is a shape check that any directory can
/// satisfy; this asks the narrower question the walk-up actually cares about.
/// A complete framework checkout ships `.claude/`, and the amplihack-rs cargo
/// workspace root names `crates/amplihack-cli` in its `Cargo.toml`.
fn looks_like_amplihack_source(dir: &Path) -> bool {
    if dir.join(".claude").is_dir() {
        return true;
    }
    let Ok(file) = fs::File::open(dir.join("Cargo.toml")) else {
        return false;
    };
    let mut head = Vec::new();
    if file
        .take(CARGO_TOML_SNIFF_LIMIT)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    String::from_utf8_lossy(&head).contains(AMPLIHACK_WORKSPACE_MARKER)
}

/// Accept `candidate` if it carries a compatible `amplifier-bundle/`.
///
/// Records why a shape-valid bundle was turned down instead of failing mute.
fn compatible_candidate(
    candidate: &Path,
    origin: FrameworkRootOrigin,
    validate: &dyn Fn(&Path) -> Result<(), String>,
    notes: &mut Vec<ResolutionNote>,
) -> Option<PathBuf> {
    if !candidate.join("amplifier-bundle").is_dir() {
        return None;
    }
    match validate(candidate) {
        Ok(()) => Some(candidate.to_path_buf()),
        Err(error) => {
            notes.push(ResolutionNote::IncompatibleBundle {
                path: candidate.to_path_buf(),
                label: origin.label(),
                error,
            });
            None
        }
    }
}

/// Bounded walk-up from `start`, accepting only amplihack source trees.
///
/// `report_out_of_scope` asks for a note naming the nearest bundle-carrying
/// ancestor the bound kept us from reaching — the pre-#1275 answer — so a user
/// who expected that directory to win learns why it did not, and how to ask
/// for it on purpose.
fn resolve_from_walk_up(
    start: &Path,
    origin: FrameworkRootOrigin,
    report_out_of_scope: bool,
    validate: &dyn Fn(&Path) -> Result<(), String>,
    notes: &mut Vec<ResolutionNote>,
) -> Option<PathBuf> {
    let scope = walk_up_scope(start);
    for dir in &scope {
        if !dir.join("amplifier-bundle").is_dir() {
            continue;
        }
        if !looks_like_amplihack_source(dir) {
            notes.push(ResolutionNote::NotAmplihackSource { path: dir.clone() });
            continue;
        }
        if let Some(root) = compatible_candidate(dir, origin, validate, notes) {
            return Some(root);
        }
    }

    if report_out_of_scope
        && let Some(boundary) = scope.last()
        && let Some(out_of_scope) = boundary
            .ancestors()
            .skip(1)
            .find(|dir| dir.join("amplifier-bundle").is_dir())
    {
        notes.push(ResolutionNote::OutsideStartingRepository {
            path: out_of_scope.to_path_buf(),
            start: start.to_path_buf(),
        });
    }

    None
}

/// Resolve the framework root from an explicit description of the environment.
///
/// Pure with respect to the process: every input is an argument, including the
/// bundle compatibility check, so the ordering and the bound are testable
/// against fabricated directory layouts.
pub(super) fn resolve_framework_root(
    query: &FrameworkRootQuery,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> FrameworkRootResolution {
    let mut notes = Vec::new();

    // 1. AMPLIHACK_HOME env var — explicit user override. No source-shape
    //    requirement: the user named this directory on purpose.
    if let Some(home) = &query.amplihack_home
        && let Some(root) = compatible_candidate(
            home,
            FrameworkRootOrigin::AmplihackHome,
            validate,
            &mut notes,
        )
    {
        return FrameworkRootResolution {
            resolved: Some(ResolvedFrameworkRoot {
                root,
                origin: FrameworkRootOrigin::AmplihackHome,
            }),
            notes,
        };
    }

    // 2. Bounded walk-up from the current directory — the checkout the user is
    //    installing from (fix #341), and no further (issue #1275).
    if let Some(cwd) = &query.current_dir
        && let Some(root) = resolve_from_walk_up(
            cwd,
            FrameworkRootOrigin::CurrentDirectory,
            true,
            validate,
            &mut notes,
        )
    {
        return FrameworkRootResolution {
            resolved: Some(ResolvedFrameworkRoot {
                root,
                origin: FrameworkRootOrigin::CurrentDirectory,
            }),
            notes,
        };
    }

    // 3. Bounded walk-up from the executable (in-tree dev binary under
    //    `target/`). An installed binary has no enclosing checkout, so this
    //    step now stops instead of climbing towards `$HOME`.
    if let Some(exe_dir) = query.current_exe.as_deref().and_then(Path::parent)
        && let Some(root) = resolve_from_walk_up(
            exe_dir,
            FrameworkRootOrigin::ExecutableParent,
            false,
            validate,
            &mut notes,
        )
    {
        return FrameworkRootResolution {
            resolved: Some(ResolvedFrameworkRoot {
                root,
                origin: FrameworkRootOrigin::ExecutableParent,
            }),
            notes,
        };
    }

    // 4. Compile-time workspace root (only meaningful for `cargo run` from
    //    the workspace; demoted because for installed binaries it pins the
    //    bundle to whatever was on disk at build time — issue #341).
    if let Some(workspace_root) = &query.compile_time_workspace_root
        && let Some(root) = compatible_candidate(
            workspace_root,
            FrameworkRootOrigin::CompileTimeWorkspaceRoot,
            validate,
            &mut notes,
        )
    {
        return FrameworkRootResolution {
            resolved: Some(ResolvedFrameworkRoot {
                root,
                origin: FrameworkRootOrigin::CompileTimeWorkspaceRoot,
            }),
            notes,
        };
    }

    // 5. ~/.amplihack (from prior staged install)
    if let Some(home) = &query.home {
        let dot = home.join(".amplihack");
        if dot.join(".claude").is_dir()
            && let Some(root) = compatible_candidate(
                &dot,
                FrameworkRootOrigin::PriorStagedInstall,
                validate,
                &mut notes,
            )
        {
            return FrameworkRootResolution {
                resolved: Some(ResolvedFrameworkRoot {
                    root,
                    origin: FrameworkRootOrigin::PriorStagedInstall,
                }),
                notes,
            };
        }
    }

    FrameworkRootResolution {
        resolved: None,
        notes,
    }
}

/// Locate the bundled framework source from the amplihack-rs source tree.
///
/// Returns the repo root (the directory that contains `amplifier-bundle/`
/// and — for a complete source checkout — `.claude/`) together with the step
/// that found it, without any network access.  Returns `None` when the source
/// tree is not reachable (e.g. the binary was installed via `cargo install`
/// and the original checkout was deleted).
///
/// Candidates that were turned down are printed, not swallowed: see
/// [`ResolutionNote`].
pub(super) fn find_bundled_framework_root() -> Option<ResolvedFrameworkRoot> {
    let query = FrameworkRootQuery {
        amplihack_home: std::env::var_os("AMPLIHACK_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        current_dir: std::env::current_dir().ok(),
        current_exe: std::env::current_exe().ok(),
        compile_time_workspace_root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf),
        home: std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
    };

    let resolution = resolve_framework_root(&query, &|candidate| {
        validate_framework_bundle_compatibility(candidate).map_err(|err| format!("{err:#}"))
    });
    for note in &resolution.notes {
        eprintln!("{note}");
    }
    resolution.resolved
}

/// Fetch the framework repository into `destination`.
///
/// **Deprecated path** — only reached when `find_bundled_framework_root()`
/// returns `None` (source tree unavailable).
///
/// Strategy (matches Python `amplihack install` behaviour):
/// 1. If `git` is found on PATH, run `git clone --depth 1 <url> <dest>`.
/// 2. If `git` is NOT on PATH, fall back to HTTP tarball download.
pub(super) fn download_and_extract_framework_repo(destination: &Path) -> Result<PathBuf> {
    if let Ok(git_path) = which_git() {
        git_clone_framework_repo(&git_path, destination)?;
        return find_compatible_framework_repo_root(destination, REPO_GIT_URL);
    }

    // git not available — fall back to HTTP tarball download
    validate_download_url(REPO_ARCHIVE_URL)?;
    let archive_bytes = http_get_with_retry(REPO_ARCHIVE_URL)
        .with_context(|| format!("failed to download framework archive from {REPO_ARCHIVE_URL}"))?;
    extract_archive(&archive_bytes, destination).with_context(|| {
        format!(
            "failed to extract framework archive into {}",
            destination.display()
        )
    })?;
    find_compatible_framework_repo_root(destination, REPO_ARCHIVE_URL)
}

/// Resolve the `git` binary path from PATH.
fn which_git() -> Result<PathBuf> {
    let Some(paths) = std::env::var_os("PATH") else {
        bail!("git not found on PATH");
    };
    let candidates: &[&str] = if cfg!(windows) {
        &["git.exe", "git"]
    } else {
        &["git"]
    };
    for dir in std::env::split_paths(&paths) {
        for candidate in candidates {
            let path = dir.join(candidate);
            if is_executable_file(&path) {
                return Ok(path);
            }
        }
    }
    bail!("git not found on PATH")
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Run `git clone --depth 1 <REPO_GIT_URL> <destination>`.
fn git_clone_framework_repo(git_path: &Path, destination: &Path) -> Result<()> {
    let stdout_file = tempfile::NamedTempFile::new()
        .context("failed to create temporary stdout file for git clone")?;
    let stderr_file = tempfile::NamedTempFile::new()
        .context("failed to create temporary stderr file for git clone")?;
    let mut command = Command::new(git_path);
    amplihack_utils::litellm_proxy::scrub_proxy_environment(&mut command);
    command
        .args([
            "clone",
            "--depth",
            "1",
            REPO_GIT_URL,
            &destination.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout_file
                .as_file()
                .try_clone()
                .context("failed to clone git stdout handle")?,
        ))
        .stderr(Stdio::from(
            stderr_file
                .as_file()
                .try_clone()
                .context("failed to clone git stderr handle")?,
        ));
    #[cfg(unix)]
    // SAFETY: `pre_exec` runs after fork and before exec. `setsid` is async-signal-safe
    // and isolates the git clone process tree so timeout cleanup can terminate it.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn git clone for {REPO_GIT_URL}"))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("failed to poll git clone status")?
        {
            break status;
        }
        if started.elapsed() >= GIT_CLONE_TIMEOUT {
            terminate_git_clone(&mut child);
            let stdout = read_limited(stdout_file.path())?;
            let stderr = read_limited(stderr_file.path())?;
            bail!(
                "git clone timed out after {:?} for {REPO_GIT_URL} into {}\nstdout:\n{}\nstderr:\n{}",
                GIT_CLONE_TIMEOUT,
                destination.display(),
                stdout,
                stderr
            );
        }
        thread::sleep(GIT_CLONE_POLL_INTERVAL);
    };
    if !status.success() {
        let stdout = read_limited(stdout_file.path())?;
        let stderr = read_limited(stderr_file.path())?;
        bail!(
            "git clone failed with status {status} for {REPO_GIT_URL} into {}\nstdout:\n{}\nstderr:\n{}",
            destination.display(),
            stdout,
            stderr
        );
    }
    Ok(())
}

fn terminate_git_clone(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if pid > 0 {
            // Negative pid targets the process group created with setsid above.
            unsafe {
                let _ = libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let mut command = Command::new("taskkill");
        amplihack_utils::litellm_proxy::scrub_proxy_environment(&mut command);
        command
            .args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = run_with_timeout(command, TERMINATE_PROCESS_TREE_TIMEOUT);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn read_limited(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("failed to open captured output {}", path.display()))?;
    let total_bytes = file
        .metadata()
        .with_context(|| format!("failed to inspect captured output {}", path.display()))?
        .len() as usize;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(CAPTURE_LIMIT as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read captured output {}", path.display()))?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if total_bytes > CAPTURE_LIMIT {
        let discarded = total_bytes - CAPTURE_LIMIT;
        text.push_str(&format!("\n[truncated: discarded {discarded} bytes]"));
    }
    Ok(text)
}

pub(super) fn find_framework_repo_root(root: &Path) -> Result<PathBuf> {
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        // Accept either `.claude/` (Python repo layout) or
        // `amplifier-bundle/` (Rust repo layout) as a repo root marker
        // (fix #254).
        if dir.join(".claude").is_dir() || dir.join("amplifier-bundle").is_dir() {
            return Ok(dir);
        }

        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry.with_context(|| format!("failed to inspect {}", dir.display()))?;
            if entry
                .file_type()
                .with_context(|| format!("failed to inspect {}", entry.path().display()))?
                .is_dir()
            {
                queue.push_back(entry.path());
            }
        }
    }

    bail!(
        "downloaded framework archive did not contain a repository root with .claude or amplifier-bundle under {}",
        root.display()
    )
}

pub(super) fn find_compatible_framework_repo_root(root: &Path, source: &str) -> Result<PathBuf> {
    let repo_root = find_framework_repo_root(root)?;
    if let Err(error) = validate_framework_bundle_compatibility(&repo_root) {
        return Err(anyhow::anyhow!(
            "downloaded framework bundle from {source} is incompatible at {}: {error}",
            repo_root.display()
        ));
    }

    Ok(repo_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Accept everything: isolates the *ordering and bound* under test from
    /// the (separately tested) bundle compatibility rules.
    fn accept_all(_candidate: &Path) -> Result<(), String> {
        Ok(())
    }

    /// A directory carrying a structurally plausible bundle and nothing else —
    /// exactly what the pre-#1275 shape check accepted from any ancestor.
    fn bundle_only(dir: &Path) -> PathBuf {
        fs::create_dir_all(dir.join("amplifier-bundle").join("recipes")).unwrap();
        dir.to_path_buf()
    }

    /// A directory that also looks like a complete framework checkout.
    fn amplihack_source(dir: &Path) -> PathBuf {
        bundle_only(dir);
        fs::create_dir_all(dir.join(".claude")).unwrap();
        dir.to_path_buf()
    }

    fn git_dir(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    fn cwd_query(cwd: &Path) -> FrameworkRootQuery {
        FrameworkRootQuery {
            current_dir: Some(cwd.to_path_buf()),
            ..FrameworkRootQuery::default()
        }
    }

    fn rendered(notes: &[ResolutionNote]) -> String {
        notes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn walk_up_finds_the_checkout_that_contains_the_current_directory() {
        // Fix #341's workflow: `amplihack install` run from deep inside the
        // clone must stage from that clone.
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("amplihack-rs");
        amplihack_source(&repo);
        git_dir(&repo);
        let cwd = repo.join("crates").join("amplihack-cli");
        fs::create_dir_all(&cwd).unwrap();

        let resolution = resolve_framework_root(&cwd_query(&cwd), &accept_all);

        assert_eq!(
            resolution.resolved,
            Some(ResolvedFrameworkRoot {
                root: repo,
                origin: FrameworkRootOrigin::CurrentDirectory,
            }),
            "the enclosing checkout must still win (fix #341)"
        );
        assert!(resolution.notes.is_empty(), "{:?}", resolution.notes);
    }

    #[test]
    fn a_bundle_in_an_ancestor_outside_the_repository_is_refused_and_reported() {
        // Issue #1275: the hostile shape. `~/src` carries a perfectly
        // well-formed amplihack source tree; the user is working in an
        // unrelated repository underneath it. The old walk-up staged from
        // `~/src`; nothing about the user's cwd asked for that.
        let temp = tempfile::tempdir().unwrap();
        let ancestor = temp.path().join("src");
        amplihack_source(&ancestor);
        let project = ancestor.join("unrelated-project");
        git_dir(&project);
        let cwd = project.join("lib");
        fs::create_dir_all(&cwd).unwrap();

        let resolution = resolve_framework_root(&cwd_query(&cwd), &accept_all);

        assert_eq!(
            resolution.resolved, None,
            "a bundle outside the current repository must not be staged implicitly"
        );
        assert_eq!(
            resolution.notes,
            vec![ResolutionNote::OutsideStartingRepository {
                path: ancestor.clone(),
                start: cwd,
            }]
        );
        let text = rendered(&resolution.notes);
        assert!(
            text.contains(&ancestor.display().to_string()) && text.contains("install --local"),
            "the refusal must name the directory and the deliberate route: {text}"
        );
    }

    #[test]
    fn without_an_enclosing_repository_only_the_starting_directory_is_considered() {
        // An unpacked tarball has no `.git`. Installing from inside it still
        // works (this is the shape `bugfix_341_install_freshness` builds), but
        // it does not license a climb towards `$HOME`.
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        amplihack_source(&checkout);
        let nested = checkout.join("docs");
        fs::create_dir_all(&nested).unwrap();

        let from_root = resolve_framework_root(&cwd_query(&checkout), &accept_all);
        assert_eq!(
            from_root.resolved.map(|r| r.root),
            Some(checkout.clone()),
            "standing in the unpacked checkout must resolve to it"
        );

        let from_nested = resolve_framework_root(&cwd_query(&nested), &accept_all);
        assert_eq!(
            from_nested.resolved, None,
            "with no repository to bound the walk, only the start directory counts"
        );
        assert_eq!(
            from_nested.notes,
            vec![ResolutionNote::OutsideStartingRepository {
                path: checkout,
                start: nested,
            }]
        );
    }

    #[test]
    fn a_directory_that_merely_carries_a_bundle_is_not_an_install_source() {
        // The shape check alone cannot tell an amplihack checkout from any
        // project that happens to vendor a directory of the same shape.
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("some-project");
        bundle_only(&repo);
        git_dir(&repo);

        let resolution = resolve_framework_root(&cwd_query(&repo), &accept_all);

        assert_eq!(resolution.resolved, None);
        assert_eq!(
            resolution.notes,
            vec![ResolutionNote::NotAmplihackSource { path: repo.clone() }]
        );

        // The cargo workspace root is the other recognised shape.
        fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/amplihack-cli\"]\n",
        )
        .unwrap();
        let resolution = resolve_framework_root(&cwd_query(&repo), &accept_all);
        assert_eq!(
            resolution.resolved.map(|r| r.root),
            Some(repo),
            "the amplihack cargo workspace root must be recognised"
        );
    }

    #[test]
    fn a_git_file_marks_a_worktree_root() {
        // Linked worktrees and submodules carry `.git` as a file, not a
        // directory. Getting that wrong would silently unbound the walk.
        let temp = tempfile::tempdir().unwrap();
        let ancestor = temp.path().join("src");
        amplihack_source(&ancestor);
        let worktree = ancestor.join("wt-1275");
        amplihack_source(&worktree);
        fs::write(
            worktree.join(".git"),
            "gitdir: /elsewhere/.git/worktrees/wt\n",
        )
        .unwrap();
        let cwd = worktree.join("crates");
        fs::create_dir_all(&cwd).unwrap();

        let resolution = resolve_framework_root(&cwd_query(&cwd), &accept_all);

        assert_eq!(
            resolution.resolved.map(|r| r.root),
            Some(worktree),
            "the worktree, not the directory above it, is the bound"
        );
    }

    #[test]
    fn amplihack_home_is_taken_at_its_word() {
        // An explicit override needs no source markers and no walk: the user
        // named this directory.
        let temp = tempfile::tempdir().unwrap();
        let explicit = bundle_only(&temp.path().join("explicit"));

        let resolution = resolve_framework_root(
            &FrameworkRootQuery {
                amplihack_home: Some(explicit.clone()),
                ..FrameworkRootQuery::default()
            },
            &accept_all,
        );

        assert_eq!(
            resolution.resolved,
            Some(ResolvedFrameworkRoot {
                root: explicit,
                origin: FrameworkRootOrigin::AmplihackHome,
            })
        );
    }

    #[test]
    fn an_incompatible_bundle_is_reported_rather_than_dropped() {
        let temp = tempfile::tempdir().unwrap();
        let explicit = bundle_only(&temp.path().join("explicit"));

        let resolution = resolve_framework_root(
            &FrameworkRootQuery {
                amplihack_home: Some(explicit.clone()),
                ..FrameworkRootQuery::default()
            },
            &|_| Err("missing required recipe".to_string()),
        );

        assert_eq!(resolution.resolved, None);
        let text = rendered(&resolution.notes);
        assert!(
            text.contains("Skipping incompatible framework bundle from AMPLIHACK_HOME")
                && text.contains("missing required recipe"),
            "{text}"
        );
    }

    #[test]
    fn the_executable_walk_up_stays_inside_its_own_checkout() {
        let temp = tempfile::tempdir().unwrap();

        // In-tree dev binary: `<repo>/target/debug/amplihack` must still find
        // the repo it was built in.
        let repo = temp.path().join("amplihack-rs");
        amplihack_source(&repo);
        git_dir(&repo);
        let exe = repo.join("target").join("debug").join("amplihack");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        let resolution = resolve_framework_root(
            &FrameworkRootQuery {
                current_exe: Some(exe),
                ..FrameworkRootQuery::default()
            },
            &accept_all,
        );
        assert_eq!(
            resolution.resolved,
            Some(ResolvedFrameworkRoot {
                root: repo,
                origin: FrameworkRootOrigin::ExecutableParent,
            })
        );

        // Installed binary: `~/.local/bin/amplihack` must not climb to a
        // bundle sitting in the home directory above it.
        let home = temp.path().join("home");
        amplihack_source(&home);
        let installed = home.join(".local").join("bin").join("amplihack");
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        let resolution = resolve_framework_root(
            &FrameworkRootQuery {
                current_exe: Some(installed),
                ..FrameworkRootQuery::default()
            },
            &accept_all,
        );
        assert_eq!(
            resolution.resolved, None,
            "an installed binary has no enclosing checkout to walk"
        );
        assert!(
            resolution.notes.is_empty(),
            "the executable walk-up has no `--local` advice to give: {:?}",
            resolution.notes
        );
    }

    #[test]
    fn a_prior_staged_install_is_the_last_local_step() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let staged = home.join(".amplihack");
        amplihack_source(&staged);

        let resolution = resolve_framework_root(
            &FrameworkRootQuery {
                home: Some(home),
                ..FrameworkRootQuery::default()
            },
            &accept_all,
        );

        assert_eq!(
            resolution.resolved,
            Some(ResolvedFrameworkRoot {
                root: staged,
                origin: FrameworkRootOrigin::PriorStagedInstall,
            })
        );
    }

    #[test]
    fn the_current_checkout_outranks_the_compile_time_workspace() {
        // Fix #341 in one assertion: a binary built elsewhere must not pin the
        // bundle to whatever was on disk at build time.
        let temp = tempfile::tempdir().unwrap();
        let built_from = amplihack_source(&temp.path().join("build-host"));
        let repo = temp.path().join("amplihack-rs");
        amplihack_source(&repo);
        git_dir(&repo);

        let resolution = resolve_framework_root(
            &FrameworkRootQuery {
                current_dir: Some(repo.clone()),
                compile_time_workspace_root: Some(built_from),
                ..FrameworkRootQuery::default()
            },
            &accept_all,
        );

        assert_eq!(
            resolution.resolved,
            Some(ResolvedFrameworkRoot {
                root: repo,
                origin: FrameworkRootOrigin::CurrentDirectory,
            })
        );
    }

    #[test]
    fn every_origin_explains_itself() {
        for origin in [
            FrameworkRootOrigin::AmplihackHome,
            FrameworkRootOrigin::CurrentDirectory,
            FrameworkRootOrigin::ExecutableParent,
            FrameworkRootOrigin::CompileTimeWorkspaceRoot,
            FrameworkRootOrigin::PriorStagedInstall,
        ] {
            assert!(
                !origin.describe().is_empty() && !origin.label().is_empty(),
                "{origin:?} must be printable in install output"
            );
        }
    }

    #[cfg(unix)]
    fn fake_git(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("git");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn git_clone_reports_nonzero_exit_with_captured_output() {
        let temp = tempfile::tempdir().unwrap();
        let fake = fake_git(
            temp.path(),
            "echo stdout-marker; echo stderr-marker >&2; exit 42",
        );

        let err = git_clone_framework_repo(&fake, &temp.path().join("dest"))
            .expect_err("non-zero git clone must fail");
        let msg = format!("{err:#}");

        assert!(
            msg.contains("status"),
            "error must include exit status: {msg}"
        );
        assert!(
            msg.contains("stdout-marker") && msg.contains("stderr-marker"),
            "error must include captured stdout/stderr: {msg}"
        );
    }

    #[test]
    fn read_limited_reports_discarded_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("captured.log");
        fs::write(&output, vec![b'x'; CAPTURE_LIMIT + 7]).unwrap();

        let rendered = read_limited(&output).unwrap();

        assert!(
            rendered.contains("[truncated: discarded 7 bytes]"),
            "truncated output must report discarded bytes: {rendered}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_clone_times_out_and_reaps_child() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("orphan-marker");
        let fake = fake_git(
            temp.path(),
            &format!(
                "(/bin/sleep 1; echo orphan > '{}') & echo started; /bin/sleep 5",
                marker.display()
            ),
        );
        let start = Instant::now();

        let err = git_clone_framework_repo(&fake, &temp.path().join("dest"))
            .expect_err("hung git clone must time out");
        let msg = format!("{err:#}");

        assert!(
            start.elapsed() < Duration::from_secs(2),
            "test timeout should be bounded, elapsed {:?}",
            start.elapsed()
        );
        assert!(msg.contains("timed out"), "error must name timeout: {msg}");
        assert!(
            msg.contains("started"),
            "timeout error must include captured output: {msg}"
        );
        thread::sleep(Duration::from_millis(1_200));
        assert!(
            !marker.exists(),
            "timeout cleanup must terminate git clone descendants"
        );
    }
}
