//! `amplihack pr watch-and-merge` — poll CI checks and merge when green.
//!
//! Watches a GitHub pull request's status checks at a configurable interval.
//! When all checks pass, merges using the specified strategy. On check failure,
//! reports the failing job name and URL and exits with code 1.
//!
//! Uses `gh` CLI for all GitHub operations — never touches `GITHUB_TOKEN` directly.

#[cfg(test)]
mod tests;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Deserialize;
use std::thread;
use std::time::Duration;

// ── Public types ────────────────────────────────────────────────────

/// `amplihack pr <subcommand>`
#[derive(Subcommand, Debug)]
pub enum PrCommands {
    /// Watch CI checks on a PR and merge when all pass.
    WatchAndMerge(WatchAndMergeArgs),
}

/// Arguments for `amplihack pr watch-and-merge`.
#[derive(clap::Args, Debug, Clone)]
pub struct WatchAndMergeArgs {
    /// Pull request number.
    pub pr_number: u32,

    /// Merge strategy: squash (default), rebase, or merge.
    #[arg(long, default_value = "squash", value_parser = ["squash", "rebase", "merge"])]
    pub strategy: String,

    /// Use administrator privileges to merge (bypass branch protections).
    #[arg(long)]
    pub admin: bool,

    /// Delete the remote branch after merging.
    #[arg(long = "delete-branch")]
    pub delete_branch: bool,

    /// Polling interval in seconds (minimum 5).
    #[arg(long, default_value_t = 30)]
    pub interval: u32,
}

// ── GhRunner trait (module-private for testability) ─────────────────

/// Output from a `gh` CLI invocation.
#[derive(Debug, Clone)]
pub struct GhOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Abstraction over `gh` CLI invocations. `RealGhRunner` calls the real binary;
/// `MockGhRunner` (in tests) returns canned responses.
pub trait GhRunner {
    fn run_gh(&self, args: &[&str]) -> Result<GhOutput>;
}

/// Production runner — calls `gh` via `std::process::Command`.
pub struct RealGhRunner;

impl GhRunner for RealGhRunner {
    fn run_gh(&self, args: &[&str]) -> Result<GhOutput> {
        let output = std::process::Command::new("gh")
            .args(args)
            .output()
            .context("failed to execute `gh` — is it installed?")?;
        Ok(GhOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }
}

// ── Check parsing ───────────────────────────────────────────────────

/// Parsed result of `gh pr checks`.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckState {
    Pass,
    Fail,
    Pending,
}

/// A single CI check entry.
#[derive(Debug, Clone)]
pub struct CheckEntry {
    pub name: String,
    pub state: CheckState,
    pub detail_url: String,
}

/// Aggregated check results for a PR.
#[derive(Debug, Clone)]
pub struct ChecksResult {
    pub entries: Vec<CheckEntry>,
    pub all_passed: bool,
    pub has_failures: bool,
    pub has_pending: bool,
}

/// A failed check with name and URL for error reporting.
#[derive(Debug, Clone)]
pub struct CheckFailure {
    pub name: String,
    pub detail_url: String,
}

/// Raw JSON shape from `gh pr checks --json name,state,detailsUrl`.
#[derive(Deserialize)]
struct RawCheck {
    name: String,
    state: String,
    #[serde(rename = "detailsUrl")]
    details_url: String,
}

/// Map a GitHub check state string to our `CheckState` enum.
fn map_check_state(state: &str) -> CheckState {
    match state {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckState::Pass,
        "FAILURE" | "ERROR" => CheckState::Fail,
        // PENDING, QUEUED, IN_PROGRESS, or any unknown state
        _ => CheckState::Pending,
    }
}

/// Parse JSON output from `gh pr checks --json name,state,detailsUrl`.
pub fn parse_checks_json(json_str: &str) -> Result<ChecksResult> {
    let raw: Vec<RawCheck> =
        serde_json::from_str(json_str).context("failed to parse checks JSON from gh CLI")?;

    let entries: Vec<CheckEntry> = raw
        .into_iter()
        .map(|r| CheckEntry {
            state: map_check_state(&r.state),
            name: r.name,
            detail_url: r.details_url,
        })
        .collect();

    let has_failures = entries.iter().any(|e| e.state == CheckState::Fail);
    let has_pending = entries.iter().any(|e| e.state == CheckState::Pending);
    let all_passed = !has_failures && !has_pending;

    Ok(ChecksResult {
        entries,
        all_passed,
        has_failures,
        has_pending,
    })
}

// ── Core logic ──────────────────────────────────────────────────────

/// Minimum allowed polling interval (seconds).
pub const MIN_INTERVAL_SECS: u32 = 5;

/// Maximum transient-failure retries for a single `gh` invocation.
pub const MAX_RETRIES: u32 = 3;

/// Backoff delays for transient retries (seconds): 5, 15, 45.
#[cfg(not(test))]
const BACKOFF_SECS: [u64; 3] = [5, 15, 45];

/// In tests, use zero-length backoff so tests run instantly.
#[cfg(test)]
const BACKOFF_SECS: [u64; 3] = [0, 0, 0];

/// Run `gh` with retries on transient failures (non-check failures).
/// A transient failure is when `runner.run_gh()` returns `Err` (the command
/// itself failed to execute, e.g. network timeout). A non-transient failure
/// is when `gh` returns a non-zero exit code (returned as `Ok(GhOutput)` with
/// `success == false`) — these are NOT retried.
pub fn run_gh_with_retry(runner: &dyn GhRunner, args: &[&str]) -> Result<GhOutput> {
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match runner.run_gh(args) {
            Ok(output) => return Ok(output),
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < MAX_RETRIES {
                    let delay = BACKOFF_SECS[attempt as usize];
                    if delay > 0 {
                        thread::sleep(Duration::from_secs(delay));
                    }
                }
            }
        }
    }
    Err(last_err
        .unwrap()
        .context(format!("gh command failed after {} retries", MAX_RETRIES)))
}

/// Build the `gh pr merge` argument list from the provided args.
pub fn build_merge_args(args: &WatchAndMergeArgs) -> Vec<String> {
    let mut merge_args = vec![
        "pr".to_string(),
        "merge".to_string(),
        args.pr_number.to_string(),
        format!("--{}", args.strategy),
    ];
    if args.admin {
        merge_args.push("--admin".to_string());
    }
    if args.delete_branch {
        merge_args.push("--delete-branch".to_string());
    }
    merge_args
}

/// Poll checks and merge when all pass. Returns Ok(()) on successful merge,
/// or Err with check failure details.
pub fn poll_and_merge(
    runner: &dyn GhRunner,
    args: &WatchAndMergeArgs,
    stderr_writer: &mut dyn std::io::Write,
) -> Result<()> {
    let pr_num = args.pr_number.to_string();
    let check_args = ["pr", "checks", &pr_num, "--json", "name,state,detailsUrl"];

    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let _ = writeln!(
            stderr_writer,
            "⏳ Waiting for checks on PR #{}... (attempt {})",
            args.pr_number, attempt
        );

        let output = run_gh_with_retry(runner, &check_args).context("failed to fetch PR checks")?;

        if !output.success {
            bail!("gh pr checks returned an error: {}", output.stderr.trim());
        }

        let checks = parse_checks_json(&output.stdout)?;

        if checks.entries.is_empty() {
            let _ = writeln!(
                stderr_writer,
                "⚠️  No checks found for PR #{}. Proceeding to merge.",
                args.pr_number
            );
        }

        if checks.has_failures {
            let failures: Vec<CheckFailure> = checks
                .entries
                .iter()
                .filter(|e| e.state == CheckState::Fail)
                .map(|e| CheckFailure {
                    name: e.name.clone(),
                    detail_url: e.detail_url.clone(),
                })
                .collect();

            let detail = failures
                .iter()
                .map(|f| format!("  - {} ({})", f.name, f.detail_url))
                .collect::<Vec<_>>()
                .join("\n");

            bail!("CI checks failed on PR #{}:\n{}", args.pr_number, detail);
        }

        if checks.all_passed || checks.entries.is_empty() {
            let _ = writeln!(
                stderr_writer,
                "✅ All checks passed. Merging PR #{}...",
                args.pr_number
            );

            let merge_args = build_merge_args(args);
            let merge_refs: Vec<&str> = merge_args.iter().map(|s| s.as_str()).collect();
            let merge_output =
                run_gh_with_retry(runner, &merge_refs).context("failed to invoke gh pr merge")?;

            if !merge_output.success {
                bail!(
                    "merge failed for PR #{}: {}",
                    args.pr_number,
                    merge_output.stderr.trim()
                );
            }

            let _ = writeln!(
                stderr_writer,
                "🎉 PR #{} merged successfully.",
                args.pr_number
            );
            return Ok(());
        }

        // Checks are still pending — wait and retry.
        #[cfg(not(test))]
        thread::sleep(Duration::from_secs(u64::from(args.interval)));
    }
}

/// Validate args and enforce constraints (e.g., minimum interval).
pub fn validate_args(args: &WatchAndMergeArgs) -> Result<()> {
    if args.interval < MIN_INTERVAL_SECS {
        bail!(
            "polling interval must be at least {} seconds, got {}",
            MIN_INTERVAL_SECS,
            args.interval
        );
    }
    Ok(())
}

/// Entry point for `amplihack pr` dispatch.
pub fn run(command: PrCommands) -> Result<()> {
    match command {
        PrCommands::WatchAndMerge(args) => {
            validate_args(&args)?;

            // Preflight: check `gh auth status`
            let runner = RealGhRunner;
            let auth = runner.run_gh(&["auth", "status"])?;
            if !auth.success {
                bail!(
                    "GitHub CLI is not authenticated. Run `gh auth login` first.\n{}",
                    auth.stderr.trim()
                );
            }

            let mut stderr = std::io::stderr();
            poll_and_merge(&runner, &args, &mut stderr)
        }
    }
}
