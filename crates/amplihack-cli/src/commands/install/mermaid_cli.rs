//! Best-effort install of the mermaid CLI (`mmdc`, npm package
//! `@mermaid-js/mermaid-cli`) during `amplihack install` (issue #828).
//!
//! The `pr-guide` skill renders mermaid diagrams to images locally for Azure
//! DevOps (where mermaid does not render in PR descriptions/comments),
//! preferring a local `mmdc` over the third-party `mermaid.ink` service for
//! privacy. `mmdc` pulls in puppeteer + a headless Chromium download
//! (hundreds of MB) and requires Node/npm, which may be absent or restricted
//! in many environments.
//!
//! Per the install-completeness invariant in
//! `amplifier-bundle/context/PHILOSOPHY.md`, `amplihack install` must fail
//! loudly ONLY for REQUIRED components. mmdc is NOT required, so this phase is
//! strictly best-effort: every failure path warns-and-continues, and
//! [`ensure_mermaid_cli`] ALWAYS returns `Ok`. It must never abort the
//! overall install.
//!
//! Behavior:
//! 1. If `AMPLIHACK_SKIP_MMDC` is set (any non-empty value) → `SkippedByEnv`.
//! 2. If `mmdc --version` succeeds (already on PATH) → `AlreadyPresent`.
//! 3. If `npm --version` fails (npm absent) → `SkippedNoNpm` (informative,
//!    not an error).
//! 4. Otherwise run `npm install -g @mermaid-js/mermaid-cli`; on failure
//!    warn-and-continue → `Failed`.
//! 5. Re-probe `mmdc`. Present → `Installed`. Still absent (e.g. npm prefix
//!    not on PATH) → warn-and-continue → `Failed`.

use anyhow::Result;
use std::process::{Command, Stdio};

/// The scoped npm package that provides the `mmdc` binary. Hardcoded exactly;
/// never constructed from env/config/user input (no command-injection surface).
const MERMAID_CLI_PACKAGE: &str = "@mermaid-js/mermaid-cli";

/// User-facing line shown when mmdc could not be provisioned. Shared by the
/// `Failed` and the defense-in-depth `Err(_)` arms in `mod.rs` so the message
/// stays in one place.
pub(super) const FALLBACK_NOTICE: &str = "  ⚠️  mermaid CLI not installed; pr-guide will fall back to mermaid.ink for Azure DevOps diagrams";

/// What [`ensure_mermaid_cli`] did. Lets the caller pretty-print a one-line
/// user-facing status banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// `mmdc` was already discoverable on PATH at probe time → skipped.
    AlreadyPresent,
    /// `npm install -g @mermaid-js/mermaid-cli` succeeded and the re-probe
    /// found `mmdc` on PATH.
    Installed,
    /// `AMPLIHACK_SKIP_MMDC` was set (any non-empty value) → opted out.
    SkippedByEnv,
    /// `npm` was not available → skipped with an informative message.
    SkippedNoNpm,
    /// `npm install` was attempted but failed (network/permission/Chromium
    /// download), or it reported success yet `mmdc` is still not on PATH.
    /// Best-effort: warned and continued.
    Failed,
}

/// Probe a binary by running `<bin> --version`. Returns `true` only if the
/// command spawned and exited successfully. Any spawn error (binary absent on
/// PATH) or non-zero exit yields `false`. We only need the exit status, so the
/// child's stdout/stderr are discarded via `Stdio::null()` — this both keeps
/// the install console quiet and avoids allocating/reading capture pipes.
fn version_probe_succeeds(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Ensure the mermaid CLI (`mmdc`) is available, best-effort. Always returns
/// `Ok` — a missing/broken mmdc must never block `amplihack install`. See the
/// module docs for the full decision flow.
pub(super) fn ensure_mermaid_cli() -> Result<Outcome> {
    // (1) Opt-out: any non-empty value disables the attempt entirely, taking
    // precedence over every probe (for minimal/offline environments).
    let skip = std::env::var_os("AMPLIHACK_SKIP_MMDC")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if skip {
        return Ok(Outcome::SkippedByEnv);
    }

    // (2) Preflight: already installed? Skip without touching npm.
    if version_probe_succeeds("mmdc") {
        return Ok(Outcome::AlreadyPresent);
    }

    // (3) Preflight: npm available? If not, skip (not an error).
    if !version_probe_succeeds("npm") {
        return Ok(Outcome::SkippedNoNpm);
    }

    // (4) Install. Arg-vector form only (no shell): prevents injection. No
    // sudo / no privilege escalation; no hard timeout (the Chromium download
    // is legitimately slow and is bounded by npm's own network timeouts).
    let install_result = Command::new("npm")
        .args(["install", "-g", MERMAID_CLI_PACKAGE])
        .status();

    match install_result {
        Ok(status) if status.success() => {
            // (5) Re-probe: npm may have succeeded but placed mmdc in a prefix
            // that isn't on PATH. Don't claim success unless mmdc is reachable.
            if version_probe_succeeds("mmdc") {
                Ok(Outcome::Installed)
            } else {
                tracing::warn!(
                    "npm install -g {MERMAID_CLI_PACKAGE} reported success but mmdc is \
                     still not on PATH (npm global prefix may not be on PATH)"
                );
                Ok(Outcome::Failed)
            }
        }
        Ok(status) => {
            tracing::warn!(
                code = status.code(),
                "npm install -g {MERMAID_CLI_PACKAGE} exited non-zero"
            );
            Ok(Outcome::Failed)
        }
        Err(err) => {
            tracing::warn!(%err, "failed to spawn npm install -g {MERMAID_CLI_PACKAGE}");
            Ok(Outcome::Failed)
        }
    }
}
