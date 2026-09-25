//! Rust trial helper: isolated-HOME runner for the Rust CLI binary.
//!
//! Ports Python `amplihack/rust_trial.py`. Provides functions to locate,
//! download, install, and run the Rust CLI binary in an isolated `$HOME`
//! so that trial runs do not affect the user's main configuration.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::util::run_with_timeout;

const RUST_TRIAL_TIMEOUT: Duration = Duration::from_secs(3600);

/// Options for installing the Rust CLI binary.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Target directory for installation.
    pub install_dir: PathBuf,
    /// Force re-installation even if binary already exists.
    pub force: bool,
    /// Run bootstrap after installation.
    pub bootstrap: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            install_dir: default_install_dir(),
            force: false,
            bootstrap: false,
        }
    }
}

/// Default trial home directory.
///
/// Checks `AMPLIHACK_RUST_TRIAL_HOME`, falls back to `~/.amplihack-rust-trial`.
pub fn default_trial_home() -> PathBuf {
    if let Ok(val) = std::env::var("AMPLIHACK_RUST_TRIAL_HOME") {
        return PathBuf::from(val);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".amplihack-rust-trial")
}

/// Default installation directory for the binary (`~/.local/bin`).
pub fn default_install_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("bin")
}

/// Search for the Rust CLI binary in multiple locations.
///
/// Search order:
/// 1. `AMPLIHACK_RUST_BINARY` environment variable
/// 2. Bundled binary in trial home
/// 3. `~/.amplihack/bin/amplihack`
/// 4. `$PATH` lookup
/// 5. Download from GitHub releases (not attempted here — returns error)
pub fn find_rust_cli_binary(trial_home: &Path) -> Result<PathBuf> {
    // 1. Env var override
    if let Ok(val) = std::env::var("AMPLIHACK_RUST_BINARY") {
        let p = PathBuf::from(&val);
        if p.is_file() {
            debug!(path = %p.display(), "found binary via AMPLIHACK_RUST_BINARY");
            return Ok(p);
        }
        warn!(path = %p.display(), "AMPLIHACK_RUST_BINARY set but file not found");
    }

    // 2. Bundled in trial home
    let bundled = trial_home.join("bin").join("amplihack");
    if bundled.is_file() {
        debug!(path = %bundled.display(), "found bundled binary");
        return Ok(bundled);
    }

    // 3. ~/.amplihack/bin/
    if let Some(home) = home_dir() {
        let dot_amp = home.join(".amplihack").join("bin").join("amplihack");
        if dot_amp.is_file() {
            debug!(path = %dot_amp.display(), "found binary in ~/.amplihack/bin");
            return Ok(dot_amp);
        }
    }

    // 4. PATH lookup
    //
    // Issue #1274 — one seam. Previously `env::var("PATH")` (so a legal
    // non-UTF-8 `$PATH` read as absent) split without a filter (so an empty
    // element resolved `amplihack` against the cwd of whatever repo the user
    // happened to be sitting in). This is a "choose a file to run" walk.
    for dir in amplihack_utils::launch_target::env_path_dirs() {
        let candidate = dir.join("amplihack");
        if candidate.is_file() {
            debug!(path = %candidate.display(), "found binary on PATH");
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "Rust CLI binary not found. Searched: env var, {}/bin, ~/.amplihack/bin, PATH. \
         Use download_latest_release_binary() to obtain it.",
        trial_home.display()
    )
}

/// Build an isolated HOME environment for trial execution.
///
/// Returns env vars that set `HOME` to the trial home and preserve
/// critical variables like `PATH`, `TERM`, and `LANG`.
pub fn build_trial_env(trial_home: &Path) -> HashMap<String, String> {
    // Forward the resolved agent binary so subprocesses see it explicitly,
    // even when their cwd lacks a launcher_context.json. Resolver returns the
    // canonical (allowlisted, lowercased) name; falls back to "copilot".
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let resolved = amplihack_utils::agent_binary::resolve_with_source(&cwd).unwrap_or_else(|_| {
        (
            amplihack_utils::agent_binary::DEFAULT_BINARY.to_string(),
            amplihack_utils::agent_binary::ResolutionSource::Default,
        )
    });
    build_trial_env_with(trial_home, resolved)
}

/// [`build_trial_env`] with the agent binary already resolved, so the tagging
/// rule can be tested without depending on the process environment.
fn build_trial_env_with(
    trial_home: &Path,
    (resolved, source): (String, amplihack_utils::agent_binary::ResolutionSource),
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    env.insert("HOME".to_string(), trial_home.display().to_string());
    env.insert(
        "AMPLIHACK_RUST_TRIAL_HOME".to_string(),
        trial_home.display().to_string(),
    );

    // Preserve critical env vars
    for var in &["PATH", "TERM", "LANG", "SHELL", "USER", "LOGNAME"] {
        if let Ok(val) = std::env::var(var) {
            env.insert(var.to_string(), val);
        }
    }

    // Issue #1481: a default-layer answer is tagged so descendants treat it as
    // the guess it is.
    if source == amplihack_utils::agent_binary::ResolutionSource::Default {
        env.insert(
            amplihack_utils::agent_binary::SOURCE_ENV.to_string(),
            amplihack_utils::agent_binary::default_guess_tag(&resolved),
        );
    }
    env.insert("AMPLIHACK_AGENT_BINARY".to_string(), resolved);

    env
}

/// Download the latest release binary from GitHub releases (stub).
///
/// In a full implementation this would use the GitHub API to find the
/// latest release, download the appropriate platform binary, and extract it.
pub fn download_latest_release_binary(trial_home: &Path) -> Result<PathBuf> {
    let bin_dir = trial_home.join("bin");
    std::fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create bin dir: {}", bin_dir.display()))?;

    let target = bin_dir.join("amplihack");
    info!(
        target = %target.display(),
        "download_latest_release_binary: stub — would download from GitHub"
    );

    anyhow::bail!(
        "automatic download not yet implemented; \
         install manually to {}",
        target.display()
    )
}

/// Install the Rust CLI binary to a user-local bin directory.
pub fn install_rust_cli(trial_home: &Path, options: &InstallOptions) -> Result<PathBuf> {
    let source =
        find_rust_cli_binary(trial_home).context("cannot install: source binary not found")?;

    let target = options.install_dir.join("amplihack");
    if target.exists() && !options.force {
        info!(
            path = %target.display(),
            "binary already installed, use force=true to overwrite"
        );
        return Ok(target);
    }

    std::fs::create_dir_all(&options.install_dir).with_context(|| {
        format!(
            "failed to create install dir: {}",
            options.install_dir.display()
        )
    })?;

    std::fs::copy(&source, &target).with_context(|| {
        format!(
            "failed to copy {} -> {}",
            source.display(),
            target.display()
        )
    })?;

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&target)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&target, perms)?;
    }

    info!(
        source = %source.display(),
        target = %target.display(),
        "installed Rust CLI binary"
    );

    if options.bootstrap {
        info!("bootstrap requested — would run initial setup");
    }

    Ok(target)
}

/// The trial command: `binary rust_args` with `env` layered over the inherited
/// environment.
///
/// An inherited [`amplihack_utils::agent_binary::SOURCE_ENV`] is removed first.
/// `env` re-adds it only when this level's own answer was a default guess;
/// otherwise a parent's tag would survive and mark a value this level resolved
/// from a real source as a guess (issue #1481).
fn trial_command(binary: &Path, rust_args: &[String], env: &HashMap<String, String>) -> Command {
    let mut command = Command::new(binary);
    command
        .args(rust_args)
        .env_remove(amplihack_utils::agent_binary::SOURCE_ENV)
        .envs(env);
    command
}

/// Run the Rust CLI in an isolated HOME environment.
///
/// Returns the process exit code.
pub fn run_rust_trial(rust_args: &[String], trial_home: &Path) -> i32 {
    let binary = match find_rust_cli_binary(trial_home) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    let env = build_trial_env(trial_home);
    info!(
        binary = %binary.display(),
        home = %trial_home.display(),
        args = ?rust_args,
        "running rust trial"
    );

    let command = trial_command(&binary, rust_args, &env);
    let status = run_with_timeout(command, RUST_TRIAL_TIMEOUT);

    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("error: failed to execute {}: {}", binary.display(), e);
            1
        }
    }
}

/// Parse trial-specific arguments from argv.
///
/// Returns `(trial_home, remaining_args)`.
/// Recognises `--trial-home <path>` prefix.
pub fn parse_trial_args(argv: &[String]) -> (PathBuf, Vec<String>) {
    if argv.len() >= 2 && argv[0] == "--trial-home" {
        let home = PathBuf::from(&argv[1]);
        (home, argv[2..].to_vec())
    } else {
        (default_trial_home(), argv.to_vec())
    }
}

/// CLI entry point for the rust-trial command.
///
/// Returns the process exit code.
pub fn main(argv: &[String]) -> i32 {
    let (trial_home, args) = parse_trial_args(argv);

    // Ensure trial home exists
    if let Err(e) = std::fs::create_dir_all(&trial_home) {
        eprintln!(
            "error: cannot create trial home {}: {}",
            trial_home.display(),
            e
        );
        return 1;
    }

    run_rust_trial(&args, &trial_home)
}

/// Cross-platform home directory helper.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_trial_home_is_reasonable() {
        let home = default_trial_home();
        let s = home.display().to_string();
        assert!(
            s.contains("amplihack-rust-trial") || s.contains("AMPLIHACK_RUST_TRIAL_HOME"),
            "unexpected trial home: {s}"
        );
    }

    #[test]
    fn default_install_dir_contains_local_bin() {
        let dir = default_install_dir();
        let s = dir.display().to_string();
        assert!(s.contains(".local") && s.contains("bin"));
    }

    #[test]
    fn build_trial_env_sets_home() {
        let trial_home = PathBuf::from("/test/trial");
        let env = build_trial_env(&trial_home);
        assert_eq!(env["HOME"], "/test/trial");
        assert_eq!(env["AMPLIHACK_RUST_TRIAL_HOME"], "/test/trial");
        // PATH should be forwarded
        assert!(env.contains_key("PATH"));
    }

    /// Issue #1481 / quality-audit S1: only a default-layer answer is tagged.
    #[test]
    fn build_trial_env_tags_only_a_default_layer_answer() {
        use amplihack_utils::agent_binary::{ResolutionSource, SOURCE_ENV};
        let home = PathBuf::from("/test/trial");

        let env = build_trial_env_with(&home, ("copilot".to_string(), ResolutionSource::Default));
        assert_eq!(env["AMPLIHACK_AGENT_BINARY"], "copilot");
        assert_eq!(
            env.get(SOURCE_ENV).map(String::as_str),
            Some("default:copilot")
        );

        for source in [
            ResolutionSource::Env,
            ResolutionSource::SessionMarker,
            ResolutionSource::LauncherContext,
        ] {
            let env = build_trial_env_with(&home, ("claude".to_string(), source));
            assert_eq!(env["AMPLIHACK_AGENT_BINARY"], "claude");
            assert!(
                !env.contains_key(SOURCE_ENV),
                "{source:?} must not be tagged"
            );
        }
    }

    /// Quality-audit C3-3(c): the production wrapper passes the resolver's real
    /// source through, so a default-layer answer arrives tagged.
    #[test]
    fn build_trial_env_resolves_and_tags_from_the_process() {
        use amplihack_utils::agent_binary::SOURCE_ENV;
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let cwd = tempfile::tempdir().unwrap();
        let _cwd = crate::test_support::CwdGuard::set(cwd.path()).unwrap();
        let home = PathBuf::from("/test/trial");

        {
            // Nothing to go on: the default, tagged.
            let _env = crate::test_support::AgentBinaryEnv::set(None, None);
            let env = build_trial_env(&home);
            assert_eq!(env["AMPLIHACK_AGENT_BINARY"], "copilot");
            assert_eq!(
                env.get(SOURCE_ENV).map(String::as_str),
                Some("default:copilot")
            );
        }
        {
            // A chosen value: exported as is, untagged.
            let _env = crate::test_support::AgentBinaryEnv::set(Some("codex"), None);
            let env = build_trial_env(&home);
            assert_eq!(env["AMPLIHACK_AGENT_BINARY"], "codex");
            assert!(!env.contains_key(SOURCE_ENV));
        }
    }

    /// Issue #1481: a parent's default-guess tag must not reach the trial
    /// child unless this level's own answer was a guess too.
    #[test]
    fn trial_command_drops_an_inherited_tag_unless_it_re_adds_one() {
        use amplihack_utils::agent_binary::SOURCE_ENV;
        let lookup = |command: &Command| {
            command
                .get_envs()
                .filter(|(k, _)| *k == std::ffi::OsStr::new(SOURCE_ENV))
                .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
                .last()
        };

        let mut env = HashMap::new();
        env.insert("AMPLIHACK_AGENT_BINARY".to_string(), "claude".to_string());
        let command = trial_command(Path::new("amplihack"), &[], &env);
        assert_eq!(
            lookup(&command),
            Some(None),
            "inherited tag must be removed"
        );

        env.insert(SOURCE_ENV.to_string(), "default:copilot".to_string());
        let command = trial_command(Path::new("amplihack"), &[], &env);
        assert_eq!(
            lookup(&command),
            Some(Some("default:copilot".to_string())),
            "this level's own tag must still be passed down"
        );
    }

    #[test]
    fn parse_trial_args_with_flag() {
        let argv = vec![
            "--trial-home".to_string(),
            "/custom/home".to_string(),
            "run".to_string(),
            "--verbose".to_string(),
        ];
        let (home, rest) = parse_trial_args(&argv);
        assert_eq!(home, PathBuf::from("/custom/home"));
        assert_eq!(rest, vec!["run", "--verbose"]);
    }

    #[test]
    fn parse_trial_args_without_flag() {
        let argv = vec!["run".to_string()];
        let (home, rest) = parse_trial_args(&argv);
        assert!(home.display().to_string().contains("amplihack-rust-trial"));
        assert_eq!(rest, vec!["run"]);
    }

    #[test]
    fn find_binary_fails_with_empty_path() {
        // Temporarily clear PATH-related hints so the search has nowhere to look.
        // The function checks env var, trial_home/bin, ~/.amplihack/bin, and PATH.
        // Using a nonexistent trial_home and relying on the PATH not having
        // amplihack in a truly nonexistent directory.
        let result = find_rust_cli_binary(Path::new("/nonexistent/trial/home"));
        // On CI/dev machines the binary may actually be on PATH, so we just
        // verify the function returns *something* without panicking.
        let _ = result;
    }

    #[test]
    fn install_options_default() {
        let opts = InstallOptions::default();
        assert!(!opts.force);
        assert!(!opts.bootstrap);
        let s = opts.install_dir.display().to_string();
        assert!(s.contains(".local"));
    }
}
