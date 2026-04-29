use super::*;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

/// Outcome of the startup update check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupUpdateOutcome {
    /// Continue with normal CLI execution.
    Continue,
    /// A self-update completed; main should exit immediately.
    ExitSuccess,
}

const STARTUP_UPDATE_PROMPT_TIMEOUT_SECS: u64 = 5;

pub fn maybe_print_update_notice_from_args(args: &[OsString]) -> StartupUpdateOutcome {
    if should_skip_update_check(args) || supported_release_target().is_none() {
        return StartupUpdateOutcome::Continue;
    }

    match maybe_print_update_notice() {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::debug!(?error, "startup update check skipped");
            StartupUpdateOutcome::Continue
        }
    }
}

pub fn run_update(skip_install: bool) -> Result<()> {
    println!("amplihack update (current: v{CURRENT_VERSION})");

    let release = super::network::fetch_latest_release()?;
    if !is_newer(CURRENT_VERSION, &release.version)? {
        println!("Already at the latest version (v{CURRENT_VERSION}).");
        return Ok(());
    }

    println!(
        "New version available: v{} -> v{}",
        CURRENT_VERSION, release.version
    );
    super::install::download_and_replace(&release)?;
    write_cache(&cache_path()?, &release.version)?;

    // Re-stage framework assets after binary swap (fix #249, #487).
    // The new binary may depend on updated assets in amplifier-bundle.
    // Users can opt out with --skip-install (alias --no-install).
    super::post_install::run_post_update_install(skip_install, || {
        crate::commands::install::run_install(None, false)
    })?;
    Ok(())
}

fn maybe_print_update_notice() -> Result<StartupUpdateOutcome> {
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return Ok(StartupUpdateOutcome::Continue);
    }

    let cache_path = cache_path()?;
    let now = now_secs();

    if let Some((cached_version, timestamp)) = read_cache(&cache_path)
        && now.saturating_sub(timestamp) < UPDATE_CHECK_COOLDOWN_SECS
    {
        if is_newer(CURRENT_VERSION, &cached_version)? {
            return maybe_prompt_for_startup_update(&cached_version);
        }
        return Ok(StartupUpdateOutcome::Continue);
    }

    let release = super::network::fetch_latest_release()?;
    write_cache(&cache_path, &release.version)?;
    if is_newer(CURRENT_VERSION, &release.version)? {
        return maybe_prompt_for_startup_update(&release.version);
    }
    Ok(StartupUpdateOutcome::Continue)
}

fn maybe_prompt_for_startup_update(latest: &str) -> Result<StartupUpdateOutcome> {
    print_update_notice(latest);
    let response = read_user_input_with_timeout(
        "Update now? [y/N] (5s timeout): ",
        Duration::from_secs(STARTUP_UPDATE_PROMPT_TIMEOUT_SECS),
    )?;
    if !wants_startup_update(response.as_deref()) {
        return Ok(StartupUpdateOutcome::Continue);
    }

    run_update(false)?;
    println!("✅ Update complete. Re-run amplihack to continue with the new version.");
    Ok(StartupUpdateOutcome::ExitSuccess)
}

fn wants_startup_update(response: Option<&str>) -> bool {
    matches!(
        response
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "y" | "yes"
    )
}

fn read_user_input_with_timeout(prompt: &str, timeout: Duration) -> Result<Option<String>> {
    #[cfg(not(unix))]
    {
        let _ = (prompt, timeout);
        return Ok(None);
    }

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;

        if !io::stdin().is_terminal() {
            return Ok(None);
        }

        print!("{prompt}");
        io::stdout()
            .flush()
            .context("failed to flush update prompt")?;

        let fd = io::stdin().as_raw_fd();
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready < 0 {
            return Err(io::Error::last_os_error()).context("failed waiting for update prompt");
        }
        if ready == 0 {
            println!();
            return Ok(None);
        }

        let mut response = String::new();
        io::stdin()
            .read_line(&mut response)
            .context("failed to read update prompt input")?;
        Ok(Some(response.trim().to_string()))
    }
}

/// Determine whether the update check should be skipped based on the subcommand
/// name string alone (without needing the full args slice).
///
/// Returns `true` (skip the check) when:
/// - `AMPLIHACK_NONINTERACTIVE=1` is set in the environment
/// - `AMPLIHACK_PARITY_TEST=1` is set in the environment
/// - `AMPLIHACK_NO_UPDATE_CHECK=1` is set in the environment
/// - `subcommand` is not one of the known launch commands
///   (`launch`, `claude`, `copilot`, `codex`, `amplifier`)
///
/// This is the string-oriented companion to `should_skip_update_check` and is
/// the function that callers with a parsed subcommand name should use.
///
pub fn should_skip_update_check_for_subcommand(subcommand: &str) -> bool {
    // Explicit opt-out via legacy env var.
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return true;
    }

    // Non-interactive / scripted environments suppress all update checks.
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }

    // Parity test harness — suppress update noise during automated comparison runs.
    if std::env::var("AMPLIHACK_PARITY_TEST").as_deref() == Ok("1") {
        return true;
    }

    // Only show the update notice for known launch commands.
    // All other subcommands skip the update check.
    !matches!(
        subcommand,
        "launch" | "claude" | "copilot" | "codex" | "amplifier"
    )
}

pub(super) fn should_skip_update_check(args: &[OsString]) -> bool {
    // Explicit opt-out via legacy env var.
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return true;
    }

    // Non-interactive / scripted environments (CI, AMPLIHACK_NONINTERACTIVE=1).
    if std::env::var("AMPLIHACK_NONINTERACTIVE").as_deref() == Ok("1") {
        return true;
    }

    // Parity test harness — suppress update noise during automated comparison runs.
    if std::env::var("AMPLIHACK_PARITY_TEST").as_deref() == Ok("1") {
        return true;
    }

    let first_arg = args.get(1).and_then(|arg| arg.to_str());

    // Only show the update notice when the user is about to launch a tool.
    !matches!(
        first_arg,
        Some("launch") | Some("claude") | Some("copilot") | Some("codex") | Some("amplifier")
    )
}

fn print_update_notice(latest: &str) {
    eprintln!(
        "\x1b[33mA newer version of amplihack is available (v{}). Run 'amplihack update' to upgrade.\x1b[0m",
        latest
    );
}
