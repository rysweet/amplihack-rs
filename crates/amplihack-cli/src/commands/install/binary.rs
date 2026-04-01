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
        bail!(
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH is set to {:?} but that path does not exist",
            p
        );
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
         cargo install --git https://github.com/rysweet/amplihack amplihack-hooks\n  \
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
    let home = home_dir()?;
    let local_bin = home.join(".local").join("bin");
    fs::create_dir_all(&local_bin)
        .with_context(|| format!("failed to create {}", local_bin.display()))?;

    let hooks_src = find_hooks_binary()?;
    let hooks_dst = local_bin.join("amplihack-hooks");

    fs::copy(&hooks_src, &hooks_dst).with_context(|| {
        format!(
            "failed to copy {} to {}",
            hooks_src.display(),
            hooks_dst.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hooks_dst, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", hooks_dst.display()))?;
    }

    let mut deployed = vec![hooks_dst.clone()];

    // Also copy self (the amplihack binary) if it differs from the destination
    if let Ok(self_exe) = std::env::current_exe() {
        let self_dst = local_bin.join("amplihack");
        if self_exe != self_dst {
            fs::copy(&self_exe, &self_dst).with_context(|| {
                format!("failed to copy amplihack binary to {}", self_dst.display())
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    fs::set_permissions(&self_dst, std::fs::Permissions::from_mode(0o755))
                {
                    tracing::warn!(
                        "failed to set executable bit on {}: {}",
                        self_dst.display(),
                        e
                    );
                }
            }
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
                fs::copy(&resolver_src, &resolver_dst).with_context(|| {
                    format!(
                        "failed to copy {} to {}",
                        resolver_src.display(),
                        resolver_dst.display()
                    )
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&resolver_dst, std::fs::Permissions::from_mode(0o755))
                        .with_context(|| format!("failed to chmod {}", resolver_dst.display()))?;
                }
                deployed.push(resolver_dst);
            }
        }
    }

    // PATH advisory
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let in_path = std::env::split_paths(&path_var).any(|dir| dir == local_bin);
    if !in_path {
        println!(
            "  ⚠️  ~/.local/bin is not in $PATH. Add it to your shell profile:\n   \
             export PATH=\"$HOME/.local/bin:$PATH\""
        );
    }

    if let Some(amplihack_dst) = deployed
        .iter()
        .find(|path| path.file_name().and_then(|value| value.to_str()) == Some("amplihack"))
        && let Some(on_path) = find_binary("amplihack")
        && on_path != *amplihack_dst
    {
        println!(
            "  ⚠️  `amplihack` currently resolves to {} instead of the Rust binary at {}.",
            on_path.display(),
            amplihack_dst.display()
        );
        println!(
            "     Use {} directly for the Rust CLI, or move ~/.local/bin ahead of older amplihack installs.",
            amplihack_dst.display()
        );
    }

    Ok(deployed)
}
