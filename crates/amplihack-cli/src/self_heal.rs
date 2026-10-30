//! Startup self-heal: re-stage framework assets when the binary version
//! disagrees with the on-disk install stamp.
//!
//! ## Why this exists
//!
//! PR #488 added a post-install hook to `amplihack update` so that after the
//! binary is replaced, framework assets in `~/.amplihack` are re-staged.
//! That hook only fires when the **old** running binary already contains the
//! post-install code path. Users on pre-#488 versions doing
//! `amplihack update` get a swapped binary but no asset re-stage on next
//! launch — the new binary has no idea the prior install is stale.
//!
//! This module closes the gap by version-stamping every successful install
//! into `~/.amplihack/.installed-version` and comparing the stamp against
//! [`crate::VERSION`] on every startup. If the stamp is missing or stale,
//! the install procedure is invoked automatically and the user sees a
//! one-line notice on stderr.
//!
//! ## Never an implicit downgrade (#1526)
//!
//! When the running binary is older than the stamp, nothing is installed and
//! one warning goes to stderr. Guard #1 runs before the bundle check, Guard #2
//! under the install lock; explicit `amplihack install` is never guarded.
//!
//! ## Failure mode
//!
//! Per the project's Zero-BS philosophy, install failures **propagate**
//! and are surfaced loudly by the binary entrypoint. There is no silent
//! fallback to "continue with stale assets" — the user gets a clear error
//! and a non-zero exit.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;

use crate::commands::install::bundle_compat_cache;
use crate::commands::install::paths::{home_dir, sanitize_path};
use crate::commands::install::version_stamp::{self, VersionRelation};

/// Env var that fully disables the auto-restage check.
///
/// Useful for CI, integration tests, and any context where install
/// side effects are unwanted. Any non-empty value disables.
const SKIP_ENV: &str = "AMPLIHACK_SKIP_AUTO_INSTALL";

/// Subcommands that must NOT trigger auto-install.
///
/// - `install`/`uninstall`/`update`: would recurse or undo the user's intent.
/// - `completions`/`help`/`doctor`: read-only/diagnostic; should stay fast.
/// - `agent-binary`: runs inside `$(...)` to hand the agent binary to a
///   detached launch (issue #1525); an install there would be a side effect
///   of reading a value, and its output would land on the caller's terminal.
/// - `config`: reads and writes user defaults, often inside `$(...)`
///   (issue #1606); the same reasoning as `agent-binary`.
const SKIP_SUBCOMMANDS: &[&str] = &[
    "install",
    "uninstall",
    "update",
    "completions",
    "doctor",
    "help",
    "agent-binary",
    "config",
];

/// Top-level flags that short-circuit clap and should not pay for an
/// install check.
const SKIP_FLAGS: &[&str] = &["--help", "-h", "--version", "-V"];

/// Filename of the inter-process advisory lock guarding the installer
/// critical section (issue #502 R5). Sits next to the install stamp under
/// `~/.amplihack/`.
const INSTALL_LOCK_FILE: &str = ".install.lock";

/// Issue #1526: set once this process has emitted the implicit-downgrade
/// warning, so the launch bootstrap does not print it a second time.
static DOWNGRADE_WARNED: AtomicBool = AtomicBool::new(false);

/// Public entrypoint, called from `bins/amplihack/src/main.rs` immediately
/// after the update-notice check and before `commands::dispatch`.
///
/// Returns `Ok(())` when no install was needed or after a successful one.
/// Returns `Err` if the install fails — callers are expected to surface
/// the error and abort.
pub fn ensure_assets_match_binary_version(args: &[OsString]) -> Result<()> {
    // Issue #502 R7: narrow HOME-unset carve-out. Probe `HOME` directly
    // (not via `paths::home_dir()`) so we can distinguish "no home" from
    // any other path-helper error and skip gracefully without swallowing
    // unrelated failures. This is the ONLY intentionally silent path in
    // this module.
    if std::env::var_os("HOME")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        tracing::debug!("self_heal: HOME unset; skipping (graceful carve-out)");
        return Ok(());
    }

    ensure_assets_match_binary_version_with(args, &mut std::io::stderr(), || {
        crate::commands::install::repair_framework_assets()
    })
}

/// Use parsed CLI provider intent for startup repair, ahead of inherited hints.
/// The scoped selection restores the caller's provider after the check.
pub fn ensure_assets_for_command(args: &[OsString], command: &crate::Commands) -> Result<()> {
    let provider = match command {
        crate::Commands::Launch { .. } | crate::Commands::Claude { .. } => "claude",
        crate::Commands::Copilot { .. } => "copilot",
        crate::Commands::Codex { .. } => "codex",
        crate::Commands::Amplifier { .. } => "amplifier",
        crate::Commands::RustyClawd { .. } => "rustyclawd",
        // Generic commands repair generic assets; direct install retains its
        // separate all-provider semantics and is in the startup skip list.
        _ => "",
    };
    crate::freshness::with_provider(provider, || ensure_assets_match_binary_version(args))
}

/// Resolve ordinary Codex launches before asset repair registers native resources.
/// Other providers retain their existing startup order.
pub fn resolve_startup_client(command: &crate::Commands) -> Result<()> {
    if matches!(
        command,
        crate::Commands::Codex {
            docker: false,
            append: None,
            auto: false,
            ..
        }
    ) {
        crate::bootstrap::ensure_tool_available(
            "codex",
            amplihack_utils::launch_target::OverrideOrigin::User,
        )?;
    }
    Ok(())
}

/// Resolve the path to the install advisory lock file. Mirrors
/// `version_stamp::installed_version_path` so both files live side-by-side.
fn install_lock_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(INSTALL_LOCK_FILE))
}

/// The five-line implicit-downgrade warning (issue #1526).
///
/// `installed` is the stamp's framework version, the one the refusal compared
/// against. `installed_binaries` is its `binaries:` line (`None`: unknown).
///
/// When the two agree, every binary in `~/.local/bin` is at that version and
/// the `installed:` line gives it beside the path. When they differ, an
/// implicit re-stage kept *some* binaries at the `binaries:` version and may
/// have upgraded others: `deploy_binaries` decides each one separately, so
/// `~/.local/bin/amplihack` can be at the framework version while
/// `amplihack-hooks` stays behind. The stamp does not say which, so no
/// version is attached to the path; both are printed after it, each
/// labelled for what it describes.
fn format_downgrade_warning(
    running_exe: Option<&Path>,
    installed_bin: &Path,
    running: &str,
    installed: &str,
    installed_binaries: Option<&str>,
) -> String {
    let running_path = match running_exe {
        Some(exe) => sanitize_path(exe),
        None => "<unknown>".to_string(),
    };
    let installed_version = match installed_binaries {
        Some(binaries) if binaries == installed => format!(" (v{installed})"),
        Some(binaries) => format!("; framework v{installed}; binaries kept from v{binaries}"),
        None => format!("; framework v{installed}; binaries kept from an unknown version"),
    };
    format!(
        "amplihack: warning: not re-staging framework assets — \
         this amplihack is older than the installed one.\n\
         \x20 running:   {running_path} (v{running})\n\
         \x20 installed: {}{installed_version}\n\
         \x20 Installed binaries and ~/.claude/settings.json were left unchanged.\n\
         \x20 Run `amplihack install` to replace them with this build.\n",
        sanitize_path(installed_bin)
    )
}

/// Write the implicit-downgrade warning to `notice`, at most once per process
/// and never under `AMPLIHACK_SKIP_AUTO_INSTALL`. `installed` is the stamp's
/// framework version; the binaries' version is read from the stamp here.
///
/// Best effort: the refusal holds whether or not the warning is written, so
/// a failure is logged at debug and never fails the command.
pub(crate) fn warn_implicit_downgrade_once<W: Write>(notice: &mut W, installed: &str) {
    if env_bypass_set() || DOWNGRADE_WARNED.load(AtomicOrdering::Relaxed) {
        return;
    }
    let home = match home_dir() {
        Ok(home) => home,
        Err(e) => {
            tracing::debug!("self_heal: cannot resolve the installed binary; no warning: {e:#}");
            return;
        }
    };
    if DOWNGRADE_WARNED.swap(true, AtomicOrdering::Relaxed) {
        return;
    }
    let running_exe = std::env::current_exe()
        .inspect_err(|e| tracing::debug!("self_heal: cannot resolve the running executable: {e}"))
        .ok()
        .map(|exe| crate::commands::install::resolve_running_binary_source(&exe));
    let installed_bin = crate::path_conflicts::preferred_user_bin(&home)
        .join(crate::path_conflicts::binary_filename("amplihack"));
    // The stamp's `binaries:` line: the version of the binaries an implicit
    // re-stage kept. Unreadable is reported as unknown, never guessed.
    let installed_binaries = version_stamp::read_binaries_version()
        .inspect_err(|e| tracing::debug!("self_heal: cannot read the binaries version: {e:#}"))
        .ok()
        .flatten();
    let block = format_downgrade_warning(
        running_exe.as_deref(),
        &installed_bin,
        crate::VERSION,
        installed,
        installed_binaries.as_deref(),
    );
    if let Err(e) = notice.write_all(block.as_bytes()) {
        tracing::debug!("self_heal: failed to emit implicit-downgrade warning: {e}");
    }
}

/// Test hook for [`DOWNGRADE_WARNED`]. Callers hold
/// `crate::test_support::env_lock`, so the flag is never raced.
#[cfg(test)]
pub(crate) fn set_downgrade_warned(warned: bool) {
    DOWNGRADE_WARNED.store(warned, AtomicOrdering::Relaxed);
}

/// Run `body` while holding an exclusive advisory `flock` on the install
/// lock file (issue #502 R5). The lock serialises concurrent
/// `ensure_assets_match_binary_version` callers within a host so two
/// processes never run the installer body at the same time.
///
/// The lock file is created with default permissions if missing — its
/// contents are never inspected, only its inode is used as the lock token,
/// so it does not need 0o600 like the stamp.
fn with_install_lock<T, F: FnOnce() -> Result<T>>(body: F) -> Result<T> {
    let lock_path = install_lock_path()?;
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("failed to open install lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock_file)
        .with_context(|| format!("failed to acquire install lock {}", lock_path.display()))?;
    let result = body();
    // Best-effort unlock; the kernel will release on drop regardless.
    let _ = FileExt::unlock(&lock_file);
    result
}

/// Decision-logic core, factored out for testability.
///
/// Mirrors the closure-injection pattern used by
/// `crate::update::post_install::run_post_update_install` so unit tests
/// can verify the decision tree without running a real install or touching
/// stderr.
pub(crate) fn ensure_assets_match_binary_version_with<W, F>(
    args: &[OsString],
    notice: &mut W,
    install_fn: F,
) -> Result<()>
where
    W: Write,
    F: FnOnce() -> Result<()>,
{
    if args_should_skip(args) {
        tracing::debug!("self_heal: skipped (subcommand or flag in skip list)");
        return Ok(());
    }

    // Issue #502 R7: if `HOME` is unset (or empty), skip gracefully. The
    // public entrypoint already performs this check; we re-check here so
    // unit tests that drive the `_with` variant directly receive the same
    // behaviour. This is the only intentionally silent path in the module.
    if std::env::var_os("HOME")
        .map(|v| v.is_empty())
        .unwrap_or(true)
    {
        tracing::debug!("self_heal: HOME unset; skipping (graceful carve-out)");
        return Ok(());
    }

    let stamp = version_stamp::read_installed_version().context("reading install version stamp")?;
    let expected = crate::VERSION;

    // Issue #1526 Guard #1: before the #1271 bundle check (a re-stage too)
    // and the lock, so a refusal parses no recipe YAML and writes nothing.
    // Bypass keeps its own branch below: it never installs.
    if !env_bypass_set()
        && let Some(installed) = stamp.as_deref()
        && version_stamp::relation(expected, installed) == VersionRelation::Older
    {
        // debug: tracing writes to stdout; the warning goes to `notice`.
        tracing::debug!(
            "self_heal: binary {expected} is older than install stamp {installed}; refusing implicit re-stage"
        );
        warn_implicit_downgrade_once(notice, installed);
        return Ok(());
    }

    // Issue #502 R6: when bypass is set AND there is a real skew, emit a
    // single-line stderr diagnostic so CI logs surface the drift. Both
    // values are constant or regex-validated semver, so plain formatting
    // is safe (no control-char injection surface).
    let stamp_match = stamp.as_deref() == Some(expected);
    // Issue #1271: this verdict is needed on every code path below — the
    // bypass diagnostic prints it, and a matching stamp is *not* on its own
    // sufficient to skip the re-stage (a stale installed bundle with a
    // current stamp must still repair). So it cannot be reordered away.
    // Instead it is memoised on a stat fingerprint of the bundle inputs:
    // ~58 KB of recipe YAML is parsed only when the bundle actually changed.
    let installed_bundle_issue = installed_bundle_compatibility_issue_cached()
        .context("checking installed framework bundle compatibility")?;

    if env_bypass_set() {
        if !stamp_match || installed_bundle_issue.is_some() {
            let stamp_str = stamp.as_deref().unwrap_or("<missing>");
            let bundle_status = installed_bundle_issue.as_deref().unwrap_or("compatible");
            writeln!(
                notice,
                "amplihack: AMPLIHACK_SKIP_AUTO_INSTALL set; skipping re-stage \
                 (stamp={stamp_str} current={expected}; installed_bundle={bundle_status:?})"
            )
            .context("emitting bypass diagnostic")?;
        }
        tracing::debug!("self_heal: skipped via {SKIP_ENV}");
        return Ok(());
    }

    if stamp_match && installed_bundle_issue.is_none() {
        tracing::debug!("self_heal: stamp matches binary version {expected}");
        return Ok(());
    }

    if let Some(issue) = &installed_bundle_issue {
        tracing::info!(
            "self_heal: installed framework bundle incompatible despite stamp={:?}; re-staging assets: {issue}",
            stamp.as_deref()
        );
    } else {
        match stamp.as_deref() {
            Some(prior) => {
                tracing::info!(
                    "self_heal: install stamp {prior} != binary {expected}; re-staging assets"
                );
            }
            None => {
                tracing::info!("self_heal: install stamp missing; staging assets for {expected}");
            }
        }
    }

    // Issue #502 R5: serialise installer + stamp write across processes
    // via an advisory `flock` on `~/.amplihack/.install.lock`. The second
    // waiter re-reads the stamp inside the critical section; if the first
    // winner already wrote the up-to-date stamp, the waiter exits without
    // re-running the installer.
    with_install_lock(|| {
        // Re-check after acquiring the lock — the previous holder may
        // have already brought the stamp current.
        // Quiet: the read above has already reported a malformed stamp.
        let stamp_now = version_stamp::read_installed_version_quiet()
            .context("re-reading install stamp under lock")?;
        // Issue #1526 Guard #2: a newer install may have finished while this
        // process waited on the lock.
        if let Some(installed) = stamp_now.as_deref()
            && version_stamp::relation(expected, installed) == VersionRelation::Older
        {
            tracing::debug!(
                "self_heal: install stamp became {installed} while waiting on the install lock; \
                 binary {expected} refuses implicit re-stage"
            );
            warn_implicit_downgrade_once(notice, installed);
            return Ok(());
        }
        // Deliberately uncached: a concurrent installer may have rewritten
        // the bundle moments ago, and this is the decision that guards
        // against re-running the installer needlessly.
        let installed_bundle_issue_now = installed_bundle_compatibility_issue_uncached()
            .context("re-checking installed framework bundle compatibility under lock")?;
        if stamp_now.as_deref() == Some(expected) && installed_bundle_issue_now.is_none() {
            tracing::debug!(
                "self_heal: stamp and installed bundle brought current by concurrent installer; nothing to do"
            );
            return Ok(());
        }
        // The install writes the stamp itself (issue #1526: not after a
        // refused binary deploy, which only it knows about).
        install_fn().context("running install during startup self-heal")?;
        // The bundle just changed under us; drop the memoised verdict so the
        // next invocation revalidates once and re-primes the cache.
        if let Ok(path) = bundle_compat_cache_path() {
            bundle_compat_cache::invalidate(&path);
        }
        writeln!(
            notice,
            "amplihack: framework assets re-staged for v{expected}"
        )
        .context("emitting self-heal notice")?;
        Ok(())
    })
}

/// Absolute path of the installed bundle staged under `~/.amplihack`.
fn installed_bundle_path() -> Result<PathBuf> {
    let stamp_path = version_stamp::installed_version_path()?;
    let install_root = stamp_path
        .parent()
        .context("install version stamp path has no parent directory")?;
    Ok(install_root.join("amplifier-bundle"))
}

/// Absolute path of the bundle-compatibility cache, a sibling of the stamp.
fn bundle_compat_cache_path() -> Result<PathBuf> {
    let stamp_path = version_stamp::installed_version_path()?;
    let install_root = stamp_path
        .parent()
        .context("install version stamp path has no parent directory")?;
    Ok(install_root.join(bundle_compat_cache::CACHE_FILE))
}

/// Issue #1271: memoised on a stat fingerprint of the bundle inputs, so the
/// steady-state launch pays a handful of `lstat` calls instead of parsing
/// ~58 KB of recipe YAML.
fn installed_bundle_compatibility_issue_cached() -> Result<Option<String>> {
    let bundle = installed_bundle_path()?;
    let cache_path = bundle_compat_cache_path()?;
    Ok(bundle_compat_cache::compatibility_issue_cached(
        &bundle,
        &cache_path,
    ))
}

fn installed_bundle_compatibility_issue_uncached() -> Result<Option<String>> {
    let bundle = installed_bundle_path()?;
    Ok(bundle_compat_cache::compatibility_issue_uncached(&bundle))
}

/// True when `AMPLIHACK_SKIP_AUTO_INSTALL` is set to any non-empty value.
fn env_bypass_set() -> bool {
    std::env::var_os(SKIP_ENV)
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Fast pre-clap scan of `args` to determine whether the user's command
/// should bypass the self-heal check.
///
/// We deliberately avoid calling clap here — clap parsing happens after
/// this function returns, and asking clap to parse twice would risk
/// double-erroring on bad args.
fn args_should_skip(args: &[OsString]) -> bool {
    // No subcommand at all -> clap will print help. Skip.
    if args.len() < 2 {
        return true;
    }

    for arg in args.iter().skip(1) {
        let Some(s) = arg.to_str() else {
            // Non-UTF8 arg: cannot be one of the ASCII skip tokens, but
            // could still be a positional value. Keep scanning.
            continue;
        };
        if SKIP_FLAGS.contains(&s) {
            return true;
        }
    }

    let positionals: Vec<&str> = args
        .iter()
        .skip(1)
        .filter_map(|arg| arg.to_str())
        .filter(|arg| !arg.starts_with('-'))
        .collect();

    if matches!(positionals.as_slice(), ["hygiene", "artifact-guard", ..]) {
        return true;
    }
    // `orch helper <sub>` commands are pure stdin->stdout text-transform
    // primitives (extract-json, extract-field, normalise-type,
    // normalise-verdict, normalise-loop-verdict, reclassify-task-type,
    // workflow-log-inventory, ...).
    // They are invoked *inside* recipe bash pipelines whose output is parsed
    // downstream (issue #1062). A self-heal install would emit its banner onto
    // stdout and corrupt the very pipe these helpers exist to make robust, so
    // none of them may trigger auto-install.
    if matches!(positionals.as_slice(), ["orch", "helper", ..]) {
        return true;
    }
    // `profile statusline` is the status-line wrapper Claude Code runs after
    // every turn (issue #1569). Its stdout is the status line and it must stay
    // fast and silent, so it never triggers auto-install.
    if matches!(positionals.as_slice(), ["profile", "statusline", ..]) {
        return true;
    }

    // First non-flag positional is the subcommand candidate.
    for arg in args.iter().skip(1) {
        let Some(s) = arg.to_str() else { continue };
        if s.starts_with('-') {
            continue;
        }
        return SKIP_SUBCOMMANDS.contains(&s);
    }

    // No positional found (only flags). Skip — nothing meaningful to run.
    true
}

/// Test-only: this module's tests share the crate-wide
/// `crate::test_support::env_lock` to coordinate with other env-mutating
/// tests (e.g., `nesting::tests`, `version_stamp::tests`,
/// `copilot_setup::tests`).
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// Save/restore guard for `HOME` and `AMPLIHACK_SKIP_AUTO_INSTALL`.
    /// Uses the crate-wide [`crate::test_support::env_lock`] so that this
    /// module's tests cannot race tests in other modules that mutate the
    /// same env vars.
    struct EnvGuard {
        prior_home: Option<OsString>,
        prior_skip: Option<OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(home: &std::path::Path, skip: Option<&str>) -> Self {
            let lock = crate::test_support::env_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let prior_home = std::env::var_os("HOME");
            let prior_skip = std::env::var_os(SKIP_ENV);
            // SAFETY: edition 2024 requires unsafe; serialized via env_lock.
            unsafe {
                std::env::set_var("HOME", home);
                match skip {
                    Some(v) => std::env::set_var(SKIP_ENV, v),
                    None => std::env::remove_var(SKIP_ENV),
                }
            }
            set_downgrade_warned(false);
            EnvGuard {
                prior_home,
                prior_skip,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: serialized via ENV_LOCK held in self._lock.
            unsafe {
                match self.prior_home.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
                match self.prior_skip.take() {
                    Some(v) => std::env::set_var(SKIP_ENV, v),
                    None => std::env::remove_var(SKIP_ENV),
                }
            }
        }
    }

    fn args(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(|s| OsString::from(*s)).collect()
    }

    /// Helper: build an installer closure that records invocation count
    /// and writes the given version to the stamp on call.
    fn counting_installer<'a>(
        counter: &'a Cell<u32>,
        version: &'static str,
    ) -> impl FnOnce() -> Result<()> + 'a {
        move || {
            counter.set(counter.get() + 1);
            version_stamp::write_installed_version(version)?;
            Ok(())
        }
    }

    fn stale_smart_orchestrator() -> &'static str {
        r#"name: "smart-orchestrator"
description: "stale v1.5.1 smart task orchestrator"
steps:
  - id: "parse-decomposition"
    type: "shell"
    command: |
      HELPER="$(amplihack resolve-bundle-asset helper-path)"
      python3 - <<'PY'
      import importlib
      helper = importlib.import_module("orch_helper")
      helper.parse_decomposition()
      PY
"#
    }

    fn write_stale_installed_smart_orchestrator(home: &Path) {
        let recipes = home.join(".amplihack/amplifier-bundle/recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(
            recipes.join("smart-orchestrator.yaml"),
            stale_smart_orchestrator(),
        )
        .unwrap();
    }

    fn write_compatible_installed_bundle(home: &Path) {
        let recipes = home.join(".amplihack/amplifier-bundle/recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(
            recipes.join("smart-orchestrator.yaml"),
            r#"name: "smart-orchestrator"
description: "Composable smart task orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
"#,
        )
        .unwrap();
        for recipe in crate::commands::install::bundle_compat::REQUIRED_SMART_RECIPES {
            fs::write(
                recipes.join(format!("{recipe}.yaml")),
                format!(
                    "name: \"{recipe}\"\nsteps:\n  - id: smoke\n    type: bash\n    command: 'true'\n"
                ),
            )
            .unwrap();
        }
        fs::write(
            recipes.join("_recipe_manifest.json"),
            r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-execute-routing": "11612506ae846a47",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
        )
        .unwrap();
    }

    #[test]
    fn stamp_mismatch_triggers_install() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version("0.8.55").unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 1, "installer should run on mismatch");
        let notice = String::from_utf8(buf).unwrap();
        assert!(
            notice.contains(&format!(
                "framework assets re-staged for v{}",
                crate::VERSION
            )),
            "unexpected notice: {notice}"
        );
        assert_eq!(
            version_stamp::read_installed_version().unwrap().as_deref(),
            Some(crate::VERSION),
            "stamp should be rewritten to current version"
        );
    }

    #[test]
    fn stamp_match_skips_install() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version(crate::VERSION).unwrap();
        write_compatible_installed_bundle(tmp.path());

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0, "installer must not run when stamp matches");
        assert!(buf.is_empty(), "no notice when stamp matches");
    }

    /// Issue #1271: the steady-state launch — stamp current, bundle healthy,
    /// bypass unset — must not read or parse a single recipe file. Asserting
    /// the read count rather than the return value is the point: the verdict
    /// was already correct before this change; the cost was not.
    #[test]
    fn warm_launch_with_matching_stamp_parses_no_recipe_yaml() {
        use crate::commands::install::bundle_compat::read_counter;

        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version(crate::VERSION).unwrap();
        write_compatible_installed_bundle(tmp.path());

        // First launch primes the cache and does pay the parse.
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        let (_, cold_reads) = read_counter::count_reads(|| {
            ensure_assets_match_binary_version_with(
                &args(&["amplihack", "launch"]),
                &mut buf,
                counting_installer(&calls, crate::VERSION),
            )
        });
        assert!(
            cold_reads > 0,
            "the first launch after a re-stage must actually validate the bundle"
        );

        // Every subsequent launch must be free of bundle I/O.
        let mut buf = Vec::new();
        let (result, warm_reads) = read_counter::count_reads(|| {
            ensure_assets_match_binary_version_with(
                &args(&["amplihack", "launch"]),
                &mut buf,
                counting_installer(&calls, crate::VERSION),
            )
        });
        result.expect("warm launch must succeed");

        assert_eq!(
            warm_reads, 0,
            "a launch with a current stamp and an unchanged bundle must not read recipe YAML"
        );
        assert_eq!(calls.get(), 0, "installer must not run when stamp matches");
        assert!(buf.is_empty(), "no notice when stamp matches");
    }

    /// The cache must never mask a bundle that went stale after being cached.
    #[test]
    fn cached_healthy_bundle_that_goes_stale_still_triggers_repair() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version(crate::VERSION).unwrap();
        write_compatible_installed_bundle(tmp.path());

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("healthy bundle launch must succeed");
        assert_eq!(calls.get(), 0, "healthy bundle needs no repair");

        // Bundle rots underneath the primed cache.
        write_stale_installed_smart_orchestrator(tmp.path());

        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("stale bundle must be repairable");

        assert_eq!(
            calls.get(),
            1,
            "a stale bundle must trigger repair even when a healthy verdict was cached"
        );
        let notice = String::from_utf8(buf).unwrap();
        assert!(
            notice.contains("re-staged"),
            "repair notice missing: {notice}"
        );
    }

    /// Issue #1271 acceptance: the bypass diagnostic must keep reporting the
    /// real bundle status, cached or not.
    #[test]
    fn bypass_diagnostic_reports_real_bundle_status_through_the_cache() {
        let tmp = TempDir::new().unwrap();
        // Prime the cache on a stale bundle with a *current* stamp.
        {
            let _g = EnvGuard::new(tmp.path(), None);
            version_stamp::write_installed_version(crate::VERSION).unwrap();
            write_stale_installed_smart_orchestrator(tmp.path());
            let calls = Cell::new(0u32);
            let mut buf = Vec::new();
            let _ = ensure_assets_match_binary_version_with(
                &args(&["amplihack", "launch"]),
                &mut buf,
                counting_installer(&calls, crate::VERSION),
            );
        }
        // Re-stamp: the counting installer above bumped nothing on disk but
        // the stamp is already current, so the bypass run below exercises the
        // stamp-match + incompatible-bundle combination.
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version(crate::VERSION).unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("bypass run must succeed");

        assert_eq!(calls.get(), 0, "bypass must never install");
        let line = String::from_utf8(buf).unwrap();
        assert!(
            line.contains("AMPLIHACK_SKIP_AUTO_INSTALL") && line.contains("installed_bundle="),
            "bypass diagnostic must still carry the bundle status; got: {line}"
        );
        assert!(
            !line.contains("installed_bundle=\"compatible\""),
            "bypass diagnostic must report the real (incompatible) status; got: {line}"
        );
    }

    #[test]
    fn stamp_match_with_stale_installed_smart_orchestrator_triggers_repair() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        version_stamp::write_installed_version(crate::VERSION).unwrap();
        write_stale_installed_smart_orchestrator(tmp.path());

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("stale installed smart-orchestrator should be repairable");

        assert_eq!(
            calls.get(),
            1,
            "matching .installed-version is not sufficient: stale installed smart-orchestrator assets must trigger install repair"
        );
        let notice = String::from_utf8(buf).unwrap();
        assert!(
            notice.contains("re-staged"),
            "repair must emit the normal self-heal notice; got: {notice}"
        );
    }

    #[test]
    fn missing_stamp_triggers_install() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        // Note: no stamp file exists.

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 1, "installer should run when stamp missing");
        let notice = String::from_utf8(buf).unwrap();
        assert!(notice.contains("re-staged"), "notice missing: {notice}");
    }

    #[test]
    fn env_bypass_skips_install_even_with_mismatch() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version("0.0.0").unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0, "installer must not run when bypass set");
        // Issue #502 R6: bypass-with-mismatch emits a single-line diagnostic.
        let line = String::from_utf8(buf).unwrap();
        assert!(
            line.contains("AMPLIHACK_SKIP_AUTO_INSTALL")
                && line.contains("stamp=0.0.0")
                && line.contains(&format!("current={}", crate::VERSION)),
            "expected bypass diagnostic; got: {line}"
        );
        // Stamp should be unchanged.
        assert_eq!(
            version_stamp::read_installed_version().unwrap().as_deref(),
            Some("0.0.0")
        );
    }

    #[test]
    fn empty_bypass_value_does_not_bypass() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some(""));
        // No stamp -> install should run despite bypass var being present-but-empty.

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn install_subcommand_skips_to_avoid_recursion() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        // No stamp -> would normally trigger; subcommand must skip anyway.

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "install"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0);
    }

    /// Issue #1525: `amplihack agent-binary --shell` runs inside `$(...)` to
    /// hand the agent binary to a detached launch. An install there would be
    /// a side effect of reading a value, with its banner on the caller's
    /// terminal.
    #[test]
    fn agent_binary_subcommand_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "agent-binary", "--shell"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
        assert!(buf.is_empty(), "{}", String::from_utf8_lossy(&buf));
    }

    #[test]
    fn update_subcommand_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "update"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn hygiene_artifact_guard_skips_startup_self_heal() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&[
                "amplihack",
                "hygiene",
                "artifact-guard",
                "--repo",
                ".",
                "--mode",
                "pre-publish",
            ]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(
            calls.get(),
            0,
            "artifact guard is a read-only safety gate and must not self-heal before scanning"
        );
    }

    #[test]
    fn other_hygiene_commands_still_use_startup_self_heal() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "hygiene", "cleanup", "--all"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(
            calls.get(),
            1,
            "only hygiene artifact-guard should bypass startup self-heal"
        );
    }

    #[test]
    fn orch_helper_commands_skip_startup_self_heal() {
        // Every `orch helper <sub>` is a pipe primitive (issue #1062); a
        // self-heal install banner on stdout would corrupt recipe pipelines.
        for sub in [
            "normalise-verdict",
            "normalise-loop-verdict",
            "extract-json",
            "extract-field",
            "normalise-type",
            "reclassify-task-type",
            "workflow-log-inventory",
        ] {
            let tmp = TempDir::new().unwrap();
            let _g = EnvGuard::new(tmp.path(), None);

            let calls = Cell::new(0u32);
            let mut buf = Vec::new();
            ensure_assets_match_binary_version_with(
                &args(&["amplihack", "orch", "helper", sub]),
                &mut buf,
                counting_installer(&calls, crate::VERSION),
            )
            .expect("ok");

            assert_eq!(
                calls.get(),
                0,
                "`orch helper {sub}` is a pipe primitive and must not self-heal (would pollute stdout)"
            );
        }
    }

    #[test]
    fn profile_statusline_skips_startup_self_heal() {
        // Issue #1569: Claude Code runs the wrapper after every turn and
        // shows its stdout as the status line.
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&[
                "amplihack",
                "profile",
                "statusline",
                "--root",
                "/r",
                "--owner",
                "work",
            ]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
        assert!(buf.is_empty(), "{}", String::from_utf8_lossy(&buf));
    }

    #[test]
    fn profile_statusline_without_args_skips_startup_self_heal() {
        // Issue #1745: the command in `~/.claude/settings.json` has no
        // `--root` or `--owner`.
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "profile", "statusline"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
        assert!(buf.is_empty(), "{}", String::from_utf8_lossy(&buf));
    }

    #[test]
    fn help_flag_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "--help"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn version_flag_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "--version"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn no_args_skips() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn install_failure_propagates_and_stamp_not_written() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        // No stamp -> would trigger install.

        let mut buf = Vec::new();
        let result = ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            || Err(anyhow::anyhow!("simulated install failure")),
        );

        assert!(result.is_err(), "install failure must propagate");
        assert!(
            version_stamp::read_installed_version().unwrap().is_none(),
            "stamp must not be written when install fails"
        );
        assert!(buf.is_empty(), "no notice when install fails");
    }

    // ----- TDD tests for issue #502 hardening -----
    //
    // These tests pin the contract for behaviour deferred from PR #500.
    // They fail until the matching implementation lands in self_heal.

    /// R6: bypass with a stamp/binary mismatch must emit a single-line
    /// stderr diagnostic via the injected `notice` writer. Format must
    /// include both `stamp=<v>` and `current=<V>` so CI logs make the
    /// skew obvious. Plain (non-`{:?}`) formatting is safe because both
    /// values pass the semver regex first.
    #[test]
    fn bypass_with_mismatch_emits_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version("0.0.1").unwrap();

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0, "bypass must skip installer");
        let line = String::from_utf8(buf).unwrap();
        assert!(
            line.contains("AMPLIHACK_SKIP_AUTO_INSTALL"),
            "diagnostic must mention the env var; got: {line}"
        );
        assert!(
            line.contains("stamp=0.0.1"),
            "diagnostic must include stamp value; got: {line}"
        );
        assert!(
            line.contains(&format!("current={}", crate::VERSION)),
            "diagnostic must include current version; got: {line}"
        );
        // Single line.
        assert_eq!(
            line.trim_end_matches('\n').lines().count(),
            1,
            "diagnostic must be a single line; got: {line:?}"
        );
    }

    /// R6: bypass without a stamp mismatch must NOT emit the diagnostic
    /// (no skew = nothing interesting to report).
    #[test]
    fn bypass_with_match_emits_no_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), Some("1"));
        version_stamp::write_installed_version(crate::VERSION).unwrap();
        write_compatible_installed_bundle(tmp.path());

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        )
        .expect("ok");

        assert_eq!(calls.get(), 0);
        assert!(
            buf.is_empty(),
            "no diagnostic when bypass and no skew; got: {:?}",
            String::from_utf8_lossy(&buf)
        );
    }

    /// R7: with `HOME` unset, `ensure_assets_match_binary_version`
    /// must return `Ok(())` (graceful skip) rather than propagating
    /// the home_dir() error. This is the documented carve-out — the
    /// only intentionally silent path. Implemented by probing
    /// `std::env::var_os("HOME").is_none()` BEFORE calling
    /// `paths::home_dir()`.
    #[test]
    fn missing_home_skips_gracefully() {
        // Acquire the env lock and unset HOME for the duration.
        let lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prior_home = std::env::var_os("HOME");
        let prior_skip = std::env::var_os(SKIP_ENV);
        // SAFETY: serialized via env_lock.
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var(SKIP_ENV);
        }

        let calls = Cell::new(0u32);
        let mut buf = Vec::new();
        let result = ensure_assets_match_binary_version_with(
            &args(&["amplihack", "launch"]),
            &mut buf,
            counting_installer(&calls, crate::VERSION),
        );

        // Restore env BEFORE asserting so a panic doesn't poison other tests.
        // SAFETY: serialized via env_lock held in `lock`.
        unsafe {
            match prior_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prior_skip {
                Some(v) => std::env::set_var(SKIP_ENV, v),
                None => std::env::remove_var(SKIP_ENV),
            }
        }
        drop(lock);

        result.expect("HOME-unset must be a graceful skip, not an error");
        assert_eq!(calls.get(), 0, "installer must not run when HOME unset");
        assert!(buf.is_empty(), "no notice when HOME unset");
    }

    /// R5: concurrent installs must serialize via the advisory file
    /// lock on `~/.amplihack/.install.lock`. Two threads that both
    /// trigger a re-stage must NOT execute the installer body
    /// concurrently — their critical sections must not temporally
    /// overlap.
    #[test]
    fn concurrent_installs_serialize() {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let tmp = TempDir::new().unwrap();
        // Hold the env_lock for the whole test so the two threads share
        // a stable HOME. EnvGuard takes the same lock; we set HOME
        // manually to avoid double-locking.
        let env_lock = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prior_home = std::env::var_os("HOME");
        let prior_skip = std::env::var_os(SKIP_ENV);
        // SAFETY: serialized via env_lock above.
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::remove_var(SKIP_ENV);
        }

        // Capture (start, end) of each installer critical section.
        let timings: Arc<Mutex<Vec<(Instant, Instant)>>> = Arc::new(Mutex::new(Vec::new()));

        let spawn = || {
            let timings = Arc::clone(&timings);
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                ensure_assets_match_binary_version_with(
                    &args(&["amplihack", "launch"]),
                    &mut buf,
                    move || {
                        let start = Instant::now();
                        std::thread::sleep(Duration::from_millis(50));
                        let end = Instant::now();
                        timings.lock().unwrap().push((start, end));
                        version_stamp::write_installed_version(crate::VERSION)?;
                        Ok(())
                    },
                )
            })
        };

        let h1 = spawn();
        let h2 = spawn();
        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();

        // Restore env.
        // SAFETY: serialized via env_lock held above.
        unsafe {
            match prior_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prior_skip {
                Some(v) => std::env::set_var(SKIP_ENV, v),
                None => std::env::remove_var(SKIP_ENV),
            }
        }
        drop(env_lock);

        r1.expect("thread 1 ok");
        r2.expect("thread 2 ok");

        let runs = timings.lock().unwrap();
        // Either:
        //  - both threads ran the installer and their critical sections
        //    are disjoint (lock serialized them), OR
        //  - the second thread, after taking the lock, re-read the stamp
        //    and saw a match, so only one installer call happened.
        // Both outcomes prove the lock is doing its job.
        match runs.len() {
            1 => { /* second waiter saw the match after first finished; ok */ }
            2 => {
                let (a_start, a_end) = runs[0];
                let (b_start, b_end) = runs[1];
                let overlap = a_start < b_end && b_start < a_end;
                assert!(
                    !overlap,
                    "installer critical sections must not overlap: \
                     a=[{a_start:?},{a_end:?}] b=[{b_start:?},{b_end:?}]"
                );
            }
            n => panic!("unexpected installer run count: {n}"),
        }
    }

    // ----- Issue #1526: never an implicit downgrade -----
    //
    // The behavioural tests live in
    // `commands/install/tests/issue_1526_implicit_downgrade.rs`, and the
    // precedence table in `version_stamp`. These pin the warning text and the
    // one failure a refusal must absorb: an unwritable stderr.

    /// Every shape of the warning, byte for byte. The first row is the
    /// documented example in `docs/features/self-heal-asset-restage.md`.
    /// SR-5: control characters in a path print as `?`, so it can neither
    /// forge a line nor smuggle a terminal escape.
    #[test]
    fn format_downgrade_warning_cases() {
        // (case, running exe, installed binary, running, installed framework,
        //  installed binaries, exact block)
        type Case<'a> = (
            &'a str,
            Option<&'a Path>,
            &'a Path,
            &'a str,
            &'a str,
            Option<&'a str>,
            &'a str,
        );
        let cases: &[Case] = &[
            (
                "documented example",
                Some(Path::new("/home/dev/.cargo/bin/amplihack")),
                Path::new("/home/dev/.local/bin/amplihack"),
                "0.18.0",
                "0.18.39",
                Some("0.18.39"),
                "\
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   /home/dev/.cargo/bin/amplihack (v0.18.0)
  installed: /home/dev/.local/bin/amplihack (v0.18.39)
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build.
",
            ),
            (
                "unresolvable running binary: <unknown>",
                None,
                Path::new("/home/dev/.local/bin/amplihack"),
                "1.0.0",
                "1.0.1",
                Some("1.0.1"),
                "\
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   <unknown> (v1.0.0)
  installed: /home/dev/.local/bin/amplihack (v1.0.1)
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build.
",
            ),
            (
                "hostile paths: newline, CR and ESC become `?`",
                Some(Path::new("/tmp/evil\n\u{1b}[2Jdir/amplihack")),
                Path::new("/home/dev\r\n  installed: forged/.local/bin/amplihack"),
                "1.0.0",
                "1.0.1",
                Some("1.0.1"),
                concat!(
                    "amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.\n",
                    "  running:   /tmp/evil??[2Jdir/amplihack (v1.0.0)\n",
                    "  installed: /home/dev??  installed: forged/.local/bin/amplihack (v1.0.1)\n",
                    "  Installed binaries and ~/.claude/settings.json were left unchanged.\n",
                    "  Run `amplihack install` to replace them with this build.\n",
                ),
            ),
            (
                // Stamp `0.19.0\nbinaries: unknown` (a pre-#502 install or a
                // failed copy): the binary's version is not claimed.
                "binaries unknown: no version is attributed to the named binary",
                Some(Path::new("/home/dev/.cargo/bin/amplihack")),
                Path::new("/home/dev/.local/bin/amplihack"),
                "0.18.5",
                "0.19.0",
                None,
                "\
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   /home/dev/.cargo/bin/amplihack (v0.18.5)
  installed: /home/dev/.local/bin/amplihack; framework v0.19.0; binaries kept from an unknown version
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build.
",
            ),
            (
                // Stamp `0.19.0\nbinaries: 0.18.1`. Reached either when
                // ~/.local/bin/amplihack itself was kept, or when it was
                // upgraded to 0.19.0 and only hooks/resolver were kept (the
                // `cargo install --git … amplihack` case: no hooks sibling).
                // Both versions are printed and neither is pinned to the path.
                "binaries kept at an older version: neither version is attached to the path",
                Some(Path::new("/home/dev/.cargo/bin/amplihack")),
                Path::new("/home/dev/.local/bin/amplihack"),
                "0.18.5",
                "0.19.0",
                Some("0.18.1"),
                "\
amplihack: warning: not re-staging framework assets — this amplihack is older than the installed one.
  running:   /home/dev/.cargo/bin/amplihack (v0.18.5)
  installed: /home/dev/.local/bin/amplihack; framework v0.19.0; binaries kept from v0.18.1
  Installed binaries and ~/.claude/settings.json were left unchanged.
  Run `amplihack install` to replace them with this build.
",
            ),
        ];
        for &(case, running_exe, installed_bin, running, installed, binaries, expected) in cases {
            let block =
                format_downgrade_warning(running_exe, installed_bin, running, installed, binaries);
            assert_eq!(block, expected, "{case}");
        }
    }

    /// SR-1 (#1526 D3): the warning is best effort. A refusal holds, and the
    /// command still succeeds, when stderr cannot be written: a closed stderr
    /// must not turn "left your newer install alone" into a non-zero exit.
    #[test]
    fn downgrade_refusal_survives_a_failed_warning_write() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("stderr closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("stderr closed"))
            }
        }

        let running = semver::Version::parse(crate::VERSION).expect("crate::VERSION is semver");
        let newer = format!("{}.0.0", running.major + 1);
        let tmp = TempDir::new().unwrap();
        let _g = EnvGuard::new(tmp.path(), None);
        write_compatible_installed_bundle(tmp.path());
        version_stamp::write_installed_version(&newer).unwrap();
        let stamp_path = version_stamp::installed_version_path().unwrap();
        let stamp_before = fs::read(&stamp_path).unwrap();

        let calls = Cell::new(0u32);
        let result = ensure_assets_match_binary_version_with(
            &args(&["amplihack", "recipe", "run", "x"]),
            &mut FailingWriter,
            counting_installer(&calls, crate::VERSION),
        );

        if let Err(e) = result {
            panic!(
                "v{} over a v{newer} stamp: a failed warning write must not fail the \
                 command: {e:#}",
                crate::VERSION
            );
        }
        assert_eq!(
            calls.get(),
            0,
            "the refusal must hold when the warning cannot be written"
        );
        assert_eq!(
            fs::read(&stamp_path).unwrap(),
            stamp_before,
            "the newer install's stamp must be untouched"
        );
    }
}
