//! Binary resolution and deployment for amplihack-hooks.

use super::paths::{find_binary, home_dir, is_executable};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// Resolve the amplihack-hooks binary through a 5-step chain.
///
/// 1. `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` env var (if set AND the path exists)
/// 2. Sibling of the current executable
/// 3. PATH lookup (handles reinstall after uninstall removes ~/.local/bin copy)
/// 4. `~/.local/bin/amplihack-hooks`
/// 5. `~/.cargo/bin/amplihack-hooks`
pub(super) fn find_hooks_binary() -> Result<PathBuf> {
    // Step 1: env var override
    if let Some(val) = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") {
        let p = PathBuf::from(&val);
        if p.exists() {
            return Ok(p);
        }
        bail!("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH is set to {p:?} but that path does not exist");
    }

    // Step 2: sibling of current exe
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("amplihack-hooks");
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }

    // Step 3: PATH lookup — finds system-wide installs (e.g. /usr/local/bin) even
    // after uninstall has removed the ~/.local/bin copy.
    if let Some(found) = find_binary("amplihack-hooks") {
        return Ok(found);
    }

    // Steps 4-5: ~/.local/bin then ~/.cargo/bin — one home_dir() call covers both.
    if let Ok(home) = home_dir() {
        for suffix in &[".local/bin", ".cargo/bin"] {
            let candidate = home.join(suffix).join("amplihack-hooks");
            if is_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }

    bail!(
        "amplihack-hooks binary not found. Install it with:\n  \
         cargo install --git https://github.com/rysweet/amplihack-rs amplihack-hooks\n  \
         or set AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH to its location."
    )
}

/// Validate that a binary path contains no shell metacharacters that could cause
/// shell misinterpretation or bypass command-string validation.
///
/// Blocks: space (` `), single quote (`'`), double quote (`"`), backslash (`\`),
/// plus all the shell operator characters blocked by `validate_hook_command_string`.
pub(super) fn validate_binary_path(path: &str) -> Result<()> {
    const BLOCKED: &[char] = &[
        '|', '&', ';', '$', '`', '(', ')', '{', '}', '<', '!', '>', '#', '~', '*', ' ', '\'', '"',
        '\\',
    ];
    for ch in BLOCKED {
        if path.contains(*ch) {
            bail!("binary path contains unsafe character '{ch}': {path}");
        }
    }
    Ok(())
}

/// Validate that a hook command string contains no shell metacharacters.
///
/// Blocks: `|`, `&`, `;`, `$`, backtick, `(`, `)`, `{`, `}`, `<`, `!`, `>`, `#`, `~`, `*`
pub(super) fn validate_hook_command_string(cmd: &str) -> Result<()> {
    const BLOCKED: &[char] = &[
        '|', '&', ';', '$', '`', '(', ')', '{', '}', '<', '!', '>', '#', '~', '*',
    ];
    for ch in BLOCKED {
        if cmd.contains(*ch) {
            bail!("hook command string contains unsafe metacharacter '{ch}': {cmd}");
        }
    }
    Ok(())
}

/// Copy the Rust binaries to `~/.local/bin` with 0o755 perms.
/// Emits a PATH advisory if `~/.local/bin` is not in `$PATH`.
/// Returns the list of deployed paths for the manifest.
pub(super) fn deploy_binaries() -> Result<Vec<PathBuf>> {
    use super::filesystem::deploy_binary;

    let home = home_dir()?;
    let local_bin = super::paths::preferred_user_bin_dir()?;
    fs::create_dir_all(&local_bin)
        .with_context(|| format!("failed to create {}", local_bin.display()))?;

    let hooks_src = find_hooks_binary()?;
    let hooks_dst = local_bin.join("amplihack-hooks");
    deploy_binary(&hooks_src, &hooks_dst).with_context(|| {
        format!(
            "failed to deploy {} to {}",
            hooks_src.display(),
            hooks_dst.display()
        )
    })?;

    let mut deployed = vec![hooks_dst.clone()];

    // Also deploy the amplihack binary itself plus its asset-resolver sidecar.
    //
    // Issue #885: `amplihack update` self-updates the on-disk binary and then
    // re-runs `install` to refresh framework assets. On Linux, replacing the
    // running binary's file leaves `std::env::current_exe()` pointing at a
    // now-unlinked inode — the kernel reports the path with a literal
    // " (deleted)" suffix (e.g. `~/.cargo/bin/amplihack (deleted)`). Copying
    // from that path fails with ENOENT and used to abort the entire asset
    // refresh before agents/skills/context were staged.
    //
    // `resolve_self_source` recovers the freshly-written binary (or a PATH
    // copy), and `deploy_self_and_resolver` is tolerant: a self-copy failure is
    // logged and skipped, never propagated, so the framework-asset staging that
    // follows in `run_install` always proceeds.
    deployed.extend(deploy_self_and_resolver(&local_bin, resolve_self_source()));

    // PATH advisory + auto-persist
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let in_path = std::env::split_paths(&path_var).any(|dir| dir == local_bin);
    if !in_path {
        println!(
            "  ⚠️  ~/.local/bin is not in $PATH. Add it to your shell profile:\n   \
             export PATH=\"$HOME/.local/bin:$PATH\""
        );
        if let Err(e) = super::paths::ensure_local_bin_on_shell_path() {
            tracing::warn!("failed to auto-persist PATH: {e}");
        }
    }

    if let Ok(current_exe) = std::env::current_exe()
        && let Ok(report) =
            crate::path_conflicts::analyze_current_process_path_conflicts(home, current_exe)
        && let Some(warning) = path_conflict_warning_after_install(&report)
    {
        println!("{warning}");
    }

    Ok(deployed)
}

/// Deploy the `amplihack` binary and its `amplihack-asset-resolver` sidecar
/// from `self_source` into `local_bin`.
///
/// Tolerant by design (issue #885): a failure to copy either binary is logged
/// to stderr and skipped, never propagated. The framework-asset refresh that
/// follows `deploy_binaries` in `run_install` must not be aborted just because
/// the self-copy could not complete — most notably when `amplihack update`
/// swapped the running binary out from under us and the source could not be
/// recovered. Returns the paths that were successfully deployed (for the
/// uninstall manifest).
///
/// When `self_source` is `None` (no real amplihack source could be located)
/// the self/resolver deploy is skipped entirely and an empty vector is
/// returned — the running binary is already installed somewhere on PATH, so
/// this is a safe no-op rather than an error.
pub(super) fn deploy_self_and_resolver(
    local_bin: &Path,
    self_source: Option<PathBuf>,
) -> Vec<PathBuf> {
    use super::filesystem::deploy_binary;

    let mut deployed = Vec::new();
    let Some(self_exe) = self_source else {
        return deployed;
    };

    let self_dst = local_bin.join("amplihack");
    if self_exe != self_dst {
        match deploy_binary(&self_exe, &self_dst) {
            Ok(()) => deployed.push(self_dst),
            Err(err) => eprintln!(
                "  ⚠️  Skipping amplihack self-copy to {}: {err:#}",
                self_dst.display()
            ),
        }
    }

    let resolver_src = self_exe
        .parent()
        .map(|dir| dir.join("amplihack-asset-resolver"))
        .filter(|candidate| is_executable(candidate))
        .or_else(|| find_binary("amplihack-asset-resolver"));
    if let Some(resolver_src) = resolver_src {
        let resolver_dst = local_bin.join("amplihack-asset-resolver");
        if resolver_src != resolver_dst {
            match deploy_binary(&resolver_src, &resolver_dst) {
                Ok(()) => deployed.push(resolver_dst),
                Err(err) => eprintln!(
                    "  ⚠️  Skipping amplihack-asset-resolver copy to {}: {err:#}",
                    resolver_dst.display()
                ),
            }
        }
    }

    deployed
}

/// Resolve a real, copyable source for the running `amplihack` binary,
/// starting from `std::env::current_exe()`.
///
/// Delegates to [`resolve_self_source_from`]; see it for the resolution order.
/// Returns `None` when `current_exe()` itself cannot be determined or no real
/// source can be located.
fn resolve_self_source() -> Option<PathBuf> {
    resolve_self_source_from(&std::env::current_exe().ok()?)
}

/// Resolve a real, copyable `amplihack` source from a possibly-stale
/// `current_exe()` path.
///
/// Resolution order:
/// 1. `raw_exe` as-is, when it points at an existing file.
/// 2. `raw_exe` with a trailing " (deleted)" marker stripped, when that path
///    exists — after an in-place `amplihack update`, the freshly-written
///    replacement binary lives at the original location while the running
///    process still reports the unlinked inode (issue #885).
/// 3. `amplihack` found on `$PATH`.
///
/// Returns `None` when none of these locate a real file.
pub(super) fn resolve_self_source_from(raw_exe: &Path) -> Option<PathBuf> {
    if raw_exe.exists() {
        return Some(raw_exe.to_path_buf());
    }
    if let Some(stripped) = strip_deleted_suffix(raw_exe)
        && stripped.exists()
    {
        return Some(stripped);
    }
    find_binary("amplihack")
}

/// Strip the literal " (deleted)" marker that Linux appends to
/// `/proc/self/exe` when the running executable's file has been unlinked
/// (e.g. by an in-place `amplihack update`). Returns `None` when the path has
/// no such suffix.
pub(super) fn strip_deleted_suffix(path: &Path) -> Option<PathBuf> {
    const DELETED_MARKER: &str = " (deleted)";
    path.to_str()?
        .strip_suffix(DELETED_MARKER)
        .map(PathBuf::from)
}

pub(super) fn path_conflict_warning_after_install(
    report: &crate::path_conflicts::PathConflictReport,
) -> Option<String> {
    let mut warning = String::new();
    for binary_name in ["amplihack", "amplihack-hooks"] {
        let Some(resolution) = report.resolution(binary_name) else {
            continue;
        };

        if resolution.is_shadowed_by_earlier_path_entry {
            let Some(preferred) = resolution.preferred_user_candidate.as_ref() else {
                continue;
            };
            append_shadow_warning(&mut warning, binary_name, resolution, &preferred.path);
        } else if resolution.has_ambiguous_candidates {
            append_ambiguity_warning(&mut warning, binary_name, resolution);
        }
    }

    if warning.is_empty() {
        None
    } else {
        Some(warning.trim_end().to_string())
    }
}

fn append_shadow_warning(
    warning: &mut String,
    binary_name: &str,
    resolution: &crate::path_conflicts::BinaryResolution,
    preferred_path: &std::path::Path,
) {
    if binary_name == "amplihack" && is_python_script(&resolution.resolved.path) {
        warning.push_str(&format!(
            "  ⚠️  A Python `amplihack` script at {} shadows the Rust binary at {}.\n",
            resolution.resolved.path.display(),
            preferred_path.display()
        ));
        warning.push_str(
            "     The Python script will intercept `amplihack` commands, preventing the Rust CLI from running.\n",
        );
        warning.push_str("     To fix, do one of the following:\n");
        warning.push_str(&format!(
            "       1. Remove the Python script:  rm {}\n",
            resolution.resolved.path.display()
        ));
        warning.push_str(
            "       2. Reorder PATH so ~/.local/bin comes first:  export PATH=\"$HOME/.local/bin:$PATH\"\n",
        );
        warning.push_str("       3. Uninstall the Python package:  pip uninstall amplihack\n");
        return;
    }

    warning.push_str(&format!(
        "  ⚠️  `{}` at {} shadows the user-level binary at {}.\n",
        binary_name,
        resolution.resolved.path.display(),
        preferred_path.display()
    ));
    warning.push_str(
        "     Reorder PATH so ~/.local/bin comes first:  export PATH=\"$HOME/.local/bin:$PATH\"\n",
    );
    warning.push_str(&format!(
        "     Or run the user-level binary directly: {}\n",
        preferred_path.display()
    ));
}

fn append_ambiguity_warning(
    warning: &mut String,
    binary_name: &str,
    resolution: &crate::path_conflicts::BinaryResolution,
) {
    warning.push_str(&format!(
        "  ⚠️  Multiple distinct `{binary_name}` binaries are on PATH:\n"
    ));
    for candidate in &resolution.canonical_candidates {
        warning.push_str(&format!("     - {}\n", candidate.path.display()));
    }
    warning.push_str(
        "     Remove stale candidates or reorder PATH so the intended user-level install resolves first.\n",
    );
}

/// Check whether a file is a Python script (shebang or .py extension).
fn is_python_script(path: &std::path::Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) == Some("py") {
        return true;
    }

    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut first_bytes = Vec::new();
    let mut limited = std::io::Read::take(std::io::BufReader::new(file), 1024);
    let Ok(_) = std::io::Read::read_to_end(&mut limited, &mut first_bytes) else {
        return false;
    };
    let mut first_line = first_bytes
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or(&first_bytes);
    while first_line
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        first_line = &first_line[..first_line.len() - 1];
    }

    if let Ok(first_line) = std::str::from_utf8(first_line) {
        return first_line.starts_with("#!")
            && (first_line.contains("python") || first_line.contains("Python"));
    }
    false
}
