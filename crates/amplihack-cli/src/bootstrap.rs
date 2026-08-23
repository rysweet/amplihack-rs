//! First-run bootstrap for framework assets and host CLIs.

use crate::binary_finder::{BinaryFinder, BinaryInfo};
use crate::claude_plugin;
use crate::commands::install;
use crate::copilot_setup;
use crate::freshness;
use crate::tool_update_check::{get_latest_version, sanitize_version};
use crate::util::{
    format_output_diagnostics, is_noninteractive, run_output_with_timeout, run_with_timeout,
};
use amplihack_utils::claude_native::{
    CLAUDE_NPM_PACKAGE, claude_platform_packages, detect_musl, is_materialized,
};
use amplihack_utils::launch_target::{self, InstallDecision, LaunchTarget, Resolution};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Timeout for tool installation commands (npm install, uv tool install).
/// These involve network downloads and can be legitimately slow, so we allow
/// 5 minutes before treating them as hung.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

pub fn prepare_launcher(tool: &str) -> Result<()> {
    // SEC-WS2-02: Non-interactive mode (CI, pipes, AMPLIHACK_NONINTERACTIVE=1)
    // skips all interactive setup. The environment is assumed pre-provisioned.
    // This matches Python launcher behavior and prevents hangs in sandboxes.
    if is_noninteractive() {
        tracing::debug!(
            tool,
            "non-interactive mode detected — skipping interactive bootstrap"
        );
        return Ok(());
    }

    check_required_tools()?;
    install::ensure_framework_installed()?;

    // Best-effort: bring the recipe runner up to date with upstream HEAD.
    // Runs on a 24h cooldown and can be disabled via
    // AMPLIHACK_NO_FRESHNESS_CHECK=1 or the standard non-interactive guards.
    // Network failures are logged and swallowed — launch must not depend on
    // reaching GitHub.
    freshness::ensure_recipe_runner_up_to_date();

    match tool {
        "copilot" => {
            // Hard gate: Copilot CLI requires Node.js >= 24.
            // If the system version is insufficient, auto-install a managed
            // copy to ~/.amplihack/runtimes/node/ and prepend it to PATH.
            if let Some(managed_bin_dir) = ensure_node_for_copilot()? {
                prepend_path(&managed_bin_dir)?;
                persist_path_hint(&managed_bin_dir)?;
            }
            copilot_setup::ensure_copilot_home_staged()?;
        }
        "claude" => {
            // Register amplihack as a Claude Code plugin so the agents,
            // skills, and commands staged under ~/.amplihack/.claude/ are
            // discoverable through Claude Code's plugin system. A failure
            // here must not block the launch — hooks are still wired via
            // settings.json even if the plugin registration fails.
            if let Err(err) = claude_plugin::ensure_claude_plugin_installed() {
                tracing::warn!(%err, "failed to register amplihack Claude plugin");
                eprintln!("⚠️  Failed to register amplihack as a Claude Code plugin: {err}");
            }
        }
        "codex" => configure_codex()?,
        _ => {}
    }

    Ok(())
}

/// Check that required system tools are available.
/// Prints warnings for missing tools but only fails for critical ones.
fn check_required_tools() -> Result<()> {
    // tmux is required for recipe runner workflow execution
    if which("tmux").is_none() {
        eprintln!("⚠️  tmux is not installed. Recipe workflow execution requires tmux.");
        eprintln!("   Install it:");
        eprintln!("     macOS:  brew install tmux");
        eprintln!("     Ubuntu: sudo apt install tmux");
        eprintln!("     Fedora: sudo dnf install tmux");
    }
    Ok(())
}

fn which(tool: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(tool);
            if full.is_file() { Some(full) } else { None }
        })
    })
}

/// Ensure Node.js >= 24 is available for Copilot CLI. If the system version
/// is insufficient, downloads a managed copy to `~/.amplihack/runtimes/node/`.
/// Returns `Some(bin_dir)` when a managed install was used, `None` when the
/// system node is sufficient.
fn ensure_node_for_copilot() -> Result<Option<PathBuf>> {
    use amplihack_utils::prerequisites::{
        NODE_AUTO_INSTALL_VERSION, check_node_minimum_version, node_platform_triple,
    };

    const MIN: u32 = 24;

    // Fast path: system node is sufficient.
    if check_node_minimum_version(MIN).is_ok() {
        return Ok(None);
    }

    // Non-interactive environments should not auto-install.
    if is_noninteractive() {
        bail!(
            "Node.js >= v{MIN} is required but not found, and \
             auto-install is disabled in non-interactive mode.\n\
             Install Node.js manually: https://nodejs.org/"
        );
    }

    let (os_name, arch_name) = node_platform_triple().ok_or_else(|| {
        anyhow!(
            "Node.js >= v{MIN} is required but auto-install is not supported \
             on this platform.\nInstall Node.js manually: https://nodejs.org/"
        )
    })?;

    let runtimes_dir = home_dir()?.join(".amplihack").join("runtimes");
    let dir_name = format!("node-{NODE_AUTO_INSTALL_VERSION}-{os_name}-{arch_name}");
    let install_dir = runtimes_dir.join(&dir_name);
    let bin_dir = install_dir.join("bin");

    // Already installed?
    if bin_dir.join("node").exists() {
        tracing::info!(path = %bin_dir.display(), "managed Node.js already present");
        println!("  ✅ Managed Node.js {NODE_AUTO_INSTALL_VERSION} already installed");
        return Ok(Some(bin_dir));
    }

    let ext = "tar.xz";
    let filename = format!("node-{NODE_AUTO_INSTALL_VERSION}-{os_name}-{arch_name}.{ext}");
    let url = format!("https://nodejs.org/dist/{NODE_AUTO_INSTALL_VERSION}/{filename}");
    let checksum_filename = "SHASUMS256.txt";
    let checksum_url =
        format!("https://nodejs.org/dist/{NODE_AUTO_INSTALL_VERSION}/{checksum_filename}");

    println!("  ⬇️  Downloading Node.js {NODE_AUTO_INSTALL_VERSION} ({os_name}-{arch_name})...");
    tracing::info!(%url, "downloading Node.js");

    fs::create_dir_all(&runtimes_dir)
        .with_context(|| format!("failed to create {}", runtimes_dir.display()))?;

    let tmp_path = runtimes_dir.join(&filename);
    let checksum_path = runtimes_dir.join(format!("{filename}.{checksum_filename}"));

    if let Err(err) = download_with_curl(&url, &tmp_path, "Node.js archive") {
        let _ = fs::remove_file(&tmp_path);
        bail!("{err:#}\nInstall Node.js manually: https://nodejs.org/");
    }
    if let Err(err) = download_with_curl(&checksum_url, &checksum_path, "Node.js checksum manifest")
    {
        let _ = fs::remove_file(&tmp_path);
        let _ = fs::remove_file(&checksum_path);
        bail!("{err:#}\nInstall Node.js manually: https://nodejs.org/");
    }
    if let Err(err) = verify_node_archive_sha256(&tmp_path, &checksum_path, &filename) {
        let _ = fs::remove_file(&tmp_path);
        let _ = fs::remove_file(&checksum_path);
        bail!("{err:#}\nInstall Node.js manually: https://nodejs.org/");
    }
    let _ = fs::remove_file(&checksum_path);

    println!("  📦 Installing Node.js {NODE_AUTO_INSTALL_VERSION}...");

    // Extract to a temp directory, then atomically rename to install_dir.
    // This prevents partial extraction (disk full, interrupted) from leaving
    // a broken install that the next run would accept as valid.
    let temp_dir = runtimes_dir.join(format!("{dir_name}.extracting"));

    // Clean up any stale temp dir from a prior crash
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create temp dir {}", temp_dir.display()))?;

    let mut extract_cmd = Command::new("tar");
    extract_cmd
        .args(["--strip-components=1", "-xJf"])
        .arg(&tmp_path)
        .arg("-C")
        .arg(&temp_dir);
    let extract_status =
        run_with_timeout(extract_cmd, INSTALL_TIMEOUT).context("failed to run tar")?;

    let _ = fs::remove_file(&tmp_path);

    if !extract_status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        bail!(
            "failed to extract Node.js tarball (exit {})",
            extract_status.code().unwrap_or(-1)
        );
    }

    if !temp_dir.join("bin").join("node").exists() {
        let _ = fs::remove_dir_all(&temp_dir);
        bail!(
            "Node.js extraction succeeded but bin/node not found in {}",
            temp_dir.display()
        );
    }

    fs::rename(&temp_dir, &install_dir).with_context(|| {
        let _ = fs::remove_dir_all(&temp_dir);
        format!(
            "failed to rename {} to {}",
            temp_dir.display(),
            install_dir.display()
        )
    })?;

    println!(
        "  ✅ Node.js {NODE_AUTO_INSTALL_VERSION} installed to {}",
        install_dir.display()
    );
    Ok(Some(bin_dir))
}

fn download_with_curl(url: &str, destination: &Path, label: &str) -> Result<()> {
    let mut cmd = Command::new("curl");
    cmd.args(["-fsSL", "-o"]).arg(destination).arg(url);
    let output = run_output_with_timeout(cmd, DOWNLOAD_TIMEOUT)
        .with_context(|| format!("{label} download timed out or failed to execute: {url}"))?;
    if !output.status.success() {
        bail!(
            "{label} download failed from {url}: {}",
            format_output_diagnostics(&output, 400)
        );
    }
    Ok(())
}

fn verify_node_archive_sha256(
    archive_path: &Path,
    checksum_path: &Path,
    archive_filename: &str,
) -> Result<()> {
    let manifest = fs::read_to_string(checksum_path).with_context(|| {
        format!(
            "failed to read Node.js checksum manifest {}",
            checksum_path.display()
        )
    })?;
    let expected = find_sha256_for_archive(&manifest, archive_filename)?;
    let mut archive = fs::File::open(archive_path)
        .with_context(|| format!("failed to read Node.js archive {}", archive_path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut archive, &mut hasher)
        .with_context(|| format!("failed to hash Node.js archive {}", archive_path.display()))?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        bail!(
            "Node.js archive SHA-256 verification failed for {archive_filename}: expected {expected}, got {actual}"
        );
    }
    Ok(())
}

fn find_sha256_for_archive(manifest: &str, archive_filename: &str) -> Result<String> {
    let mut matches = manifest.lines().filter_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?;
        let filename = parts.next()?;
        (filename == archive_filename).then(|| digest.to_ascii_lowercase())
    });
    let digest = matches
        .next()
        .ok_or_else(|| anyhow!("Node.js checksum manifest does not list {archive_filename}"))?;
    if matches.next().is_some() {
        bail!("Node.js checksum manifest lists {archive_filename} more than once");
    }
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Node.js checksum manifest has an invalid SHA-256 digest for {archive_filename}");
    }
    Ok(digest)
}

pub fn ensure_tool_available(tool: &str) -> Result<BinaryInfo> {
    // Issue #1266, Defect 2. Check, install, and exec all resolve through
    // `launch_target::resolve`. Before this, three separate resolutions
    // disagreed inside a single launch: the version check read `npm list -g`
    // under npm's ambient prefix, the install wrote `~/.npm-global`, and the
    // exec ran whatever `$PATH` produced. So amplihack "upgraded" a binary it
    // was never going to run, every single launch, forever.
    let resolution = launch_target::resolve(tool);
    let package = npm_package_for_install(tool);
    let latest = latest_published_version(package, resolution.target.as_ref());
    // The single directory an install writes. `decide_install` needs it to tell
    // a broken override it can repair from one it can only waste an install on.
    let amplihack_bin = home_dir()
        .ok()
        .map(|h| launch_target::amplihack_prefix_bin(&h));
    let decision = launch_target::decide_install(
        tool,
        &resolution,
        latest.as_deref(),
        amplihack_bin.as_deref(),
    );

    // One `Resolution` flows through every arm, so the failure message below
    // reports the candidates that were actually tried last — never a fresh
    // probe of the host that re-runs the whole gate to say the same thing.
    let resolution = match decision {
        InstallDecision::UseExisting => resolution,
        InstallDecision::Abstain => {
            // Nothing healthy resolved, but the evidence is inconclusive rather
            // than absent: a candidate timed out, or resolution stopped at the
            // probe cap or the total budget before examining every candidate.
            // Installing ~339 MB over a host that is merely under load is the
            // same mistake as reinstalling because a registry query failed, and
            // the rule is the same: inconclusive means stop. The message names
            // BOTH budgets because `Abstain` has two causes and quoting only
            // the per-candidate one sends the reader looking at the wrong
            // number.
            log_rejected_candidates(tool, &resolution);
            bail!(
                "Could not verify a working '{tool}' — resolution ended without a \
                 conclusive answer: a candidate did not respond within {per:?}, or \
                 the {total:?} total probe budget ran out before every candidate was \
                 examined. That usually means this host is under load rather than \
                 that '{tool}' is missing.\n\n{report}\n\
                 Re-run amplihack, or point it at a known-good binary:\n  \
                 export {tool_upper}_BINARY_PATH=/path/to/{tool}",
                per = launch_target::PER_CANDIDATE_PROBE_TIMEOUT,
                total = launch_target::TOTAL_PROBE_BUDGET,
                report = resolution.rejection_report(tool, package.unwrap_or(tool)),
                tool_upper = tool.to_uppercase(),
            );
        }
        InstallDecision::BrokenOverride => {
            // The user named a binary, it is broken, and it lives somewhere
            // amplihack does not write. Installing would resolve to the same
            // broken path and fail identically — so say what is wrong with the
            // file they actually named instead of spending ~339 MB first.
            log_rejected_candidates(tool, &resolution);
            let named = resolution
                .halted_on_user_override
                .as_deref()
                .map(launch_target::display_untrusted_path)
                .unwrap_or_default();
            bail!(
                "'{tool}' was resolved from {tool_upper}_BINARY_PATH, and that binary \
                 is not usable:\n\n{report}\n\
                 amplihack installs into {prefix}, so installing cannot repair \
                 {named} — it is not a file amplihack writes.\n\
                 Point {tool_upper}_BINARY_PATH at a working '{tool}', or unset it to \
                 let amplihack resolve one:\n  \
                 unset {tool_upper}_BINARY_PATH",
                report = resolution.rejection_report(tool, package.unwrap_or(tool)),
                prefix = amplihack_bin
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.npm-global/bin".to_string()),
                tool_upper = tool.to_uppercase(),
            );
        }
        InstallDecision::UpgradeOwned => {
            if let (Some(target), Some(pkg), Some(latest)) =
                (resolution.target.as_ref(), package, latest.as_deref())
            {
                println!("📦 Upgrading {tool} ({pkg}): {} → {latest}", target.version);
            }
            reinstall_and_reresolve(tool)
        }
        InstallDecision::InstallMissing => {
            log_rejected_candidates(tool, &resolution);
            install_tool(tool)?;
            // Uncached: the install just changed the filesystem, which is the
            // one thing the resolution memo cannot see.
            launch_target::resolve_uncached(tool)
        }
    };

    let Some(target) = resolution.target.as_ref() else {
        // Defect 3: name the real cause. The old failure was
        // `failed to spawn child process: Exec format error (os error 8)`,
        // which named nothing real. Whatever went wrong, the user gets the
        // list of what was tried, why each candidate was rejected, and a
        // command to run.
        let prefix_hint = npm_prefix_dir()
            .map(|p| p.join("bin").display().to_string())
            .unwrap_or_else(|_| "~/.npm-global/bin".to_string());
        bail!(
            "{report}\n\
             If '{tool}' is installed somewhere amplihack did not look, add it to \
             your PATH:\n  \
             export PATH=\"{prefix_hint}:$PATH\"\n\
             Or install it into amplihack's own prefix:\n  \
             npm install -g --prefix {prefix_hint} {pkg}",
            report = resolution.rejection_report(tool, package.unwrap_or(tool)),
            tool = tool,
            prefix_hint = prefix_hint,
            pkg = package.unwrap_or(tool),
        );
    };

    Ok(BinaryInfo {
        name: tool.to_string(),
        path: target.path.clone(),
        version: Some(target.version.clone()),
    })
}

/// Query the registry for the newest published version, but only when the
/// answer could change anything.
///
/// Skipped entirely when the answer cannot change the decision:
///
/// * nothing healthy resolved — `decide_install` answers `InstallMissing` or
///   `Abstain` without ever reading `latest`;
/// * the target is one amplihack does not own — it answers `UseExisting`
///   regardless.
///
/// Either way there is no reason to spend a network round trip on a decision
/// that is already made.
fn latest_published_version(
    package: Option<&'static str>,
    target: Option<&LaunchTarget>,
) -> Option<String> {
    if is_noninteractive() {
        return None;
    }
    let package = package?;
    let target = target?;
    if target.source != launch_target::TargetSource::AmplihackPrefix {
        return None;
    }
    let latest = sanitize_version(&get_latest_version(package)?);
    // An empty string is "unknown", and `decide_install` must see `None` for
    // that: a failed registry query never triggers a reinstall.
    (!latest.is_empty()).then_some(latest)
}

/// Reinstall a tool amplihack owns, then re-resolve — and answer with what the
/// filesystem says *now*, whether or not the install succeeded.
///
/// The obvious version of this function keeps the pre-upgrade `Resolution` as a
/// fallback, so "a failed upgrade keeps the existing healthy binary rather than
/// failing the launch". That reads as conservative and is the opposite, because
/// the upgrade is usually what stopped the binary being healthy:
/// `install_npm_package` runs with `--ignore-scripts`, which leaves the
/// ~500-byte placeholder at `bin/claude.exe`, and `materialize_claude_native`
/// is documented non-fatal — so it can warn and return with the placeholder
/// standing. The old target's *path* is then still correct and its *version* is
/// a memory of a file that no longer exists there. Returning it walks a
/// `LaunchTarget` the health gate has just rejected straight into
/// `Command::new`, which is `Exec format error` — issue #1266's exact symptom,
/// on the upgrade path. The failed-install arm was worse still: the retry in
/// `install_npm_package` calls `remove_package_install_dir` first, so `previous`
/// could name a path that has been deleted.
///
/// `LaunchTarget`'s contract is that health is a filter and never an
/// annotation. A fallback that skips the filter is not allowed to exist here,
/// so there isn't one: re-resolve uncached and return that. A genuinely
/// untouched healthy binary still resolves and still launches; one the upgrade
/// broke is reported by the caller's no-target path, which already prints the
/// full rejection report.
fn reinstall_and_reresolve(tool: &str) -> Resolution {
    if let Err(err) = install_tool(tool) {
        tracing::warn!(%err, tool, "tool upgrade failed; re-resolving what is on disk");
    }
    // Uncached for the same reason as the InstallMissing arm: the install just
    // changed what is on disk, which is the one thing the memo cannot see.
    launch_target::resolve_uncached(tool)
}

/// Record why nothing healthy was found before spending an install on it.
///
/// The candidate paths here have exactly the provenance
/// [`launch_target::display_untrusted_path`] exists for — `$PATH`, `$HOME`,
/// `*_BINARY_PATH`, or a filename someone planted in a directory already on
/// `$PATH` — so they are rendered through it rather than with `Display`.
///
/// A `tracing` field is not exempt from that rule. The default subscriber is a
/// human `fmt` layer writing to **stderr**, and a `%`-sigil field reaches it as
/// raw bytes: a planted name carrying `\n` forges a second log line in
/// amplihack's own voice, and OSC 52 writes the user's clipboard. The default
/// `EnvFilter` level keeps `info` off unless `RUST_LOG` is set, which narrows
/// the window but does not close it — and `RUST_LOG=info` is precisely the
/// situation these lines exist to be read in.
///
/// Same sanitiser as `Resolution::rejection_report` and `enrich_spawn_error`,
/// deliberately: one class of untrusted data, one sanitiser. It truncates at
/// the first control character instead of stripping escapes, because the tail
/// is the payload.
fn log_rejected_candidates(tool: &str, resolution: &Resolution) {
    for (path, rejection) in &resolution.rejected {
        tracing::info!(
            tool,
            path = %launch_target::display_untrusted_path(path),
            reason = rejection.explain(),
            "candidate rejected before install"
        );
    }
}

/// Map a tool name to the npm package used for installation and upgrades.
///
/// This is the single source of truth — both `install_tool` and
/// `maybe_upgrade_tool` read through here so they can never disagree on
/// which package backs a given tool.
pub(crate) fn npm_package_for_install(tool: &str) -> Option<&'static str> {
    match tool {
        "claude" => Some("@anthropic-ai/claude-code"),
        "copilot" => Some("@github/copilot"),
        "codex" => Some("@openai/codex"),
        _ => None,
    }
}

fn install_tool(tool: &str) -> Result<()> {
    if let Some(pkg) = npm_package_for_install(tool) {
        return install_npm_package(tool, pkg);
    }
    match tool {
        "amplifier" => install_amplifier(),
        other => bail!("automatic installation is not implemented for '{other}'"),
    }
}

fn install_npm_package(tool: &str, package: &str) -> Result<()> {
    let npm = BinaryFinder::find("npm")
        .context("npm is required to install Node-based host CLIs")?
        .path;

    let prefix = npm_prefix_dir()?;
    let bin_dir = prefix.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;

    prepend_path(&bin_dir)?;
    println!("📦 Installing {tool} via npm package {package}...");

    // Clean any stale temp dirs npm left behind from a prior failed install
    // (e.g. `@github/.copilot-YYsO5Mpa`). Left in place, these cause
    // `ENOTEMPTY: directory not empty, rename ...` on every subsequent install.
    clean_stale_npm_temp_dirs(&prefix, package);

    match run_npm_install(&npm, &prefix, package) {
        Ok(()) => {}
        Err(err) => {
            // Last-ditch: clean again and retry once. npm's own rename can fail
            // if a concurrent install (or even the first part of this one) raced.
            tracing::warn!(%err, "npm install failed; cleaning stale temp dirs and retrying once");
            clean_stale_npm_temp_dirs(&prefix, package);
            remove_package_install_dir(&prefix, package);
            run_npm_install(&npm, &prefix, package)?;
        }
    }

    // Issue #585: After installing @github/copilot with --omit=optional,
    // install the platform-specific native binary package separately.
    // This avoids the npm reify hang caused by cross-platform optional deps
    // while still getting the correct native binary for the current platform.
    if package == "@github/copilot" {
        let (os_name, arch) = current_platform();
        if let Some(platform_pkg) = copilot_platform_package(os_name, arch) {
            println!("📦 Installing platform binary {platform_pkg}...");
            if let Err(err) = run_npm_install(&npm, &prefix, platform_pkg) {
                // Non-fatal: Node.js may have a JS fallback via index.js on
                // sufficiently recent versions. Warn but don't fail the install.
                tracing::warn!(
                    %err,
                    platform_pkg,
                    "platform-specific binary install failed; \
                     copilot may fall back to JS implementation"
                );
                eprintln!(
                    "⚠️  Platform binary {platform_pkg} failed to install: {err}\n   \
                     Copilot may still work via JS fallback on recent Node.js versions."
                );
            }
        } else {
            tracing::info!(
                os_name,
                arch,
                "no known platform binary for this OS/arch; skipping"
            );
        }
    }

    // Issue #1266: the same shape as the copilot arm above, for the same
    // reason. `@anthropic-ai/claude-code` ships a placeholder at
    // `bin/claude.exe` and materializes the real ~339 MB native binary from an
    // optionalDependency in its postinstall. `--omit=optional` withholds the
    // dependency and `--ignore-scripts` withholds the postinstall, so the base
    // install above ALWAYS leaves the placeholder behind. Rather than relax
    // either flag for any package, install the one platform package explicitly
    // by name and then run the vendor's own postinstall against it.
    //
    // Exact string equality, never `contains()` / `starts_with()` / a tool
    // name: a near-miss such as `@anthropic-ai/claude-code-evil` must not
    // inherit the exception. See tests/claude_install_contract.rs.
    if package == "@anthropic-ai/claude-code" {
        materialize_claude_native(&npm, &prefix);
    }

    persist_path_hint(&bin_dir)?;
    Ok(())
}

/// Install the platform-native package for `@anthropic-ai/claude-code` and run
/// the vendor's postinstall so the real binary replaces the placeholder.
///
/// Never fails the caller. Every problem here warns, tells the user, and
/// returns: the health gate in `launch_target` will reject an unmaterialized
/// placeholder and resolution falls through to whatever else on the host is
/// healthy. A failed materialization must never fail a launch.
fn materialize_claude_native(npm: &Path, prefix: &Path) {
    // The honest threat model, stated where the exception lives rather than
    // only in the pull request: amplihack is about to exec this package's
    // native binary. Declining to run the package's own postinstall while
    // planning to exec its binary seconds later is not a coherent security
    // posture — the postinstall is strictly less privileged than what
    // immediately follows it. Note the scope of the exception: `--ignore-scripts`
    // still applies to the platform package installed below, so that package's
    // own lifecycle scripts stay suppressed. The residual delta over the old
    // behaviour is exactly ONE named script, at an absolute path, under a prefix
    // amplihack owns, for ONE exact-matched package name.
    let pkg_dir = prefix
        .join("lib")
        .join("node_modules")
        .join(CLAUDE_NPM_PACKAGE);

    let Some(version) = read_pinned_claude_version(&pkg_dir) else {
        tracing::warn!(
            pkg_dir = %pkg_dir.display(),
            "could not read a valid version from the installed claude package; \
             skipping native binary materialization"
        );
        eprintln!(
            "⚠️  Could not determine the installed @anthropic-ai/claude-code version; \
             skipping the native binary step."
        );
        return;
    };

    let (os_name, arch) = current_platform();
    let candidates = claude_platform_packages(os_name, arch, detect_musl());
    if candidates.is_empty() {
        tracing::info!(
            os_name,
            arch,
            "no known claude platform package for this OS/arch; skipping"
        );
        return;
    }

    let placeholder = pkg_dir.join("bin").join("claude.exe");
    for platform_pkg in candidates {
        let pinned = format!("{platform_pkg}@{version}");
        println!("📦 Installing platform binary {pinned}...");
        if let Err(err) = run_npm_install(npm, prefix, &pinned) {
            tracing::warn!(%err, platform_pkg, "claude platform package install failed");
            continue;
        }
        run_claude_vendor_postinstall(&pkg_dir, prefix);
        if claude_binary_is_materialized(&placeholder) {
            tracing::info!(
                platform_pkg,
                binary = %placeholder.display(),
                "claude native binary materialized"
            );
            return;
        }
        tracing::warn!(
            platform_pkg,
            binary = %placeholder.display(),
            "postinstall ran but the native binary was not materialized; trying the next candidate"
        );
    }

    tracing::warn!(
        os_name,
        arch,
        "claude native binary could not be materialized"
    );
    eprintln!(
        "⚠️  The Claude Code native binary could not be installed. amplihack will \
         launch a working copy from elsewhere on your PATH if one exists.\n   \
         To install manually:\n     npm install -g @anthropic-ai/claude-code"
    );
}

/// Read and validate the version of the installed claude package.
///
/// SEC-2: this value is concatenated into npm's argv as `<pkg>@<version>`, so
/// it is validated against an ANCHORED regex at the boundary where it leaves
/// `package.json` and fails closed on any mismatch. An unanchored pattern would
/// accept `1.2.3 && rm -rf ~`.
///
/// The digit and length bounds are part of the same rule: `\d+` with no cap
/// lets a poisoned `package.json` with a megabyte-long numeric version build a
/// megabyte of argv. That fails safe at the kernel's `MAX_ARG_STRLEN` rather
/// than doing damage, but bounding it here is free and it is what the frozen
/// contract specifies.
fn read_pinned_claude_version(pkg_dir: &Path) -> Option<String> {
    /// No published semver component has nine digits, and no published version
    /// string is 64 characters long.
    const MAX_PINNED_VERSION_LEN: usize = 64;
    static PINNED_VERSION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\d{1,9}\.\d{1,9}\.\d{1,9}$").expect("static pinned-version regex")
    });
    let text = fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    let manifest: Value = serde_json::from_str(&text).ok()?;
    let version = manifest.get("version")?.as_str()?;
    (version.len() <= MAX_PINNED_VERSION_LEN && PINNED_VERSION.is_match(version))
        .then(|| version.to_string())
}

/// Run the vendor's `install.cjs`, ignoring its exit code.
///
/// Verified against the vendor source: `main()` returns normally — exit 0 — for
/// an unsupported platform, for a release channel with no native binaries, and
/// for a failed `require.resolve`. Only a throwing `placeBinary` sets exit
/// code 1. Its exit status is therefore not a success signal, and the caller
/// confirms the outcome by inspecting the resulting file instead.
fn run_claude_vendor_postinstall(pkg_dir: &Path, prefix: &Path) {
    let Some(script) = contained_install_script(pkg_dir, prefix) else {
        return;
    };
    let Ok(node) = BinaryFinder::find("node") else {
        // npm's presence implies node's, so this is close to unreachable. The
        // managed-Node download exists for copilot's Node >= 24 requirement;
        // install.cjs needs only Node >= 12 and does not justify pulling a
        // runtime down.
        tracing::warn!("node not found; cannot run the claude postinstall");
        return;
    };
    let mut cmd = Command::new(node.path);
    cmd.arg(&script).current_dir(pkg_dir);
    match run_with_timeout(cmd, INSTALL_TIMEOUT) {
        Ok(status) => tracing::debug!(
            script = %script.display(),
            code = status.code(),
            "claude postinstall finished (exit status is not a success signal)"
        ),
        Err(err) => tracing::warn!(%err, script = %script.display(), "claude postinstall failed"),
    }
}

/// Resolve the vendor's `install.cjs` and prove it is where amplihack thinks
/// it is before executing it.
///
/// SEC-2. The exception that lets this script run at all is argued on the
/// grounds that it is "ONE named script, at an absolute path, under a prefix
/// amplihack owns". Without this check that last clause is an assumption
/// rather than an assertion: if
/// `<prefix>/lib/node_modules/@anthropic-ai/claude-code` is a symlink — planted
/// by another package's install, or left behind by an `npm link` — amplihack
/// would execute arbitrary JS from wherever it points, with the user's
/// privileges. `canonicalize` resolves every symlink in the path, so the
/// containment test is against where the file actually is, not where it is
/// spelled.
///
/// The boundary is `prefix`, and it has to be. Every other link in the chain is
/// derived from the untrusted path itself, so canonicalizing one of them —
/// `<prefix>/lib/node_modules/@anthropic-ai`, say — lets the attacker move the
/// boundary along with the target and makes `starts_with` tautological: the
/// script is always under the directory it was resolved through. `prefix` is
/// the highest component amplihack computes for itself rather than reading out
/// of the package tree, which is what makes it the soundest anchor available
/// here.
///
/// A-6 — but note that `prefix` is canonicalized too, so the boundary follows
/// its own symlinks. `npm_prefix_dir()` only *computes* `$HOME/.npm-global`;
/// `create_dir_all` is a no-op over an existing symlink, so if `~/.npm-global`
/// is itself a link the boundary moves with it and this check weakens to
/// "somewhere under wherever that link points". Severity is genuinely low and
/// no code change is warranted here: the precondition is write access under
/// `$HOME`, which already lets an attacker drop `install.cjs` at the legitimate
/// path. This check is containment against a link planted *inside* the package
/// tree, not the last line of defence against a compromised `$HOME`.
///
/// This is containment, not a symlink ban. npm creates symlinks inside a prefix
/// as a matter of course, so a link that still resolves within `prefix` is
/// accepted; only one that leaves it is refused.
///
/// Returns `None` (and warns) rather than failing: like every other step here,
/// a problem leaves the placeholder in place and the health gate deals with it.
fn contained_install_script(pkg_dir: &Path, prefix: &Path) -> Option<PathBuf> {
    let (Ok(script), Ok(boundary)) = (
        pkg_dir.join("install.cjs").canonicalize(),
        prefix.canonicalize(),
    ) else {
        tracing::warn!(
            pkg_dir = %pkg_dir.display(),
            "could not resolve the claude postinstall script; skipping it"
        );
        return None;
    };
    if !script.starts_with(&boundary) {
        tracing::warn!(
            script = %script.display(),
            prefix = %boundary.display(),
            "the claude postinstall script resolves outside the prefix amplihack \
             owns; refusing to run it"
        );
        return None;
    }
    Some(script)
}

/// Outcome verification: is the file at `path` a real native binary?
///
/// An unreadable file answers `false`, and that is a decision rather than an
/// accident: this runs immediately after amplihack's own `npm install`, so a
/// head it cannot read is a failed install, not a healthy one. The read error
/// is no longer folded into `read == 0` — `label_failed_probe` deleted the same
/// `.unwrap_or(0)` idiom for turning an EACCES into a confident wrong
/// diagnosis, and a silent zero here would report "placeholder shape" for a
/// file whose bytes were never seen.
fn claude_binary_is_materialized(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let mut head = [0u8; 8];
    let Ok(read) = fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head))
    else {
        tracing::warn!(
            path = %path.display(),
            "could not read the installed binary to verify it materialised"
        );
        return false;
    };
    is_materialized(&head[..read], metadata.len())
}

fn run_npm_install(npm: &Path, prefix: &Path, package: &str) -> Result<()> {
    let mut npm_cmd = Command::new(npm);
    npm_cmd
        .arg("install")
        .arg("-g")
        .arg("--prefix")
        .arg(prefix)
        .arg("--omit=optional")
        .arg(package)
        .arg("--ignore-scripts");
    let status = run_with_timeout(npm_cmd, INSTALL_TIMEOUT).with_context(|| {
        format!(
            "npm install timed out for package '{package}' after {}s.\n\
             This is often caused by npm hanging on cross-platform optional deps.\n\
             Try running manually:\n  \
             npm install -g --prefix {} --omit=optional --ignore-scripts {package}",
            INSTALL_TIMEOUT.as_secs(),
            prefix.display(),
        )
    })?;

    if !status.success() {
        bail!(
            "npm install failed for package '{package}' (exit code: {code}).\n\
             Try running manually:\n  \
             npm install -g --prefix {prefix} --omit=optional --ignore-scripts {package}\n\
             If the problem persists, check npm logs:\n  \
             npm cache clean --force && npm install -g --prefix {prefix} {package}",
            package = package,
            code = status
                .code()
                .map_or("unknown".to_string(), |c| c.to_string()),
            prefix = prefix.display(),
        );
    }
    Ok(())
}

/// Determine the correct `@github/copilot-{os}-{arch}` package for the
/// current platform. Returns `None` for unrecognized OS/arch combinations,
/// which signals the caller to skip the platform binary install (non-fatal).
fn copilot_platform_package(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("@github/copilot-linux-x64"),
        ("linux", "aarch64") => Some("@github/copilot-linux-arm64"),
        ("macos", "x86_64") => Some("@github/copilot-darwin-x64"),
        ("macos", "aarch64") => Some("@github/copilot-darwin-arm64"),
        ("windows", "x86_64") => Some("@github/copilot-win32-x64"),
        ("windows", "aarch64") => Some("@github/copilot-win32-arm64"),
        _ => None,
    }
}

/// Returns `(os_name, arch)` using Rust's compile-time target constants.
/// Values match `copilot_platform_package` keys directly ("linux", "macos",
/// "windows" for OS; "x86_64", "aarch64" for arch).
fn current_platform() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

/// Remove stale `.<name>-XXXX` temp dirs that npm leaves behind in the scope
/// directory after a crashed install.
///
/// For a scoped package like `@github/copilot`, npm stages the new copy in
/// `$prefix/lib/node_modules/@github/.copilot-XXXX` and then renames over the
/// final directory. If the rename fails (or npm is killed mid-install), the
/// temp dir is left behind and every subsequent `npm install` trips ENOTEMPTY.
///
/// For an unscoped package `foo`, npm stages it as
/// `$prefix/lib/node_modules/.foo-XXXX`.
fn clean_stale_npm_temp_dirs(prefix: &Path, package: &str) {
    let node_modules = prefix.join("lib").join("node_modules");
    let (scope_dir, dot_prefix) = match split_npm_package(package) {
        Some((scope, name)) => (node_modules.join(format!("@{scope}")), format!(".{name}-")),
        None => (node_modules, format!(".{package}-")),
    };
    let Ok(entries) = fs::read_dir(&scope_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with(&dot_prefix) {
            continue;
        }
        let path = entry.path();
        tracing::warn!(path = %path.display(), "removing stale npm temp dir");
        if let Err(err) = fs::remove_dir_all(&path) {
            tracing::warn!(%err, path = %path.display(), "failed to remove stale npm temp dir");
        } else {
            println!("  🧹 Removed stale npm temp dir: {}", path.display());
        }
    }
}

/// Remove the installed package directory (if present) so `npm install` can
/// recreate it from scratch. Used as a final fallback when the rename path is
/// still wedged.
fn remove_package_install_dir(prefix: &Path, package: &str) {
    let node_modules = prefix.join("lib").join("node_modules");
    let install_dir = match split_npm_package(package) {
        Some((scope, name)) => node_modules.join(format!("@{scope}")).join(name),
        None => node_modules.join(package),
    };
    if install_dir.exists() {
        tracing::warn!(
            path = %install_dir.display(),
            "removing existing package install dir before retry"
        );
        let _ = fs::remove_dir_all(&install_dir);
    }
}

fn split_npm_package(package: &str) -> Option<(&str, &str)> {
    let rest = package.strip_prefix('@')?;
    let (scope, name) = rest.split_once('/')?;
    if scope.is_empty() || name.is_empty() {
        return None;
    }
    Some((scope, name))
}

fn install_amplifier() -> Result<()> {
    let uv = BinaryFinder::find("uv")
        .context("uv is required to install amplifier")?
        .path;
    let bin_dir = uv_bin_dir()?;
    prepend_path(&bin_dir)?;

    println!("📦 Installing amplifier via uv tool...");
    let mut uv_cmd = Command::new(uv);
    uv_cmd
        .arg("tool")
        .arg("install")
        .arg("git+https://github.com/microsoft/amplifier");
    let status =
        run_with_timeout(uv_cmd, INSTALL_TIMEOUT).context("failed to execute uv tool install")?;

    if !status.success() {
        bail!("uv tool install failed for amplifier");
    }

    persist_path_hint(&bin_dir)?;
    Ok(())
}

fn configure_codex() -> Result<()> {
    let config_dir = home_dir()?.join(".openai").join("codex");
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create {}", config_dir.display()))?;
    let config_path = config_dir.join("config.json");

    let mut value = if config_path.exists() {
        let raw = fs::read_to_string(&config_path).with_context(|| {
            format!(
                "refusing to overwrite unreadable existing Codex config {}",
                config_path.display()
            )
        })?;
        let parsed: Value = serde_json::from_str(&raw).with_context(|| {
            format!(
                "refusing to overwrite malformed existing Codex config {}",
                config_path.display()
            )
        })?;
        if !parsed.is_object() {
            bail!(
                "refusing to overwrite existing Codex config {} because it is not an object",
                config_path.display()
            );
        }
        parsed
    } else {
        json!({})
    };

    let object = value
        .as_object_mut()
        .expect("value is guaranteed an object");
    if object.get("approval_mode").and_then(Value::as_str) != Some("auto") {
        object.insert(
            "approval_mode".to_string(),
            Value::String("auto".to_string()),
        );
        fs::write(&config_path, serde_json::to_string_pretty(&value)? + "\n")
            .with_context(|| format!("failed to write {}", config_path.display()))?;
    }

    Ok(())
}

fn prepend_path(dir: &Path) -> Result<()> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    // Check membership without allocating a Vec in the common already-present case.
    if std::env::split_paths(&current).any(|existing| existing == dir) {
        return Ok(());
    }

    let mut updated = vec![dir.to_path_buf()];
    updated.extend(std::env::split_paths(&current));
    let joined = std::env::join_paths(updated).context("failed to rebuild PATH")?;
    // SAFETY: This CLI is single-process during bootstrap and updates PATH intentionally.
    unsafe {
        std::env::set_var("PATH", joined);
    }
    Ok(())
}

fn persist_path_hint(bin_dir: &Path) -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let profile = if shell.ends_with("/zsh") || shell.ends_with("/zsh5") {
        home_dir()?.join(".zshrc")
    } else {
        home_dir()?.join(".bashrc")
    };
    let export_line = format!("export PATH=\"{}:$PATH\"", bin_dir.display());

    let existing = fs::read_to_string(&profile).unwrap_or_default();
    if existing.contains(&export_line) {
        return Ok(());
    }

    let mut content = existing;
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str("# Added by amplihack\n");
    content.push_str(&export_line);
    content.push('\n');

    fs::write(&profile, content).with_context(|| format!("failed to update {}", profile.display()))
}

/// npm's `--prefix` for amplihack's own install: the PARENT of the bin
/// directory, because that is what npm's flag takes.
///
/// C1 — derived from `launch_target::amplihack_prefix_bin` rather than spelled
/// again, so the two cannot drift. Every caller here immediately does
/// `.join("bin")` to get back to where it started; the round trip is what keeps
/// npm's argument and amplihack's search path provably the same directory.
fn npm_prefix_dir() -> Result<PathBuf> {
    let bin = launch_target::amplihack_prefix_bin(&home_dir()?);
    bin.parent()
        .map(Path::to_path_buf)
        .context("amplihack's npm prefix has no parent directory")
}

fn uv_bin_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("UV_TOOL_BIN_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            fs::create_dir_all(&path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            return Ok(path);
        }
    }

    let path = home_dir()?.join(".local").join("bin");
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(path)
}

/// Delegates to [`launch_target::home_dir`] so the install path and the
/// resolution path cannot disagree about where home is. This used to read
/// `HOME` alone while `candidate_paths` read `HOME` or `USERPROFILE`; see that
/// function for what the drift cost on Windows.
fn home_dir() -> Result<PathBuf> {
    launch_target::home_dir().ok_or_else(|| anyhow!("neither HOME nor USERPROFILE is set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_codex_sets_auto_mode() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        configure_codex().unwrap();

        let config = fs::read_to_string(temp.path().join(".openai/codex/config.json")).unwrap();
        let value: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(value["approval_mode"], "auto");

        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn configure_codex_refuses_malformed_existing_config_without_overwriting() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let config_dir = temp.path().join(".openai/codex");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.json");
        let original = "{ this is not json";
        fs::write(&config_path, original).unwrap();

        let error = configure_codex().expect_err("malformed config must be preserved");

        assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
        assert!(
            error.to_string().contains("malformed")
                || error.to_string().contains("refusing to overwrite"),
            "error should clearly explain malformed config preservation; got {error:#}"
        );

        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn configure_codex_refuses_non_object_existing_config_without_overwriting() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let config_dir = temp.path().join(".openai/codex");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.json");
        let original = "[\"not\", \"an\", \"object\"]\n";
        fs::write(&config_path, original).unwrap();

        let error = configure_codex().expect_err("non-object config must be preserved");

        assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
        assert!(
            error.to_string().contains("not an object")
                || error.to_string().contains("refusing to overwrite"),
            "error should clearly explain non-object config preservation; got {error:#}"
        );

        crate::test_support::restore_home(previous_home);
    }

    #[test]
    fn node_checksum_manifest_requires_exact_archive_entry() {
        let expected = "a".repeat(64);
        let other = "b".repeat(64);
        let manifest = format!("{expected}  node-v1-linux-x64.tar.xz\n{other}  other.tar.xz\n");

        let digest = find_sha256_for_archive(&manifest, "node-v1-linux-x64.tar.xz").unwrap();

        assert_eq!(digest, expected);
        assert!(find_sha256_for_archive(&manifest, "missing.tar.xz").is_err());
    }

    #[test]
    fn node_archive_sha256_mismatch_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("node-test.tar.xz");
        let checksum = temp.path().join("SHASUMS256.txt");
        fs::write(&archive, b"not the expected archive").unwrap();
        fs::write(&checksum, format!("{}  node-test.tar.xz\n", "0".repeat(64))).unwrap();

        let error = verify_node_archive_sha256(&archive, &checksum, "node-test.tar.xz")
            .expect_err("checksum mismatch must fail closed");

        assert!(
            error.to_string().contains("SHA-256 verification failed"),
            "checksum mismatch should be explicit; got {error:#}"
        );
    }

    // ========================================================================
    // Issue #585: copilot_platform_package() helper
    // ========================================================================

    #[test]
    fn copilot_platform_package_returns_correct_linux_x64() {
        // Contract: On linux/x86_64, must return @github/copilot-linux-x64
        let result = copilot_platform_package("linux", "x86_64");
        assert_eq!(
            result,
            Some("@github/copilot-linux-x64"),
            "linux + x86_64 must map to copilot-linux-x64"
        );
    }

    #[test]
    fn copilot_platform_package_returns_correct_linux_arm64() {
        let result = copilot_platform_package("linux", "aarch64");
        assert_eq!(
            result,
            Some("@github/copilot-linux-arm64"),
            "linux + aarch64 must map to copilot-linux-arm64"
        );
    }

    #[test]
    fn copilot_platform_package_returns_correct_macos_arm64() {
        let result = copilot_platform_package("macos", "aarch64");
        assert_eq!(
            result,
            Some("@github/copilot-darwin-arm64"),
            "macos + aarch64 must map to copilot-darwin-arm64"
        );
    }

    #[test]
    fn copilot_platform_package_returns_correct_macos_x64() {
        let result = copilot_platform_package("macos", "x86_64");
        assert_eq!(
            result,
            Some("@github/copilot-darwin-x64"),
            "macos + x86_64 must map to copilot-darwin-x64"
        );
    }

    #[test]
    fn copilot_platform_package_returns_correct_windows_x64() {
        let result = copilot_platform_package("windows", "x86_64");
        assert_eq!(
            result,
            Some("@github/copilot-win32-x64"),
            "windows + x86_64 must map to copilot-win32-x64"
        );
    }

    #[test]
    fn copilot_platform_package_returns_none_for_unknown_os() {
        let result = copilot_platform_package("freebsd", "x86_64");
        assert_eq!(
            result, None,
            "unknown OS must return None (non-fatal fallback)"
        );
    }

    #[test]
    fn copilot_platform_package_returns_none_for_unknown_arch() {
        let result = copilot_platform_package("linux", "riscv64");
        assert_eq!(
            result, None,
            "unknown arch must return None (non-fatal fallback)"
        );
    }

    // ========================================================================
    // Issue #585: split_npm_package (existing helper, verify edge cases)
    // ========================================================================

    #[test]
    fn split_npm_package_handles_copilot_platform_packages() {
        // Contract: platform-specific packages like @github/copilot-linux-x64
        // must parse correctly through split_npm_package.
        assert_eq!(
            split_npm_package("@github/copilot-linux-x64"),
            Some(("github", "copilot-linux-x64"))
        );
        assert_eq!(
            split_npm_package("@github/copilot-darwin-arm64"),
            Some(("github", "copilot-darwin-arm64"))
        );
    }

    #[test]
    fn persist_path_hint_is_idempotent() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous_home = crate::test_support::set_home(temp.path());
        let previous_shell = std::env::var_os("SHELL");
        // SAFETY: Test-only shell override.
        unsafe {
            std::env::set_var("SHELL", "/bin/bash");
        }

        let bin_dir = temp.path().join(".npm-global/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        persist_path_hint(&bin_dir).unwrap();
        persist_path_hint(&bin_dir).unwrap();

        let profile = fs::read_to_string(temp.path().join(".bashrc")).unwrap();
        assert_eq!(profile.matches("Added by amplihack").count(), 1);

        match previous_shell {
            Some(value) => unsafe { std::env::set_var("SHELL", value) },
            None => unsafe { std::env::remove_var("SHELL") },
        }
        crate::test_support::restore_home(previous_home);
    }

    // =======================================================================
    // F-S1 / SEC-2 — containment is anchored to the prefix amplihack CREATED,
    // never to a boundary the untrusted path defines for itself.
    //
    // `contained_install_script` exists to turn "install.cjs sits under a
    // prefix amplihack owns" from an assumption into an assertion, because
    // that clause is the entire justification for the `--ignore-scripts`
    // exception. Deriving the boundary by canonicalizing the *package-derived*
    // `@anthropic-ai` directory defeats the check: if that directory is itself
    // a symlink out of the tree, the boundary moves with the attacker and
    // `starts_with` trivially succeeds.
    //
    // Only one link in the chain is not package-derived: the prefix, which
    // amplihack creates. That is the only sound anchor.
    // =======================================================================

    /// `<root>/prefix` with `lib/node_modules` created, standing in for the
    /// npm prefix amplihack owns.
    fn owned_prefix(root: &Path) -> PathBuf {
        let prefix = root.join("prefix");
        fs::create_dir_all(prefix.join("lib").join("node_modules")).unwrap();
        prefix
    }

    fn pkg_dir_under(prefix: &Path) -> PathBuf {
        prefix
            .join("lib")
            .join("node_modules")
            .join(CLAUDE_NPM_PACKAGE)
    }

    #[test]
    fn the_ordinary_npm_layout_is_accepted() {
        // Positive control. The containment fix must not be so tight that it
        // refuses the real thing — a refusal here silently disables the whole
        // native-materialization path and reintroduces the placeholder.
        let temp = tempfile::tempdir().unwrap();
        let prefix = owned_prefix(temp.path());
        let pkg_dir = pkg_dir_under(&prefix);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(pkg_dir.join("install.cjs"), "// vendor postinstall\n").unwrap();

        let script = contained_install_script(&pkg_dir, &prefix)
            .expect("a real npm layout under amplihack's own prefix must be accepted");
        assert!(
            script.ends_with("install.cjs"),
            "expected the vendor script, got {}",
            script.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_scope_dir_cannot_redefine_its_own_containment_boundary() {
        // The attack: another package's install, or a stray `npm link`, leaves
        // `<prefix>/lib/node_modules/@anthropic-ai` as a symlink to a tree the
        // attacker controls. Canonicalizing THAT directory to derive the
        // boundary makes the check tautological — the script is always under
        // the thing it resolved through — and amplihack execs arbitrary JS
        // with the user's privileges, during an install, as root's shell would
        // never have to be involved.
        let temp = tempfile::tempdir().unwrap();
        let prefix = owned_prefix(temp.path());

        let outside = temp.path().join("elsewhere");
        fs::create_dir_all(outside.join("claude-code")).unwrap();
        let planted = outside.join("claude-code").join("install.cjs");
        fs::write(&planted, "// arbitrary attacker JS\n").unwrap();

        std::os::unix::fs::symlink(
            &outside,
            prefix
                .join("lib")
                .join("node_modules")
                .join("@anthropic-ai"),
        )
        .unwrap();

        assert!(
            contained_install_script(&pkg_dir_under(&prefix), &prefix).is_none(),
            "install.cjs resolves to {planted}, which is outside the prefix \
             amplihack created at {prefix}. It must be refused: the boundary \
             has to be the prefix, not a directory the untrusted path resolves \
             through.",
            planted = planted.display(),
            prefix = prefix.display(),
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_package_dir_cannot_escape_the_prefix() {
        // Same escape one level deeper: the scope directory is real and the
        // `claude-code` package directory is the symlink. A boundary derived
        // from `@anthropic-ai` catches this one by accident; the test pins
        // that it is caught on purpose, from either level.
        let temp = tempfile::tempdir().unwrap();
        let prefix = owned_prefix(temp.path());
        let scope = prefix
            .join("lib")
            .join("node_modules")
            .join("@anthropic-ai");
        fs::create_dir_all(&scope).unwrap();

        let outside = temp.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("install.cjs"), "// arbitrary attacker JS\n").unwrap();

        std::os::unix::fs::symlink(&outside, scope.join("claude-code")).unwrap();

        assert!(
            contained_install_script(&pkg_dir_under(&prefix), &prefix).is_none(),
            "a symlinked package directory pointing at {} must be refused",
            outside.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_stays_inside_the_prefix_is_still_accepted() {
        // The check is containment, not a blanket symlink ban. npm itself
        // creates symlinks inside the prefix, so refusing every link would
        // break real installs. This is the boundary case that separates
        // "canonicalize and compare against the prefix" from "reject links".
        let temp = tempfile::tempdir().unwrap();
        let prefix = owned_prefix(temp.path());
        let scope = prefix
            .join("lib")
            .join("node_modules")
            .join("@anthropic-ai");
        fs::create_dir_all(&scope).unwrap();

        let real = prefix.join("real-claude-code");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("install.cjs"), "// vendor postinstall\n").unwrap();

        std::os::unix::fs::symlink(&real, scope.join("claude-code")).unwrap();

        assert!(
            contained_install_script(&pkg_dir_under(&prefix), &prefix).is_some(),
            "a symlink resolving to {} — still inside the prefix — must be accepted",
            real.display()
        );
    }

    #[test]
    fn a_missing_install_script_is_refused_rather_than_assumed() {
        let temp = tempfile::tempdir().unwrap();
        let prefix = owned_prefix(temp.path());
        let pkg_dir = pkg_dir_under(&prefix);
        fs::create_dir_all(&pkg_dir).unwrap();

        assert!(
            contained_install_script(&pkg_dir, &prefix).is_none(),
            "no install.cjs on disk means nothing to run"
        );
    }

    // ------------------------------------------------------------------
    // log_rejected_candidates — a tracing field is still a terminal sink
    //
    // The default subscriber is a human `fmt` layer on stderr, so a `%`-sigil
    // field arrives as raw bytes. These paths come from `$PATH`, `$HOME`,
    // `*_BINARY_PATH` or a planted filename, which is the same provenance
    // `rejection_report` and `enrich_spawn_error` sanitise. This pins the third
    // site onto the same sanitiser so the three cannot drift.
    // ------------------------------------------------------------------

    /// Capture what the `fmt` subscriber would actually write to the terminal.
    fn captured_rejection_log(planted: &str) -> String {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = Buffer;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();

        let resolution = Resolution {
            target: None,
            rejected: vec![(
                PathBuf::from(planted),
                amplihack_utils::launch_target::Rejection::PlaceholderStub,
            )],
            halted_on_user_override: None,
        };
        tracing::subscriber::with_default(subscriber, || {
            log_rejected_candidates("claude", &resolution)
        });

        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn a_planted_candidate_name_cannot_write_escape_sequences_to_the_terminal() {
        for planted in [
            "/tmp/\x1b[2J\x1b[Hclaude",          // CSI: clear screen, home cursor
            "/tmp/\x1b]52;c;ZXZpbA==\x07claude", // OSC 52: write the clipboard
            "/tmp/\u{9b}2Jclaude",               // 8-bit C1 CSI, no ESC involved
        ] {
            let logged = captured_rejection_log(planted);
            assert!(
                !logged.contains('\x1b') && !logged.contains('\u{9b}'),
                "{planted:?} reached the terminal as {logged:?}"
            );
        }
    }

    #[test]
    fn a_planted_candidate_name_cannot_forge_a_second_log_line() {
        // The tail is the payload: a newline here manufactures an extra row in
        // amplihack's own voice, on the diagnostic the user is reading to
        // decide what to do next.
        let logged =
            captured_rejection_log("/tmp/claude\n  INFO the install is fine; run it directly");
        assert!(
            !logged.contains("run it directly"),
            "the forged tail survived: {logged:?}"
        );
        assert_eq!(
            logged.lines().count(),
            1,
            "one rejection must render as exactly one line: {logged:?}"
        );
    }

    #[test]
    fn an_ordinary_candidate_path_is_logged_intact() {
        // Non-vacuity: the assertions above must be about control characters,
        // not about the path being dropped or mangled in general.
        let logged = captured_rejection_log("/usr/local/bin/claude");
        assert!(
            logged.contains("/usr/local/bin/claude"),
            "an ordinary path must survive verbatim: {logged:?}"
        );
    }
}
