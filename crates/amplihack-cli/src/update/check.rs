use super::*;

pub fn maybe_print_update_notice_from_args(args: &[OsString]) {
    if should_skip_update_check(args) || supported_release_target().is_none() {
        return;
    }

    if let Err(error) = maybe_print_update_notice() {
        tracing::debug!(?error, "startup update check skipped");
    }
}

pub fn run_update() -> Result<()> {
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
    Ok(())
}

fn maybe_print_update_notice() -> Result<()> {
    if std::env::var(NO_UPDATE_CHECK_ENV).unwrap_or_default() == "1" {
        return Ok(());
    }

    let cache_path = cache_path()?;
    let now = now_secs();

    if let Some((cached_version, timestamp)) = read_cache(&cache_path)
        && now.saturating_sub(timestamp) < UPDATE_CHECK_COOLDOWN_SECS
    {
        if is_newer(CURRENT_VERSION, &cached_version)? {
            print_update_notice(&cached_version);
        }
        return Ok(());
    }

    let release = super::network::fetch_latest_release()?;
    write_cache(&cache_path, &release.version)?;
    if is_newer(CURRENT_VERSION, &release.version)? {
        print_update_notice(&release.version);
    }
    Ok(())
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
