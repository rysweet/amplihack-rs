//! Best-effort mermaid CLI (`mmdc`) install phase (issue #828).
//!
//! The pr-guide skill prefers rendering mermaid diagrams to images with the
//! local mermaid CLI (`mmdc`, npm package `@mermaid-js/mermaid-cli`) instead of
//! the third-party `mermaid.ink` service — see
//! `amplifier-bundle/skills/pr-guide/reference.md`, section
//! "Mermaid on Azure DevOps". To make that local path available out of the box,
//! `amplihack install` provisions `mmdc` on a best-effort basis.
//!
//! **Optional / best-effort by design.** Unlike `recipe-runner-rs` (a REQUIRED
//! component that bails when missing), `mmdc` is OPTIONAL: it pulls in puppeteer
//! plus a headless Chromium download (hundreds of MB) and requires Node/npm,
//! which may be absent or restricted in many environments. Per the
//! install-completeness invariant in `amplifier-bundle/context/PHILOSOPHY.md`,
//! install must fail loudly only for REQUIRED components. A failed `mmdc`
//! install therefore emits a warning and continues — it never aborts the
//! install. [`ensure_mermaid_cli`] consequently returns an [`Outcome`] (never a
//! `Result`): there is no error path to propagate.
//!
//! Behavior:
//! 1. Opt-out: if `AMPLIHACK_SKIP_MMDC` is set to any non-empty value, skip
//!    entirely (minimal/offline environments).
//! 2. Preflight: if `mmdc --version` succeeds, it is already installed — skip.
//! 3. Preflight: if `npm --version` fails, npm is unavailable — skip with an
//!    informative message (never an error).
//! 4. Install: run `npm install -g @mermaid-js/mermaid-cli`, then re-probe.
//!    Any failure (non-zero exit, spawn error, or a success that still leaves
//!    `mmdc` off PATH) warns and continues.

use std::process::{Command, Stdio};

/// User-facing failure message shown when the optional `mmdc` install does not
/// complete. pr-guide still works — it falls back to `mermaid.ink`.
const FALLBACK_NOTICE: &str =
    "mermaid CLI not installed; pr-guide will fall back to mermaid.ink for Azure DevOps diagrams";

/// What [`ensure_mermaid_cli`] did, so callers (and tests) can reason about the
/// branch taken without inspecting stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// `AMPLIHACK_SKIP_MMDC` was set — the whole phase was skipped.
    SkippedByEnv,
    /// `mmdc --version` already succeeded — nothing to do.
    AlreadyPresent,
    /// `npm` was not available, so no install was attempted.
    SkippedNpmAbsent,
    /// `npm install -g @mermaid-js/mermaid-cli` succeeded and `mmdc` is now
    /// reachable on PATH.
    Installed,
    /// An install attempt was made but did not leave a working `mmdc` (non-zero
    /// exit, spawn error, or success without a PATH-reachable binary). Warned
    /// and continued — never fatal.
    InstallFailed,
}

/// Ensure the mermaid CLI (`mmdc`) is available, best-effort. Prints
/// user-facing status lines and **always** returns — there is no failure path
/// that propagates to the caller.
pub(super) fn ensure_mermaid_cli() -> Outcome {
    // 1. Opt-out for minimal/offline environments. Truthy if set to any
    //    non-empty value (matches the `recipe_runner.rs` env convention; the
    //    `=1` form in the docs is illustrative).
    if env_opt_out() {
        println!("  ℹ AMPLIHACK_SKIP_MMDC set; skipping mermaid CLI install");
        return Outcome::SkippedByEnv;
    }

    // 2. Already installed? `mmdc --version` is a fast PATH probe.
    if mermaid_cli_present() {
        println!("  ✓ mermaid CLI (mmdc) already installed; skipping");
        return Outcome::AlreadyPresent;
    }

    // 3. npm available? Without it there is nothing we can do — skip, do not
    //    error.
    if !npm_present() {
        println!(
            "  ℹ npm not available; skipping mermaid CLI install \
             (pr-guide will fall back to mermaid.ink)"
        );
        return Outcome::SkippedNpmAbsent;
    }

    // 4. Attempt the best-effort global install. No hard timeout: the Chromium
    //    download is legitimately slow; we rely on npm's own network timeouts.
    println!("Installing mermaid CLI for local diagram rendering...");
    match run_npm_install() {
        Ok(true) => {
            // npm reported success — re-probe in case the global-prefix binary
            // landed off PATH (mirrors recipe_runner's ~/.cargo/bin caveat).
            if mermaid_cli_present() {
                println!("  ✓ mermaid CLI (mmdc) installed");
                Outcome::Installed
            } else {
                warn_and_continue(
                    "npm install -g @mermaid-js/mermaid-cli reported success but mmdc is \
                     not reachable on PATH",
                );
                Outcome::InstallFailed
            }
        }
        Ok(false) => {
            warn_and_continue("npm install -g @mermaid-js/mermaid-cli exited non-zero");
            Outcome::InstallFailed
        }
        Err(err) => {
            warn_and_continue(&format!("failed to run npm install for mermaid CLI: {err}"));
            Outcome::InstallFailed
        }
    }
}

/// Emit the warn-and-continue pair: a structured `tracing::warn!` carrying the
/// specific failure detail, plus the locked user-facing fallback notice.
fn warn_and_continue(detail: &str) {
    tracing::warn!("{detail}");
    eprintln!("  ⚠️  {FALLBACK_NOTICE}");
}

/// `true` when `AMPLIHACK_SKIP_MMDC` is set to any non-empty value.
fn env_opt_out() -> bool {
    std::env::var_os("AMPLIHACK_SKIP_MMDC")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// `true` when `mmdc --version` spawns and exits successfully.
fn mermaid_cli_present() -> bool {
    command_succeeds("mmdc", &["--version"])
}

/// `true` when `npm --version` spawns and exits successfully.
fn npm_present() -> bool {
    command_succeeds("npm", &["--version"])
}

/// Run `npm install -g @mermaid-js/mermaid-cli`, inheriting stdio so the user
/// sees npm's progress. Returns `Ok(true)` on a zero exit, `Ok(false)` on a
/// non-zero exit, and `Err` if the process could not be spawned.
fn run_npm_install() -> std::io::Result<bool> {
    let status = Command::new("npm")
        .args(["install", "-g", "@mermaid-js/mermaid-cli"])
        .status()?;
    Ok(status.success())
}

/// Spawn `program args...` with all stdio nulled and report whether it exited
/// successfully. A spawn error (e.g. binary not on PATH) maps to `false`.
fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
