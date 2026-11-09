use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Single global lock for all environment-mutating tests.
///
/// Both HOME and CWD mutations must serialize through one lock to prevent
/// races. Tests that need both HOME and CWD should acquire `env_lock()` once.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// Alias for `env_lock()` — all env locks use the same underlying mutex.
pub(crate) fn home_env_lock() -> &'static Mutex<()> {
    env_lock()
}

/// Alias for `env_lock()` — all env locks use the same underlying mutex.
pub(crate) fn cwd_env_lock() -> &'static Mutex<()> {
    env_lock()
}

/// One lock for tests that start a token refresh or read the refresh count
/// (issue #2036). The count (`usage_direct::refreshes_in_progress`) covers the
/// whole process, and the rotator's stop waits for it to reach zero, so a test
/// that checks it must not run beside another test's refresh. Poisoning is
/// ignored: a failed test must not fail the others.
pub(crate) fn refresh_count_lock() -> std::sync::MutexGuard<'static, ()> {
    static REFRESH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    REFRESH_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[path = "../tests/support/scratch_base.rs"]
#[allow(dead_code)]
mod scratch_base;

/// A temporary directory that no framework lookup can see past: no
/// `amplifier-bundle/`, `.claude/` or `src/amplihack/.claude/` above it, and
/// not under the real `~/.claude` or `~/.amplihack`. Panics when no candidate
/// base qualifies. The rule, and the lookups that need it, are in
/// `tests/support/scratch_base.rs`, shared with `amplihack-hooks`,
/// `amplihack-types` and `cli_integration`.
pub(crate) fn tempdir_outside_framework_roots() -> tempfile::TempDir {
    tempfile::tempdir_in(scratch_base::scratch_base()).unwrap()
}

#[cfg(test)]
mod scratch_base_tests {
    use super::scratch_base::{FRAMEWORK_MARKERS, scratch_base_rejection};
    use std::fs;

    #[test]
    fn a_base_below_any_framework_marker_is_rejected_by_name() {
        for marker in FRAMEWORK_MARKERS {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir_all(root.path().join(marker)).unwrap();
            let base = root.path().join("runtime/tmp");
            fs::create_dir_all(&base).unwrap();
            let reason = scratch_base_rejection(&base, None)
                .unwrap_or_else(|| panic!("{marker} above the base was not rejected"));
            assert!(
                reason.contains(&root.path().join(marker).display().to_string()),
                "the rejection must name the offending ancestor: {reason}"
            );
        }
    }

    #[test]
    fn a_base_under_the_real_home_dirs_is_rejected_without_any_marker() {
        let home = tempfile::tempdir().unwrap();
        for dir in [".claude", ".amplihack"] {
            let base = home.path().join(dir).join("work");
            fs::create_dir_all(&base).unwrap();
            let reason = scratch_base_rejection(&base, Some(home.path()));
            assert!(
                reason.as_deref().is_some_and(|r| r.contains(dir)),
                "{}: {reason:?}",
                base.display()
            );
        }
    }

    #[test]
    fn a_missing_base_is_rejected_and_a_clean_one_accepted() {
        let root = super::tempdir_outside_framework_roots();
        assert_eq!(scratch_base_rejection(root.path(), None), None);
        assert!(scratch_base_rejection(&root.path().join("absent"), None).is_some());
    }
}

/// RAII guard for process environment variables mutated by tests.
///
/// Tests must acquire `env_lock()` before constructing this guard.
pub(crate) struct EnvGuard {
    previous: Vec<(OsString, Option<OsString>)>,
}

impl EnvGuard {
    /// Capture variables before a fixture's manual mutations. The shared
    /// environment lock must remain held until this guard drops.
    pub(crate) fn capture(keys: &[&str]) -> Self {
        Self {
            previous: keys
                .iter()
                .map(|key| (OsString::from(key), std::env::var_os(key)))
                .collect(),
        }
    }

    pub(crate) fn set<const N: usize>(values: [(&str, &str); N]) -> Self {
        let previous = values
            .iter()
            .map(|(name, _)| (OsString::from(name), std::env::var_os(name)))
            .collect();
        for (name, value) in values {
            unsafe { std::env::set_var(name, value) };
        }
        Self { previous }
    }

    /// Remove `names` from the environment until the guard is dropped.
    pub(crate) fn unset<const N: usize>(names: [&str; N]) -> Self {
        let previous = names
            .iter()
            .map(|name| (OsString::from(name), std::env::var_os(name)))
            .collect();
        for name in names {
            unsafe { std::env::remove_var(name) };
        }
        Self { previous }
    }
}

/// The variables that name an outer recipe run (#1984): the shell helper's five
/// run-identity names (amplifier-bundle/tools/git-repo-env.sh) plus the two
/// depth variables. A test started inside a recipe-run session inherits them,
/// and a runner it starts then joins the outer tree or is refused by its depth
/// guard instead of starting a fresh top-level run. Sorted.
pub(crate) const OUTER_RUN_IDENTITY_VARS: [&str; 7] = [
    "AMPLIHACK_MAX_DEPTH",
    "AMPLIHACK_RECIPE_RUN_ID",
    "AMPLIHACK_RUN_DIR",
    "AMPLIHACK_RUN_ID",
    "AMPLIHACK_SESSION_DEPTH",
    "AMPLIHACK_SESSION_TREE_DIR",
    "AMPLIHACK_TREE_ID",
];

/// Remove every [`OUTER_RUN_IDENTITY_VARS`] entry until the guard drops. The
/// caller must hold `env_lock()` (or `home_env_lock()`), as for any `EnvGuard`.
pub(crate) fn clear_outer_run_env() -> EnvGuard {
    EnvGuard::unset(OUTER_RUN_IDENTITY_VARS)
}

/// Runs `f` under the shared env lock with each variable set (or removed when
/// `None`). The previous values are restored on return and on panic.
pub(crate) fn with_env<T>(vars: &[(&str, Option<&OsStr>)], f: impl FnOnce() -> T) -> T {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let keys: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
    let _restore = EnvGuard::capture(&keys);
    for (key, value) in vars {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    f()
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..) {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// RAII guard giving a test a known agent-binary environment: every
/// `agent_binary::SESSION_MARKERS` variable removed, and `AMPLIHACK_AGENT_BINARY`
/// / `AMPLIHACK_AGENT_BINARY_SOURCE` set to `binary` / `tag` (or removed when
/// `None`). Everything is restored on drop.
///
/// Tests must acquire `env_lock()` before constructing this guard.
pub(crate) struct AgentBinaryEnv {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl AgentBinaryEnv {
    pub(crate) fn set(binary: Option<&str>, tag: Option<&str>) -> Self {
        use amplihack_utils::agent_binary::{BINARY_ENV, SESSION_MARKERS, SOURCE_ENV};
        let keys = SESSION_MARKERS
            .iter()
            .map(|(key, _)| *key)
            .chain([BINARY_ENV, SOURCE_ENV]);
        let previous = keys.map(|key| (key, std::env::var_os(key))).collect();
        // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
        unsafe {
            for (key, _) in SESSION_MARKERS {
                std::env::remove_var(key);
            }
            for (key, value) in [(BINARY_ENV, binary), (SOURCE_ENV, tag)] {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { previous }
    }
}

impl Drop for AgentBinaryEnv {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            // SAFETY: as in `set`.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

pub(crate) fn set_home(path: &Path) -> Option<std::ffi::OsString> {
    let previous = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", path);
    }
    previous
}

pub(crate) fn restore_home(previous: Option<std::ffi::OsString>) {
    if let Some(value) = previous {
        unsafe { std::env::set_var("HOME", value) };
    } else {
        unsafe { std::env::remove_var("HOME") };
    }
}

pub(crate) fn set_cwd(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let previous = std::env::current_dir()?;
    std::env::set_current_dir(path)?;
    Ok(previous)
}

pub(crate) fn restore_cwd(previous: &Path) -> std::io::Result<()> {
    std::env::set_current_dir(previous)
}

/// RAII guard that sets the current working directory and restores the
/// previous value on drop, even when the test panics.
///
/// Tests must acquire `env_lock()` (or an alias like `cwd_env_lock()`) before
/// constructing this guard so concurrent tests don't observe the mutated cwd.
/// Restore-on-drop reports an error if the previous directory is unavailable.
pub(crate) struct CwdGuard {
    previous: PathBuf,
}

impl CwdGuard {
    /// Switch the process cwd to `path`, returning a guard that restores the
    /// prior cwd on drop.
    pub(crate) fn set(path: &Path) -> std::io::Result<Self> {
        let previous = std::env::current_dir()?;
        std::env::set_current_dir(path)?;
        Ok(Self { previous })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Err(error) = std::env::set_current_dir(&self.previous) {
            eprintln!("failed to restore fixture working directory: {error}");
        }
    }
}

/// RAII guard that sets `HOME` and restores the previous value on drop, even
/// when the test panics.
///
/// Tests must acquire `env_lock()` (or `home_env_lock()`) before constructing
/// this guard so concurrent tests don't observe the mutated HOME.
pub(crate) struct HomeGuard {
    previous: Option<std::ffi::OsString>,
}

impl HomeGuard {
    /// Set `HOME` to `path`, returning a guard that restores (or unsets) the
    /// prior value on drop.
    pub(crate) fn set(path: &Path) -> Self {
        let previous = std::env::var_os("HOME");
        // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
        unsafe {
            std::env::set_var("HOME", path);
        }
        Self { previous }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
        unsafe {
            match self.previous.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// RAII guard that clears `AMPLIHACK_GRAPH_DB_PATH` and `AMPLIHACK_KUZU_DB_PATH`
/// for the duration of a test and restores the previous values on drop.
///
/// `EnvBuilder::with_project_graph_db` reads these env vars first and falls
/// back to a project-derived path only when neither is set. Tests that assert
/// against the project-derived path therefore must clear any ambient values
/// the developer's shell or CI runner may have set, otherwise the leaking
/// value wins and the assertion fails. Acquire the `env_lock()` before
/// constructing this guard so concurrent tests don't observe the cleared
/// state.
pub(crate) struct ClearedGraphDbEnv {
    previous_graph: Option<std::ffi::OsString>,
    previous_kuzu: Option<std::ffi::OsString>,
}

impl ClearedGraphDbEnv {
    pub(crate) fn new() -> Self {
        let previous_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        let previous_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH");
            std::env::remove_var("AMPLIHACK_KUZU_DB_PATH");
        }
        Self {
            previous_graph,
            previous_kuzu,
        }
    }
}

impl Drop for ClearedGraphDbEnv {
    fn drop(&mut self) {
        unsafe {
            match self.previous_graph.take() {
                Some(value) => std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value),
                None => std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH"),
            }
            match self.previous_kuzu.take() {
                Some(value) => std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value),
                None => std::env::remove_var("AMPLIHACK_KUZU_DB_PATH"),
            }
        }
    }
}

#[cfg(unix)]
#[path = "../tests/support/exec_stub.rs"]
#[allow(dead_code)]
mod exec_stub;

/// Writes an executable stub at `path` (mode 0755) through a short-lived
/// `sh`, so no fork of this process can inherit a write descriptor to it and
/// make a concurrent exec fail with `ETXTBSY` (issue #1973).
#[cfg(unix)]
pub(crate) fn write_executable(path: &Path, body: &str) {
    exec_stub::write_executable(path, body, 0o755);
}
