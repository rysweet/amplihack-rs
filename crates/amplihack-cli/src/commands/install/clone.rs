//! Framework source resolution over local candidates, and the bounded git runner.
//!
//! As of issue #254 the framework assets are bundled inside the amplihack-rs
//! source tree (`amplifier-bundle/`) and no longer fetched from the upstream
//! `rysweet/amplihack` repository at install time. Issue #1955 adds a version
//! gate to every step here; see [`super::framework_source`].
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
//!
//! When none of these has this binary's version, the source is the exact release
//! tag ([`super::tag_fetch`]), never a branch.
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
use super::framework_source::{self, MismatchRule, SourceRefused, SourceVersion};
use super::paths::{sanitize_path, sanitize_text};
#[cfg(windows)]
use crate::util::run_with_timeout;
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GIT_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(windows)]
const TERMINATE_PROCESS_TREE_TIMEOUT: Duration = Duration::from_secs(5);
const CAPTURE_LIMIT: usize = 8192;

/// SEC-2: removed from every git call. The first 15 are
/// `git rev-parse --local-env-vars`, which could point git at another repository
/// or config; the rest can run a command or supply templates.
const GIT_SCRUBBED_ENV: [&str; 20] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_SSH_COMMAND",
    "GIT_PROXY_COMMAND",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_TEMPLATE_DIR",
];

/// SEC-6: a repository's own config can't run anything through these.
#[cfg(not(windows))]
const GIT_HOOKS_OFF: &str = "core.hooksPath=/dev/null";
#[cfg(windows)]
const GIT_HOOKS_OFF: &str = "core.hooksPath=NUL";

/// Workspace-member path that identifies an amplihack-rs `Cargo.toml`.
pub(super) const AMPLIHACK_WORKSPACE_MARKER: &str = "crates/amplihack-cli";

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

    /// Short label used in diagnostics about a candidate.
    pub(super) fn label(self) -> &'static str {
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
    /// A valid candidate whose version is not the binary's (issue #1955).
    VersionMismatch {
        path: PathBuf,
        label: &'static str,
        source_version: SourceVersion,
        binary_version: String,
    },
}

impl std::fmt::Display for ResolutionNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompatibleBundle { path, label, error } => write!(
                f,
                "⚠️  Skipping incompatible framework bundle from {label}: {}: {}",
                sanitize_path(path),
                sanitize_text(error)
            ),
            Self::OutsideStartingRepository { path, start } => write!(
                f,
                "ℹ️  Ignoring the framework bundle at {path}: it is outside the repository \
                 containing {}, so amplihack will not stage from it implicitly. \
                 Run `amplihack install --local {path}` to stage from it deliberately.",
                sanitize_path(start),
                path = sanitize_path(path),
            ),
            Self::NotAmplihackSource { path } => write!(
                f,
                "ℹ️  Ignoring the framework bundle at {path}: that directory does not look like \
                 an amplihack source checkout (no `.claude/`, no amplihack cargo workspace). \
                 Run `amplihack install --local {path}` to stage from it deliberately.",
                path = sanitize_path(path),
            ),
            Self::VersionMismatch {
                path,
                label,
                source_version,
                binary_version,
            } => write!(
                f,
                "ℹ️  Ignoring the framework source at {} ({label}): its version is \
                 {source_version} and this binary is v{binary_version}.",
                sanitize_path(path),
            ),
        }
    }
}

/// Everything resolution decided, including the candidates it turned down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct FrameworkRootResolution {
    pub(super) resolved: Option<ResolvedFrameworkRoot>,
    /// The version the gate read for `resolved`.
    pub(super) version: Option<SourceVersion>,
    /// A plain `install` stops at a mismatched source the user pointed at.
    pub(super) refused: Option<SourceRefused>,
    pub(super) notes: Vec<ResolutionNote>,
}

/// What the version gate decides about a valid candidate (issue #1955).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CandidateVerdict {
    Accept(SourceVersion),
    Skip(ResolutionNote),
    Refuse(SourceRefused),
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

impl FrameworkRootQuery {
    /// The environment of this process.
    pub(super) fn from_environment() -> Self {
        Self {
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
        }
    }

    /// D5: sessions set `AMPLIHACK_HOME` to `~/.amplihack` themselves, so that
    /// directory is judged as the staged install it is, never as a source the
    /// user pointed at. A path that can't be canonicalized doesn't count.
    pub(super) fn without_staged_home(&self) -> Self {
        let mut query = self.clone();
        if let (Some(amplihack_home), Some(home)) = (&self.amplihack_home, &self.home)
            && let (Ok(named), Ok(staged)) = (
                amplihack_home.canonicalize(),
                home.join(".amplihack").canonicalize(),
            )
            && named == staged
        {
            query.amplihack_home = None;
        }
        query
    }
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
    // SEC-11: only a regular file, so a FIFO in a directory the user is only
    // standing in can't block a launch.
    let cargo_toml = dir.join("Cargo.toml");
    if !fs::symlink_metadata(&cargo_toml).is_ok_and(|meta| meta.is_file()) {
        return false;
    }
    let Ok(file) = fs::File::open(&cargo_toml) else {
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

/// The version gate, applied to each candidate that passes validation.
type CandidateGate<'a> = dyn Fn(&Path, FrameworkRootOrigin) -> CandidateVerdict + 'a;

/// The checks resolution applies to a candidate, and what they found.
struct CandidateChecks<'a> {
    validate: &'a dyn Fn(&Path) -> Result<(), String>,
    gate: &'a CandidateGate<'a>,
    notes: Vec<ResolutionNote>,
}

impl CandidateChecks<'_> {
    /// Accept `candidate` if it carries a compatible `amplifier-bundle/` and
    /// passes the gate. Validation comes first, so an incompatible source is
    /// skipped whatever its version (D4).
    ///
    /// Records why a shape-valid bundle was turned down instead of failing mute.
    fn check(
        &mut self,
        candidate: &Path,
        origin: FrameworkRootOrigin,
    ) -> Result<Option<SourceVersion>, SourceRefused> {
        if !candidate.join("amplifier-bundle").is_dir() {
            return Ok(None);
        }
        if let Err(error) = (self.validate)(candidate) {
            self.notes.push(ResolutionNote::IncompatibleBundle {
                path: candidate.to_path_buf(),
                label: origin.label(),
                error,
            });
            return Ok(None);
        }
        match (self.gate)(candidate, origin) {
            CandidateVerdict::Accept(version) => Ok(Some(version)),
            CandidateVerdict::Skip(note) => {
                self.notes.push(note);
                Ok(None)
            }
            CandidateVerdict::Refuse(refused) => Err(refused),
        }
    }

    /// Bounded walk-up from `start`, accepting only amplihack source trees.
    ///
    /// `report_out_of_scope` asks for a note naming the nearest bundle-carrying
    /// ancestor the bound kept us from reaching — the pre-#1275 answer — so a user
    /// who expected that directory to win learns why it did not, and how to ask
    /// for it on purpose.
    fn walk_up(
        &mut self,
        start: &Path,
        origin: FrameworkRootOrigin,
        report_out_of_scope: bool,
    ) -> Result<Option<(PathBuf, SourceVersion)>, SourceRefused> {
        let scope = walk_up_scope(start);
        for dir in &scope {
            if !dir.join("amplifier-bundle").is_dir() {
                continue;
            }
            if !looks_like_amplihack_source(dir) {
                self.notes
                    .push(ResolutionNote::NotAmplihackSource { path: dir.clone() });
                continue;
            }
            if let Some(version) = self.check(dir, origin)? {
                return Ok(Some((dir.clone(), version)));
            }
        }

        if report_out_of_scope
            && let Some(boundary) = scope.last()
            && let Some(out_of_scope) = boundary
                .ancestors()
                .skip(1)
                .find(|dir| dir.join("amplifier-bundle").is_dir())
        {
            self.notes.push(ResolutionNote::OutsideStartingRepository {
                path: out_of_scope.to_path_buf(),
                start: start.to_path_buf(),
            });
        }

        Ok(None)
    }

    fn walk_candidates(
        &mut self,
        query: &FrameworkRootQuery,
    ) -> Result<Option<(PathBuf, FrameworkRootOrigin, SourceVersion)>, SourceRefused> {
        // 1. AMPLIHACK_HOME env var — explicit user override. No source-shape
        //    requirement: the user named this directory on purpose.
        let origin = FrameworkRootOrigin::AmplihackHome;
        if let Some(home) = &query.amplihack_home
            && let Some(version) = self.check(home, origin)?
        {
            return Ok(Some((home.clone(), origin, version)));
        }

        // 2. Bounded walk-up from the current directory — the checkout the user is
        //    installing from (fix #341), and no further (issue #1275).
        let origin = FrameworkRootOrigin::CurrentDirectory;
        if let Some(cwd) = &query.current_dir
            && let Some((root, version)) = self.walk_up(cwd, origin, true)?
        {
            return Ok(Some((root, origin, version)));
        }

        // 3. Bounded walk-up from the executable (in-tree dev binary under
        //    `target/`). An installed binary has no enclosing checkout, so this
        //    step now stops instead of climbing towards `$HOME`.
        let origin = FrameworkRootOrigin::ExecutableParent;
        if let Some(exe_dir) = query.current_exe.as_deref().and_then(Path::parent)
            && let Some((root, version)) = self.walk_up(exe_dir, origin, false)?
        {
            return Ok(Some((root, origin, version)));
        }

        // 4. Compile-time workspace root (only meaningful for `cargo run` from
        //    the workspace; demoted because for installed binaries it pins the
        //    bundle to whatever was on disk at build time — issue #341).
        let origin = FrameworkRootOrigin::CompileTimeWorkspaceRoot;
        if let Some(workspace_root) = &query.compile_time_workspace_root
            && let Some(version) = self.check(workspace_root, origin)?
        {
            return Ok(Some((workspace_root.clone(), origin, version)));
        }

        // 5. ~/.amplihack (from prior staged install)
        let origin = FrameworkRootOrigin::PriorStagedInstall;
        if let Some(home) = &query.home {
            let dot = home.join(".amplihack");
            if dot.join(".claude").is_dir()
                && let Some(version) = self.check(&dot, origin)?
            {
                return Ok(Some((dot, origin, version)));
            }
        }

        Ok(None)
    }
}

/// Resolve the framework root from an explicit description of the environment.
///
/// Pure with respect to the process: every input is an argument, including the
/// bundle compatibility check and the version gate, so the ordering and the
/// bound are testable against fabricated directory layouts.
pub(super) fn resolve_framework_root_gated(
    query: &FrameworkRootQuery,
    validate: &dyn Fn(&Path) -> Result<(), String>,
    gate: &CandidateGate<'_>,
) -> FrameworkRootResolution {
    let mut checks = CandidateChecks {
        validate,
        gate,
        notes: Vec::new(),
    };
    let outcome = checks.walk_candidates(query);
    let notes = checks.notes;
    match outcome {
        Ok(Some((root, origin, version))) => FrameworkRootResolution {
            resolved: Some(ResolvedFrameworkRoot { root, origin }),
            version: Some(version),
            refused: None,
            notes,
        },
        Ok(None) => FrameworkRootResolution {
            notes,
            ..FrameworkRootResolution::default()
        },
        Err(refused) => FrameworkRootResolution {
            refused: Some(refused),
            notes,
            ..FrameworkRootResolution::default()
        },
    }
}

/// [`resolve_framework_root_gated`] with no version gate, for the tests of
/// ordering and the walk-up bound.
#[cfg(test)]
pub(super) fn resolve_framework_root(
    query: &FrameworkRootQuery,
    validate: &dyn Fn(&Path) -> Result<(), String>,
) -> FrameworkRootResolution {
    resolve_framework_root_gated(query, validate, &|_, _| {
        CandidateVerdict::Accept(SourceVersion::Unknown)
    })
}

/// Locate a framework source with this binary's version, without any network
/// access, for callers that read from it rather than stage a whole bundle.
///
/// Returns the repo root (the directory that contains `amplifier-bundle/`
/// and — for a complete source checkout — `.claude/`) together with the step
/// that found it. Returns `None` when no local source has this binary's version.
///
/// Candidates that were turned down go to the debug log, not stderr: this runs
/// on every Codex launch, and only staging needs to say why a fetch runs.
pub(super) fn find_bundled_framework_root() -> Option<ResolvedFrameworkRoot> {
    find_bundled_framework_root_for(&FrameworkRootQuery::from_environment(), crate::VERSION)
}

/// [`find_bundled_framework_root`] with its inputs passed in (D9).
pub(super) fn find_bundled_framework_root_for(
    query: &FrameworkRootQuery,
    binary_version: &str,
) -> Option<ResolvedFrameworkRoot> {
    let resolution = framework_source::resolve_gated(
        query,
        binary_version,
        MismatchRule::NoteAndContinue,
        &|candidate| {
            validate_framework_bundle_compatibility(candidate).map_err(|err| format!("{err:#}"))
        },
    );
    for note in &resolution.notes {
        tracing::debug!("{note}");
    }
    resolution.resolved
}

/// Resolve the `git` binary path from PATH.
pub(super) fn which_git() -> Result<PathBuf> {
    let Some(paths) = std::env::var_os("PATH") else {
        bail!("git not found on PATH");
    };
    which_git_in(&paths)
}

/// [`which_git`] over an explicit PATH value. Relative entries are skipped
/// (SEC-8): version reading runs in untrusted directories, and `.` on PATH must
/// not hand git to whatever the directory holds.
pub(super) fn which_git_in(paths: &std::ffi::OsStr) -> Result<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &["git.exe", "git"]
    } else {
        &["git"]
    };
    for dir in std::env::split_paths(paths).filter(|dir| dir.is_absolute()) {
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

/// What a bounded git call did. `status` is `None` when it hit its time limit.
#[derive(Debug)]
pub(super) struct GitOutput {
    pub(super) status: Option<ExitStatus>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    /// All of stdout, including what `stdout` was truncated to leave out.
    pub(super) stdout_bytes: u64,
}

impl GitOutput {
    pub(super) fn succeeded(&self) -> bool {
        self.status.is_some_and(|status| status.success())
    }
}

/// The one way install runs git: scrubbed environment, hooks and credential
/// helpers off, no stdin, its own session, and killed with its children at
/// `timeout` (SEC-2, SEC-6).
pub(super) fn run_bounded_git(
    git: &Path,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<GitOutput> {
    run_bounded_git_with(git, args, cwd, timeout, &|_| {})
}

/// [`run_bounded_git`] with `configure` applied to the command just before the
/// spawn.
pub(super) fn run_bounded_git_with(
    git: &Path,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
    configure: &dyn Fn(&mut Command),
) -> Result<GitOutput> {
    let stdout_file =
        tempfile::NamedTempFile::new().context("failed to create temporary stdout file for git")?;
    let stderr_file =
        tempfile::NamedTempFile::new().context("failed to create temporary stderr file for git")?;
    let mut command = Command::new(git);
    amplihack_utils::litellm_proxy::scrub_inference_environment(&mut command);
    for name in GIT_SCRUBBED_ENV {
        command.env_remove(name);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            GIT_HOOKS_OFF,
            "-c",
            "core.fsmonitor=false",
            "-c",
            "credential.helper=",
        ])
        .args(args)
        .current_dir(cwd)
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
    // and isolates the git process tree so timeout cleanup can terminate it.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    configure(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {} in {}", git.display(), cwd.display()))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("failed to poll git status")? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            terminate_git(&mut child);
            break None;
        }
        thread::sleep(GIT_POLL_INTERVAL);
    };
    let stdout_bytes = stdout_file
        .as_file()
        .metadata()
        .context("failed to inspect captured git stdout")?
        .len();
    Ok(GitOutput {
        status,
        stdout: read_limited(stdout_file.path())?,
        stderr: read_limited(stderr_file.path())?,
        stdout_bytes,
    })
}

fn terminate_git(child: &mut std::process::Child) {
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
        amplihack_utils::litellm_proxy::scrub_inference_environment(&mut command);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::commands::profile::ROOT_ENV;

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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
    fn a_scratch_directory_has_no_repository_marker_above_it() {
        // Issue #1937: a stray `.git` in the scratch parent (for example
        // `/tmp/.git`, written by a fixture that inherited a hook's GIT_DIR)
        // makes every walk-up that starts below it stop there. The walk-up
        // tests above all build their fixtures in a scratch directory, so the
        // scope of a fresh one must be that directory alone.
        let temp = crate::test_support::tempdir_outside_framework_roots();

        assert_eq!(
            walk_up_scope(temp.path()),
            vec![temp.path().to_path_buf()],
            "a repository marker above the scratch directory {} was found by the walk-up; \
             remove the stray .git from its ancestors",
            temp.path().display()
        );
    }

    #[test]
    fn a_directory_that_merely_carries_a_bundle_is_not_an_install_source() {
        // The shape check alone cannot tell an amplihack checkout from any
        // project that happens to vendor a directory of the same shape.
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();

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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
        let temp = crate::test_support::tempdir_outside_framework_roots();
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
    /// Written through `test_support::write_executable`, so no concurrent
    /// fork can hold a write descriptor to the script when it is spawned
    /// (`ETXTBSY`, issue #1973).
    fn fake_git(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("git");
        crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{body}\n"));
        path
    }

    #[cfg(unix)]
    #[test]
    fn bounded_git_reports_nonzero_exit_with_captured_output() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let fake = fake_git(
            temp.path(),
            "echo stdout-marker; echo stderr-marker >&2; exit 42",
        );

        let output =
            run_bounded_git(&fake, &["fetch"], temp.path(), Duration::from_secs(30)).unwrap();

        assert_eq!(output.status.and_then(|status| status.code()), Some(42));
        assert!(!output.succeeded());
        assert!(output.stdout.contains("stdout-marker"), "{output:?}");
        assert!(output.stderr.contains("stderr-marker"), "{output:?}");
    }

    #[test]
    fn read_limited_reports_discarded_bytes() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let output = temp.path().join("captured.log");
        fs::write(&output, vec![b'x'; CAPTURE_LIMIT + 7]).unwrap();

        let rendered = read_limited(&output).unwrap();

        assert!(
            rendered.contains("[truncated: discarded 7 bytes]"),
            "truncated output must report discarded bytes: {rendered}"
        );
    }

    /// Issue #1765: the fake outlives the bound by far and the bound is loose,
    /// so only a timeout that never fires can fail it. The orphan writes its
    /// marker 5 s after it starts, long after the 250 ms timeout, so a
    /// surviving descendant has time to show itself before the check.
    #[cfg(unix)]
    #[test]
    fn bounded_git_times_out_and_reaps_child() {
        let temp = crate::test_support::tempdir_outside_framework_roots();
        let marker = temp.path().join("orphan-marker");
        let marker_text = marker.to_str().expect("temporary paths are UTF-8");
        assert!(
            !marker_text.contains(['\'', '\n']),
            "cannot single-quote {marker_text:?} for /bin/sh"
        );
        let fake = fake_git(
            temp.path(),
            &format!(
                "(/bin/sleep 5; echo orphan > '{marker_text}') & echo started; exec /bin/sleep 60"
            ),
        );
        let start = Instant::now();

        let output = run_bounded_git_with(
            &fake,
            &["fetch"],
            temp.path(),
            Duration::from_millis(250),
            &|command: &mut Command| {
                command.env("HOME", temp.path()).env_remove(ROOT_ENV);
            },
        )
        .unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(30),
            "test timeout should be bounded, elapsed {elapsed:?}"
        );
        assert!(output.status.is_none(), "a hung git must time out");
        assert!(
            output.stdout.contains("started"),
            "a timeout must still return the captured output: {output:?}"
        );
        if let Some(remaining) = Duration::from_secs(6).checked_sub(start.elapsed()) {
            thread::sleep(remaining);
        }
        assert!(
            !marker.exists(),
            "timeout cleanup must terminate git's descendants (the call took {elapsed:?})"
        );
    }
}
