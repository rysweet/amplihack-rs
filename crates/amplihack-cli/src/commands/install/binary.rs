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

/// Select the STABLE deployed `amplihack-hooks` path from a deployment set.
///
/// This is the pure selection seam for issue #911: Copilot hook registration
/// must bake the deployed `~/.local/bin/amplihack-hooks` path (returned by
/// [`deploy_binaries`]) rather than the transient source build artifact
/// (`<cwd>/target/debug/amplihack-hooks`) that [`find_hooks_binary`] can
/// resolve when installing from a worktree. Selection keys strictly on the
/// exact `amplihack-hooks` file name so ordering changes or stray build paths
/// can never be chosen. An absent entry is a hard error — baking a guessed
/// path would reintroduce the exit-127 outage.
pub(super) fn deployed_hooks_binary(deployed: &[PathBuf]) -> Result<PathBuf> {
    deployed
        .iter()
        .find(|p| p.file_name().is_some_and(|name| name == "amplihack-hooks"))
        .cloned()
        .context("no amplihack-hooks entry found in the deployed binary set")
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

    // Also copy self (the amplihack binary) and the asset-resolver into
    // ~/.local/bin. These copies are best-effort: a failure here must NOT
    // abort the framework-asset (agents/skills/context) refresh that follows
    // (issue #885). During `amplihack update` the amplihack binary was already
    // swapped in place by the self-update, so a stale/failed ~/.local/bin copy
    // is recoverable — but aborting before the asset restage leaves the
    // framework stale, which is the regression we must avoid.
    if let Ok(self_exe) = std::env::current_exe() {
        if let Some(self_dst) = deploy_self_binary(&local_bin, &self_exe) {
            deployed.push(self_dst);
        }

        // Resolve the real source so the asset-resolver is looked up next to
        // the freshly-installed binary, not the deleted-marker path.
        let self_src = resolve_running_binary_source(&self_exe);
        let resolver_src = self_src
            .parent()
            .map(|dir| dir.join("amplihack-asset-resolver"))
            .filter(|candidate| is_executable(candidate))
            .or_else(|| find_binary("amplihack-asset-resolver"));
        if let Some(resolver_src) = resolver_src {
            let resolver_dst = local_bin.join("amplihack-asset-resolver");
            if resolver_src != resolver_dst {
                match deploy_binary(&resolver_src, &resolver_dst) {
                    Ok(()) => deployed.push(resolver_dst),
                    Err(e) => {
                        println!(
                            "  ⚠️  Skipped amplihack-asset-resolver copy to {}: {e:#}",
                            resolver_dst.display()
                        );
                        tracing::warn!("amplihack-asset-resolver deploy skipped: {e:#}");
                    }
                }
            }
        }
    }

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

/// Resolve the on-disk path to copy the running amplihack binary FROM.
///
/// On Linux, `amplihack update` atomically replaces the binary underneath the
/// running process. `std::env::current_exe()` then reports the original path
/// with a trailing `" (deleted)"` marker (the kernel's tag for an unlinked
/// inode in `/proc/self/exe`). That literal path does not exist, so using it
/// as a copy source fails with `ENOENT` and previously aborted the post-update
/// framework-asset refresh (issue #885).
///
/// If `current_exe` carries the `(deleted)` marker and the un-suffixed path is
/// a real file — the freshly-installed binary the self-update just wrote —
/// return that path. Otherwise return `current_exe` unchanged so the caller's
/// deleted-source handling in [`super::filesystem::deploy_binary`] still applies
/// (it treats a missing source with an already-present destination as a no-op).
pub(super) fn resolve_running_binary_source(current_exe: &Path) -> PathBuf {
    const DELETED_MARKER: &str = " (deleted)";
    let raw = current_exe.to_string_lossy();
    if let Some(stripped) = raw.strip_suffix(DELETED_MARKER) {
        let real = PathBuf::from(stripped);
        if real.is_file() {
            return real;
        }
    }
    current_exe.to_path_buf()
}

/// Copy the running amplihack binary into `local_bin`, returning the
/// destination path when a copy actually occurred.
///
/// Resilient by design (issue #885):
/// - Resolves a Linux `"(deleted)"` `current_exe` to the real freshly-installed
///   binary via [`resolve_running_binary_source`], so `amplihack update` copies
///   the NEW binary into `~/.local/bin` rather than failing on a deleted path.
/// - Returns `None` (a successful no-op) when the resolved source already IS the
///   destination — the up-to-date binary is in place, nothing to do.
/// - NEVER propagates an error: a failed `~/.local/bin` copy is logged as a
///   warning and swallowed so the framework-asset refresh still proceeds.
pub(super) fn deploy_self_binary(local_bin: &Path, current_exe: &Path) -> Option<PathBuf> {
    use super::filesystem::deploy_binary;

    let self_src = resolve_running_binary_source(current_exe);
    let self_dst = local_bin.join("amplihack");
    // Same-file no-op: the binary is already deployed at the destination.
    if self_src == self_dst {
        return None;
    }
    match deploy_binary(&self_src, &self_dst) {
        Ok(()) => Some(self_dst),
        Err(e) => {
            println!(
                "  ⚠️  Skipped amplihack binary copy to {}: {e:#}",
                self_dst.display()
            );
            tracing::warn!("amplihack binary deploy skipped: {e:#}");
            None
        }
    }
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

#[cfg(test)]
mod tests {
    //! TDD regression tests for issue #911.
    //!
    //! Root cause: the Copilot plugin `hooks.json` baked the TRANSIENT source
    //! build path (`<cwd>/target/debug/amplihack-hooks`, returned by
    //! [`find_hooks_binary`] via the sibling-of-exe step when installing from a
    //! worktree) instead of the STABLE deployed path
    //! (`~/.local/bin/amplihack-hooks`, i.e. the first entry of
    //! [`deploy_binaries`]). When the build/worktree dir is later cleaned, every
    //! hook command exits 127 and Copilot CLI 1.0.71 fails CLOSED — denying every
    //! tool call in nested recipe sub-agents.
    //!
    //! The fix threads the DEPLOYED path through to the Copilot plugin
    //! registration. [`deployed_hooks_binary`] is the pure selection seam that
    //! encodes the invariant "pick the stable deployed `amplihack-hooks`, never a
    //! build artifact". These tests define its contract and MUST fail until it
    //! exists.
    use super::*;

    #[test]
    fn deployed_hooks_binary_selects_stable_deployed_amplihack_hooks() {
        // Mirrors the shape of deploy_binaries()'s return value: the deployed
        // amplihack-hooks under the user bin dir is the FIRST entry, followed by
        // the amplihack self-binary and the asset-resolver.
        let deployed = vec![
            PathBuf::from("/home/u/.local/bin/amplihack-hooks"),
            PathBuf::from("/home/u/.local/bin/amplihack"),
            PathBuf::from("/home/u/.local/bin/amplihack-asset-resolver"),
        ];

        let selected =
            deployed_hooks_binary(&deployed).expect("should select the deployed hooks bin");

        assert_eq!(
            selected,
            Path::new("/home/u/.local/bin/amplihack-hooks"),
            "must select the stable deployed amplihack-hooks path"
        );
        let s = selected.to_string_lossy();
        assert!(
            !s.contains("target/debug") && !s.contains("target/release"),
            "selected hooks binary must NEVER be a transient build artifact, got: {s}"
        );
        assert!(
            s.ends_with(".local/bin/amplihack-hooks"),
            "selected hooks binary must be the deployed ~/.local/bin copy, got: {s}"
        );
    }

    #[test]
    fn deployed_hooks_binary_ignores_non_hooks_entries() {
        // Even if ordering changes, selection must key on the amplihack-hooks
        // file name — never accidentally return the amplihack self-binary or a
        // stray transient path that happens to be present.
        let deployed = vec![
            PathBuf::from("/home/u/.local/bin/amplihack"),
            PathBuf::from("/tmp/build/target/debug/amplihack-hooks-scratch"),
            PathBuf::from("/home/u/.local/bin/amplihack-hooks"),
        ];

        let selected =
            deployed_hooks_binary(&deployed).expect("should find the deployed hooks bin");

        assert_eq!(
            selected,
            Path::new("/home/u/.local/bin/amplihack-hooks"),
            "must pick the file named exactly amplihack-hooks under the deployed bin dir"
        );
        assert!(
            !selected.to_string_lossy().contains("target/debug"),
            "must never select a target/debug build artifact"
        );
    }

    #[test]
    fn deployed_hooks_binary_errs_when_absent() {
        // A deployment vector with no amplihack-hooks entry is a hard error:
        // baking a bogus/guessed path would reintroduce the 127 outage.
        let deployed = vec![PathBuf::from("/home/u/.local/bin/amplihack")];

        assert!(
            deployed_hooks_binary(&deployed).is_err(),
            "absent amplihack-hooks in deployed set must be an error, not a silent guess"
        );
    }
}
