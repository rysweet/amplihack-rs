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

/// The literal skip-line emitted to stderr when the version-check prompt is
/// suppressed by a subprocess-safe signal (env, argv flag, or non-TTY stdin).
///
/// This wording is part of the public contract for delegated agents that
/// grep for it as evidence the check ran-and-was-bypassed (vs. silently
/// hung). Rewording is a breaking change to that contract.
pub(super) const SUBPROCESS_SAFE_SKIP_LINE: &str =
    "amplihack: skipping update check (subprocess-safe / no TTY)";

/// The literal `--subprocess-safe` clap long flag, scanned for in argv before
/// clap parsing so the skip can fire before any update-check side effects.
const SUBPROCESS_SAFE_ARG: &str = "--subprocess-safe";

/// Why the startup update check was skipped, returned by [`classify_skip_reason`].
///
/// The variant determines whether [`maybe_print_update_notice_from_args`]
/// emits the [`SUBPROCESS_SAFE_SKIP_LINE`]:
///
/// * [`SkipReason::SubprocessSafe`] → emit the skip-line (visible bypass).
/// * [`SkipReason::ExplicitOptOut`] → silent (user explicitly opted out).
/// * [`SkipReason::NotLaunch`]      → silent passthrough (non-launch subcommand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    /// Subprocess-safe skip: any of NONINTERACTIVE / AGENT_BINARY (non-empty)
    /// / CI (non-empty) / `--subprocess-safe` argv token. Non-TTY stdin is
    /// classified as SubprocessSafe at the entry point (not here, to keep
    /// `classify_skip_reason` pure on env+args for unit testability).
    SubprocessSafe,
    /// Explicit opt-out: AMPLIHACK_NO_UPDATE_CHECK=1 or AMPLIHACK_PARITY_TEST=1.
    /// User asked for no update activity at all — emit nothing.
    ExplicitOptOut,
    /// Not a recognized launch subcommand — silent passthrough preserves
    /// existing behavior for `amplihack --help`, `amplihack version`, etc.
    NotLaunch,
}

pub fn maybe_print_update_notice_from_args(args: &[OsString]) -> StartupUpdateOutcome {
    if supported_release_target().is_none() {
        return StartupUpdateOutcome::Continue;
    }

    // Pure env+args classification, then promote non-TTY stdin to
    // SubprocessSafe when no other signal fired. (TTY check is applied at
    // the entry point so `classify_skip_reason` stays I/O-free for tests.)
    let reason = classify_skip_reason(args)
        .or_else(|| (!io::stdin().is_terminal()).then_some(SkipReason::SubprocessSafe));

    match reason {
        Some(SkipReason::SubprocessSafe) => {
            // Visible bypass: tell delegated agents the check was suppressed.
            eprintln!("{SUBPROCESS_SAFE_SKIP_LINE}");
            StartupUpdateOutcome::Continue
        }
        Some(SkipReason::ExplicitOptOut | SkipReason::NotLaunch) => StartupUpdateOutcome::Continue,
        None => match maybe_print_update_notice() {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::debug!(?error, "startup update check skipped");
                StartupUpdateOutcome::Continue
            }
        },
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
        crate::commands::install::run_install(None, false, true)
    })?;
    Ok(())
}

fn maybe_print_update_notice() -> Result<StartupUpdateOutcome> {
    // NOTE: AMPLIHACK_NO_UPDATE_CHECK is intentionally NOT re-checked here.
    // The sole caller (`maybe_print_update_notice_from_args`) only invokes
    // this function when `classify_skip_reason` returned `None`, which
    // already guarantees that env var is unset / != "1" (it would have
    // produced `SkipReason::ExplicitOptOut` otherwise). Re-reading would
    // be a redundant syscall + String allocation on the hot startup path.
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
/// Returns `true` (skip the check) when any signal recognized by
/// [`classify_skip_reason`] fires for an argv consisting of the binary name
/// followed by `subcommand`. This includes:
///
/// - `AMPLIHACK_NO_UPDATE_CHECK=1` / `AMPLIHACK_PARITY_TEST=1` (ExplicitOptOut)
/// - `AMPLIHACK_NONINTERACTIVE` / `AMPLIHACK_AGENT_BINARY` / `CI` non-empty
///   (SubprocessSafe)
/// - `subcommand` is not in `{launch, claude, copilot, codex, amplifier}`
///   (NotLaunch)
///
/// This function does NOT check stdin-TTY state; the binary entry point
/// ([`maybe_print_update_notice_from_args`]) handles that as part of its
/// SubprocessSafe classification.
pub fn should_skip_update_check_for_subcommand(subcommand: &str) -> bool {
    let args = [OsString::from("amplihack"), OsString::from(subcommand)];
    classify_skip_reason(&args).is_some()
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn should_skip_update_check(args: &[OsString]) -> bool {
    classify_skip_reason(args).is_some()
}

/// Pure (env + argv) classification of why the startup update check should
/// be skipped, returning `None` when the check should proceed.
///
/// This function performs no I/O beyond reading process env vars and is the
/// canonical place to add new SubprocessSafe / ExplicitOptOut signals.
/// Callers wishing to additionally treat non-TTY stdin as SubprocessSafe
/// (the default at the binary entry point) MUST do that check themselves
/// — it is intentionally NOT inside this function so unit tests can exercise
/// classification deterministically without a fake-TTY harness.
///
/// Precedence (first match wins):
///   1. ExplicitOptOut: `AMPLIHACK_NO_UPDATE_CHECK=1` or
///      `AMPLIHACK_PARITY_TEST=1`. Silent — never emits the skip-line.
///   2. NotLaunch: `args[1]` is not in
///      `{launch, claude, copilot, codex, amplifier}`. Silent — the check
///      would never have fired for this subcommand, so there is nothing to
///      announce as "suppressed". This MUST take precedence over the
///      SubprocessSafe checks below: per spec, the skip-line is only emitted
///      when the invocation WOULD have triggered the check (recognized launch
///      subcommand) but a subprocess-safe signal suppressed it.
///   3. SubprocessSafe (env): `AMPLIHACK_NONINTERACTIVE` non-empty,
///      `AMPLIHACK_AGENT_BINARY` non-empty, or `CI` non-empty.
///   4. SubprocessSafe (argv): linear OsStr-equality scan of `args[1..]`
///      for the literal `--subprocess-safe` long flag.
///
/// `None` only when a recognized launch subcommand is invoked AND no other
/// signal fires.
pub(super) fn classify_skip_reason(args: &[OsString]) -> Option<SkipReason> {
    // (1) Explicit opt-out — silent.
    //
    // Use `var_os` (no UTF-8 validation, no String alloc on miss) and a
    // single OsStr equality check. Hot path: this fires for every CLI
    // invocation, and the typical case (env unset) avoids any allocation.
    if env_eq_str(NO_UPDATE_CHECK_ENV, "1") || env_eq_str("AMPLIHACK_PARITY_TEST", "1") {
        return Some(SkipReason::ExplicitOptOut);
    }

    // (2) NotLaunch: silent passthrough for non-launch subcommands.
    //
    // This MUST run before the SubprocessSafe checks — see precedence note in
    // the doc-comment above. For e.g. `amplihack --version` running inside an
    // agent subprocess (AMPLIHACK_AGENT_BINARY=copilot), we want pure
    // passthrough with no extra stderr noise, since the update check was
    // never going to fire for `--version` regardless.
    let first_arg = args.get(1).and_then(|arg| arg.to_str());
    if !matches!(
        first_arg,
        Some("launch") | Some("claude") | Some("copilot") | Some("codex") | Some("amplifier")
    ) {
        return Some(SkipReason::NotLaunch);
    }

    // (3) SubprocessSafe via env — emit the skip-line.
    //
    // Each var is treated as an opaque presence signal: any non-empty value
    // triggers skip. Empty string is the documented "no delegation" sentinel
    // for AMPLIHACK_AGENT_BINARY (matches resolve_subprocess_safe semantics
    // in commands/launch/command.rs) and we extend the same rule to CI for
    // consistency.
    if env_non_empty("AMPLIHACK_NONINTERACTIVE") {
        return Some(SkipReason::SubprocessSafe);
    }
    if env_non_empty("AMPLIHACK_AGENT_BINARY") {
        return Some(SkipReason::SubprocessSafe);
    }
    if env_non_empty("CI") {
        return Some(SkipReason::SubprocessSafe);
    }

    // (4) SubprocessSafe via argv: linear OsStr-equality scan for the
    // literal `--subprocess-safe` clap long flag. We scan pre-clap so the
    // skip can fire before any update I/O. Match must be by full equality
    // so e.g. `--subprocess-safe-foo` or `--no-subprocess-safe` do NOT count.
    let needle = std::ffi::OsStr::new(SUBPROCESS_SAFE_ARG);
    if args.iter().skip(1).any(|a| a.as_os_str() == needle) {
        return Some(SkipReason::SubprocessSafe);
    }

    None
}

/// Returns `true` when the named env var is set to a non-empty value.
/// Treats unset and empty-string identically (both → `false`), matching the
/// `resolve_subprocess_safe` convention in `commands/launch/command.rs`.
fn env_non_empty(name: &str) -> bool {
    match std::env::var_os(name) {
        Some(v) => !v.is_empty(),
        None => false,
    }
}

/// Returns `true` when the named env var is set to a value that compares
/// byte-equal to `expected`. Uses `var_os` to avoid the UTF-8 validation +
/// `String` allocation that `std::env::var` performs (relevant on the
/// per-startup hot path of `classify_skip_reason`).
fn env_eq_str(name: &str, expected: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v == std::ffi::OsStr::new(expected))
}

fn print_update_notice(latest: &str) {
    eprintln!(
        "\x1b[33mA newer version of amplihack is available (v{latest}). Run 'amplihack update' to upgrade.\x1b[0m"
    );
}
