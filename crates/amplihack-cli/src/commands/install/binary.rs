//! Binary resolution and deployment for amplihack-hooks.

use super::paths::{find_binary, home_dir, is_executable};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::PathBuf;

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

    // Also copy self (the amplihack binary) if it differs from the destination.
    // Uses atomic rename-then-replace (issue #304) so it succeeds even when the
    // destination is the currently-running binary.
    if let Ok(self_exe) = std::env::current_exe() {
        let self_dst = local_bin.join("amplihack");
        if self_exe != self_dst {
            deploy_binary(&self_exe, &self_dst).with_context(|| {
                format!(
                    "failed to deploy amplihack binary to {}",
                    self_dst.display()
                )
            })?;
            deployed.push(self_dst);
        }

        let resolver_src = self_exe
            .parent()
            .map(|dir| dir.join("amplihack-asset-resolver"))
            .filter(|candidate| is_executable(candidate))
            .or_else(|| find_binary("amplihack-asset-resolver"));
        if let Some(resolver_src) = resolver_src {
            let resolver_dst = local_bin.join("amplihack-asset-resolver");
            if resolver_src != resolver_dst {
                deploy_binary(&resolver_src, &resolver_dst).with_context(|| {
                    format!(
                        "failed to deploy {} to {}",
                        resolver_src.display(),
                        resolver_dst.display()
                    )
                })?;
                deployed.push(resolver_dst);
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
