//! Binary resolution and deployment for amplihack-hooks.

use super::InstallMode;
use super::paths::{find_binary, home_dir, is_executable};
use super::release_layout::{self, ReleaseTxn};
use super::version_stamp::{VersionRelation, relation};
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
    if let Some(found) = hooks_override_or_sibling()? {
        return Ok(found);
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

/// Steps 1-2 of [`find_hooks_binary`]: the operator's override, else the
/// hooks binary shipped next to this executable (whose version is ours).
fn hooks_override_or_sibling() -> Result<Option<PathBuf>> {
    if let Some(val) = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") {
        let p = PathBuf::from(&val);
        if p.exists() {
            return Ok(Some(p));
        }
        bail!("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH is set to {p:?} but that path does not exist");
    }
    Ok(std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(HOOKS_BIN)))
        .filter(|candidate| is_executable(candidate)))
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

/// Issue #1526: an implicit re-stage fills an empty slot, and replaces an
/// occupied one only when `running` is strictly newer than the install stamp.
pub(super) fn implicit_deploy_allowed(
    dst_exists: bool,
    running: &str,
    stamp: Option<&str>,
) -> bool {
    !dst_exists || stamp.is_some_and(|s| relation(running, s) == VersionRelation::Newer)
}

/// Whether this install may write `src` to `dst`.
fn may_deploy(mode: InstallMode, stamp: Option<&str>, src: &Path, dst: &Path) -> bool {
    // Cheapest first. symlink_metadata: a dangling symlink still occupies
    // `dst`. A source that already is the destination is a no-op copy.
    mode == InstallMode::Explicit
        || implicit_deploy_allowed(dst.symlink_metadata().is_ok(), crate::VERSION, stamp)
        || same_file(src, dst)
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Issue #1526: an implicit copy that failed left the installed binary in
/// place, so the stamp must not record it at this build's version.
fn kept_after_failed_copy(mode: InstallMode, dst: &Path) -> bool {
    mode == InstallMode::Implicit && dst.symlink_metadata().is_ok()
}

const HOOKS_BIN: &str = "amplihack-hooks";
const SELF_BIN: &str = "amplihack";
const RESOLVER_BIN: &str = "amplihack-asset-resolver";

/// Whether `path` is one of the binaries [`deploy_binaries`] writes into
/// `bin_dir`, or one of their copies inside a release (issue #1954).
pub(super) fn is_deployed_binary(path: &Path, bin_dir: &Path) -> bool {
    let in_dir = path.parent() == Some(bin_dir)
        || path.parent().is_some_and(|parent| {
            parent.file_name().is_some_and(|n| n == "bin")
                && home_dir().is_ok_and(|home| {
                    release_layout::is_release_path(
                        &release_layout::releases_dir(&home.join(".amplihack")),
                        parent,
                    )
                })
        });
    in_dir
        && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            [HOOKS_BIN, SELF_BIN, RESOLVER_BIN]
                .iter()
                .any(|b| n == crate::path_conflicts::binary_filename(b))
        })
}

/// A new release for this build under `~/.amplihack/releases` (issue #1954).
pub(super) fn new_release_txn() -> Result<ReleaseTxn> {
    let amplihack_dir = home_dir()?.join(".amplihack");
    ReleaseTxn::allocate(&amplihack_dir, crate::VERSION)
}

/// Commits `txn` and turns a failed link switch into an error naming the
/// link. Nothing is rolled back: every link still names a complete release.
pub(super) fn commit_release(txn: ReleaseTxn) -> Result<release_layout::SwitchReport> {
    let root = txn.root().to_path_buf();
    let report = txn.commit()?;
    if let Some((link, reason)) = &report.failed {
        bail!(
            "release {} is complete but switching {} to it failed: {reason}. The links \
             already switched stay on the new release; run `amplihack install` again to \
             switch the rest",
            root.display(),
            link.display()
        );
    }
    Ok(report)
}

/// Deploys the binaries into a release of their own and switches their links.
/// See [`deploy_binaries_into`].
#[cfg(test)]
pub(super) fn deploy_binaries(
    mode: InstallMode,
    stamp: Option<&str>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut txn = new_release_txn()?;
    let deployed = deploy_binaries_into(&mut txn, mode, stamp)?;
    commit_release(txn)?;
    print_path_conflict_advisory();
    Ok(deployed)
}

/// Copies one binary into the release and plans the link `dst` to it.
///
/// Issue #1954: the installed file at `dst` is never rewritten. The new copy
/// goes into `txn`'s `bin/`, and `dst` becomes a symlink to it when the
/// release commits, so a process already running the old binary keeps it.
/// A source that no longer exists (an update replaced it under a running
/// process, #524) with `dst` present leaves `dst` as it is.
pub(super) fn stage_binary(txn: &mut ReleaseTxn, src: &Path, dst: &Path) -> Result<bool> {
    #[cfg(any(test, feature = "test-support"))]
    let source_metadata = src.symlink_metadata();
    #[cfg(not(any(test, feature = "test-support")))]
    let source_metadata = src.metadata();
    if let Err(e) = source_metadata
        && e.kind() == std::io::ErrorKind::NotFound
    {
        if dst.is_file() {
            return Ok(false);
        }
        return Err(e).with_context(|| format!("source binary not found: {}", src.display()));
    }
    #[cfg(any(test, feature = "test-support"))]
    {
        // Small files must meet the executable contract before copy-and-chmod.
        super::test_support::artifacts::validate_executable(src)?;
    }
    let name = dst
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("destination has no file name: {}", dst.display()))?;
    let bin_dir = txn.bin_dir();
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;
    let staged = bin_dir.join(name);
    #[cfg(any(test, feature = "test-support"))]
    if src.metadata()?.len() > super::test_support::footprint::INDEPENDENT_LIMIT {
        use super::test_support::artifacts::{ArtifactRegistry, FilesystemLinks};
        let mut artifacts = ArtifactRegistry::new();
        artifacts.register(src)?;
        artifacts.link_with(src, &staged, &bin_dir, &FilesystemLinks)?;
        txn.linked_test_binaries.push(staged.clone());
        txn.plan_link(dst, &staged)?;
        return Ok(true);
    }
    fs::copy(src, &staged)
        .with_context(|| format!("failed to copy {} to {}", src.display(), staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", staged.display()))?;
    }
    txn.plan_link(dst, &staged)?;
    Ok(true)
}

/// Copy the Rust binaries into `txn`'s `bin/` with 0o755 perms and plan the
/// `~/.local/bin` links to them (issue #1954).
/// Emits a notice if `~/.local/bin` is not in `$PATH`. The PATH conflict
/// advisory is [`print_path_conflict_advisory`], run after the commit.
/// Returns the link paths for the manifest, the hooks binary always first
/// (even when an implicit re-stage kept the installed one), and the installed
/// binaries an implicit re-stage kept. A kept slot plans no link.
pub(super) fn deploy_binaries_into(
    txn: &mut ReleaseTxn,
    mode: InstallMode,
    stamp: Option<&str>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let local_bin = super::paths::preferred_user_bin_dir()?;
    fs::create_dir_all(&local_bin)
        .with_context(|| format!("failed to create {}", local_bin.display()))?;

    let hooks_dst = local_bin.join(HOOKS_BIN);
    // Issue #1526: the stamp check vouches only for binaries at this build's
    // version. An implicit re-stage therefore replaces an installed hooks
    // binary only with the override or the sibling of this executable; one
    // found elsewhere on PATH may only fill an empty slot.
    let hooks_src = match mode {
        InstallMode::Explicit => Some(find_hooks_binary()?),
        InstallMode::Implicit => match hooks_override_or_sibling()? {
            None if hooks_dst.symlink_metadata().is_err() => Some(find_hooks_binary()?),
            found => found,
        },
    };
    let mut kept = Vec::new();
    match hooks_src {
        Some(hooks_src) if may_deploy(mode, stamp, &hooks_src, &hooks_dst) => {
            stage_binary(txn, &hooks_src, &hooks_dst).with_context(|| {
                format!(
                    "failed to deploy {} to {}",
                    hooks_src.display(),
                    hooks_dst.display()
                )
            })?;
        }
        _ => kept.push(hooks_dst.clone()),
    }

    let mut deployed = vec![hooks_dst.clone()];

    // Also copy self (the amplihack binary) and the asset-resolver. These
    // copies are best-effort: a failure here must NOT abort the
    // framework-asset (agents/skills/context) refresh that follows (issue
    // #885), but it is reported.
    if let Ok(self_exe) = std::env::current_exe() {
        let self_src = resolve_running_binary_source(&self_exe);
        let self_dst = local_bin.join(SELF_BIN);
        if !may_deploy(mode, stamp, &self_src, &self_dst) {
            kept.push(self_dst);
        } else {
            match stage_binary(txn, &self_src, &self_dst) {
                Ok(_) => deployed.push(self_dst),
                Err(e) => {
                    println!(
                        "  ⚠️  Skipped amplihack binary copy to {}: {e:#}",
                        self_dst.display()
                    );
                    tracing::warn!("amplihack binary deploy skipped: {e:#}");
                    if kept_after_failed_copy(mode, &self_dst) {
                        kept.push(self_dst);
                    }
                }
            }
        }

        // The real source (`self_src`) is used so the asset-resolver is looked
        // up next to the freshly-installed binary, not the deleted-marker path.
        // Issue #1526: as for hooks, a PATH copy may only fill an empty slot
        // during an implicit re-stage.
        let resolver_dst = local_bin.join(RESOLVER_BIN);
        #[cfg(not(any(test, feature = "test-support")))]
        let path_lookup_allowed =
            mode == InstallMode::Explicit || resolver_dst.symlink_metadata().is_err();
        #[cfg(any(test, feature = "test-support"))]
        let resolver_src = Some(super::test_support::resolver_stub()?);
        #[cfg(not(any(test, feature = "test-support")))]
        let resolver_src = self_src
            .parent()
            .map(|dir| dir.join(RESOLVER_BIN))
            .filter(|candidate| is_executable(candidate))
            .or_else(|| {
                path_lookup_allowed
                    .then(|| find_binary(RESOLVER_BIN))
                    .flatten()
            });
        if let Some(resolver_src) = resolver_src {
            if !may_deploy(mode, stamp, &resolver_src, &resolver_dst) {
                kept.push(resolver_dst);
            } else {
                match stage_binary(txn, &resolver_src, &resolver_dst) {
                    Ok(_) => deployed.push(resolver_dst),
                    Err(e) => {
                        println!(
                            "  ⚠️  Skipped amplihack-asset-resolver copy to {}: {e:#}",
                            resolver_dst.display()
                        );
                        tracing::warn!("amplihack-asset-resolver deploy skipped: {e:#}");
                        if kept_after_failed_copy(mode, &resolver_dst) {
                            kept.push(resolver_dst);
                        }
                    }
                }
            }
        }
    }

    // PATH advisory + auto-persist. Always ensure the managed profile block is
    // present so future shells prefer the user-level Rust binary, even when the
    // current PATH already mentions ~/.local/bin later.
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let in_path = std::env::split_paths(&path_var).any(|dir| dir == local_bin);
    if !in_path {
        println!(
            "  ⚠️  ~/.local/bin is not in $PATH. Add it to your shell profile:\n   \
             export PATH=\"$HOME/.local/bin:$PATH\""
        );
    }
    if let Err(e) = super::paths::ensure_local_bin_on_shell_path() {
        tracing::warn!("failed to auto-persist PATH: {e}");
    }

    Ok((deployed, kept))
}

/// Prints the PATH conflict advisory for the installed `amplihack`.
///
/// Call only after [`commit_release`]: the analysis looks at
/// `~/.local/bin/amplihack`, which is a link that names nothing new until the
/// release commits. Run before it, on a fresh home the analysis finds no
/// user-level binary for another `amplihack` on PATH to shadow, and the
/// advisory never prints.
pub(super) fn print_path_conflict_advisory() {
    let Ok(home) = home_dir() else {
        return;
    };
    if let Ok(current_exe) = std::env::current_exe()
        && let Ok(report) =
            crate::path_conflicts::analyze_current_process_path_conflicts(home, current_exe)
        && let Some(warning) = path_conflict_warning_after_install(&report)
    {
        println!("{warning}");
    }
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
pub(crate) fn resolve_running_binary_source(current_exe: &Path) -> PathBuf {
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
///
/// Install code stages into a release instead (issue #1954); this direct
/// copy is kept for the #885 tests of the deleted-source rules.
#[cfg(test)]
pub(super) fn deploy_self_binary(local_bin: &Path, current_exe: &Path) -> Option<PathBuf> {
    use super::filesystem::deploy_binary;

    let self_src = resolve_running_binary_source(current_exe);
    let self_dst = local_bin.join(SELF_BIN);
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
        // The npx / pnpm dlx / bunx shim running this install shadows
        // ~/.local/bin only until that command exits, and the install already
        // says so (#1480). Judge the PATH the user is left with instead:
        // whatever remains once it is gone.
        let is_transient = |path: &std::path::Path| {
            super::stale_wrappers::npm_launcher_path(path)
                == Some(super::stale_wrappers::NpmLauncherKind::Transient)
        };
        let without_shims;
        let resolution = if is_transient(&resolution.resolved.path) {
            let Some(remaining) =
                resolution.without_candidates(|candidate| is_transient(&candidate.path))
            else {
                continue;
            };
            without_shims = remaining;
            &without_shims
        } else {
            resolution
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
    // Issue #1496: a persistent launcher for our own npm wrapper (`npm
    // install -g`, a project dependency) is not a stale or unknown program,
    // but it does decide which binary `amplihack` runs. Say so plainly.
    if binary_name == "amplihack" && is_persistent_npm_launcher(&resolution.resolved.path) {
        warning.push_str(&format!(
            "  ⚠️  `amplihack` at {} is the npm launcher for this package (e.g. `npm install -g @rysweet/amplihack-rs`) and comes before {} on PATH.\n",
            resolution.resolved.path.display(),
            preferred_path.display()
        ));
        append_npm_launcher_guidance(warning);
        return;
    }

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

fn is_persistent_npm_launcher(path: &std::path::Path) -> bool {
    super::stale_wrappers::npm_launcher_path(path)
        == Some(super::stale_wrappers::NpmLauncherKind::Persistent)
}

/// What a persistent launcher for our own npm wrapper means and how to prefer
/// the `~/.local/bin` copy instead (issue #1496). Shared by the shadow and the
/// ambiguity branches: `amplihack update` re-runs install with `~/.local/bin`
/// moved first, so the same launcher shows up as an ambiguity there.
fn append_npm_launcher_guidance(warning: &mut String) {
    warning.push_str(
        "     Through that launcher `amplihack` runs the npm wrapper and its own cached release binary, not the copy `amplihack update` maintains in ~/.local/bin.\n",
    );
    warning.push_str("     To use ~/.local/bin/amplihack directly, do one of the following:\n");
    warning.push_str(
        "       1. Remove the launcher with the package manager that installed it, e.g.  npm uninstall -g @rysweet/amplihack-rs  (a project's node_modules/.bin: remove the dependency there)\n",
    );
    warning.push_str(
        "       2. Reorder PATH so ~/.local/bin comes first:  export PATH=\"$HOME/.local/bin:$PATH\"\n",
    );
}

fn append_ambiguity_warning(
    warning: &mut String,
    binary_name: &str,
    resolution: &crate::path_conflicts::BinaryResolution,
) {
    warning.push_str(&format!(
        "  ⚠️  Multiple distinct `{binary_name}` binaries are on PATH:\n"
    ));
    let mut launcher = None;
    let mut others = false;
    let preferred = resolution
        .preferred_user_candidate
        .as_ref()
        .map(|candidate| candidate.path.as_path());
    for candidate in &resolution.canonical_candidates {
        if binary_name == "amplihack" && is_persistent_npm_launcher(&candidate.path) {
            launcher = Some(candidate.path.as_path());
            warning.push_str(&format!(
                "     - {}  (this package's npm launcher, e.g. `npm install -g`; not stale)\n",
                candidate.path.display()
            ));
        } else {
            others |= Some(candidate.path.as_path()) != preferred;
            warning.push_str(&format!("     - {}\n", candidate.path.display()));
        }
    }
    if launcher.is_some() {
        warning.push_str("     Whichever comes first on PATH in your shell wins.\n");
        append_npm_launcher_guidance(warning);
    }
    // Any other candidate may really be stale; the launcher note must not
    // swallow that advice.
    if launcher.is_none() || others {
        warning.push_str(
            "     Remove stale candidates or reorder PATH so the intended user-level install resolves first.\n",
        );
    }
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

/// The hooks-binary path that belongs in a DURABLE config file — `settings.json`,
/// a plugin manifest, anything that outlives the process that wrote it.
///
/// `find_hooks_binary()` prefers a sibling of the running executable. That is
/// right for *finding* a binary to deploy and wrong for *recording* one: run
/// `amplihack install` from a build tree and it returns
/// `<target>/debug/amplihack-hooks`, which is a cargo artifact directory that
/// gets cleaned. Every hook then fails to exec.
///
/// Issue #911 documented this after every hook started exiting 127, and the fix
/// was applied to the Copilot plugin registration — but `ensure_settings_json`
/// kept receiving the transient path, so Claude Code's hooks were still wired
/// into a build directory. One such config pointed at
/// `/tmp/amplihack-precommit-target/debug/amplihack-hooks`, the shared
/// pre-commit target dir, and broke the moment that cache was reclaimed.
///
/// Prefer the deployed copy under the user's bin dir; fall back to discovery
/// only when nothing has been deployed yet.
pub(super) fn stable_hooks_binary() -> Result<PathBuf> {
    let deployed = super::paths::preferred_user_bin_dir()
        .ok()
        .map(|dir| dir.join("amplihack-hooks"))
        .filter(|candidate| is_executable(candidate));
    Ok(choose_durable_hooks_path(deployed, find_hooks_binary()?))
}

/// Pure seam for [`stable_hooks_binary`], so the preference is testable without
/// a real HOME, a real deploy, or control over `current_exe()`.
pub(super) fn choose_durable_hooks_path(deployed: Option<PathBuf>, discovered: PathBuf) -> PathBuf {
    match deployed {
        Some(deployed) => deployed,
        // Nothing deployed yet. `discovered` is the best we have, but say so
        // when it is a build artifact: a silent transient path in settings.json
        // is exactly the failure this function exists to prevent, and the next
        // person deserves to know why their hooks stopped working.
        None => {
            if looks_like_build_artifact(&discovered) {
                tracing::warn!(
                    path = %discovered.display(),
                    "recording a hooks path that lives in a build directory; it will \
                     break when that directory is cleaned (issue #911). Run \
                     `amplihack install` to deploy a stable copy."
                );
            }
            discovered
        }
    }
}

/// The amplihack binary the `statusLine` entry records (issue #1745): the
/// deployed copy under the user's bin dir when there is one, as for the
/// hooks (issue #911), else the running binary. Either is refused if other
/// users can replace it. A result in a build directory is recorded, with a
/// warning, because the entry stops working once that directory is cleaned.
pub(super) fn stable_self_binary() -> Result<PathBuf> {
    let deployed = super::paths::preferred_user_bin_dir()
        .ok()
        .map(|dir| dir.join(SELF_BIN))
        .filter(|candidate| is_executable(candidate));
    let exe = match deployed {
        Some(deployed) => durable_link_or_target(&deployed)?,
        None => crate::commands::profile::statusline_binary()?,
    };
    if let Some(warning) = build_output_warning(&exe) {
        println!("  ⚠️  {warning}");
    }
    Ok(exe)
}

/// `deployed`, checked like any status-line binary. Issue #1954: when it is a
/// link into a release, the durable entry names the link, never the release
/// path, which a later install may remove once it is unused. The link's own
/// directory gets the same owner and write check as the release it names.
fn durable_link_or_target(deployed: &Path) -> Result<PathBuf> {
    let target = crate::commands::profile::statusline_binary_at(deployed)?;
    let is_link = deployed
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink());
    let into_release = home_dir().is_ok_and(|home| {
        release_layout::is_release_path(
            &release_layout::releases_dir(&home.join(".amplihack")),
            &target,
        )
    });
    if !(is_link && into_release) {
        return Ok(target);
    }
    if let Some(dir) = deployed.parent() {
        crate::commands::profile::statusline_binary_at(dir)?;
    }
    Ok(deployed.to_path_buf())
}

/// The warning for a `statusLine` entry that names `exe`, if it needs one.
pub(super) fn build_output_warning(exe: &Path) -> Option<String> {
    looks_like_build_artifact(exe).then(|| {
        format!(
            "statusLine names {}, a build output; the status line stops working when that \
             directory is cleaned",
            exe.display()
        )
    })
}

/// Does this path live inside a cargo build directory?
///
/// Matched on whole path components, not a substring: a user whose home is
/// `/home/debug` must not be flagged.
pub(super) fn looks_like_build_artifact(path: &Path) -> bool {
    let mut saw_profile = false;
    for component in path.components() {
        let part = component.as_os_str();
        if part == "debug" || part == "release" {
            saw_profile = true;
        }
        if part == "target" || part.to_string_lossy().contains("target-") {
            return true;
        }
    }
    saw_profile
}
