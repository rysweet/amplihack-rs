use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Single global lock for all environment-mutating tests.
///
/// Both HOME and CWD mutations must serialize through one lock to prevent
/// races. Tests that need both HOME and CWD should acquire `env_lock()` once.
///
/// Creating the lock also fires [`claude_profile_isolation::purge_inherited`]
/// (issue #2922), so a `CLAUDE_CONFIG_DIR` or login profile inherited from the
/// session that started `cargo test` is gone before the first test that holds
/// this lock mutates anything. The purge sits in the initialiser rather than
/// in a separate step each test must remember, because a step each test must
/// remember is the shape that let a test write a fixture's settings into a
/// developer's live profile in the first place. It runs exactly once per
/// process, at the first call, and never again.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| {
        claude_profile_isolation::purge_inherited();
        Mutex::new(())
    })
}

/// Alias for `env_lock()` — all env locks use the same underlying mutex.
pub(crate) fn home_env_lock() -> &'static Mutex<()> {
    env_lock()
}

/// A `PATH` that still holds this host's real directories, for a test that
/// starts a program resolved through it.
///
/// Tests in this binary set the process-global `PATH` to a single private
/// directory while they run (the install and update suites do, to test
/// shadowing and absence). A sibling test that spawns `sh`, `bash` or `git`
/// in that window finds none of them, and fails with `NotFound` or exit 127
/// for a reason that has nothing to do with what it was testing.
///
/// The value is read once, under the same lock those tests hold, so it is
/// never a narrowed one. Spawns then use it with no lock, and stay parallel:
/// `Command::env("PATH", ..)` sets the search path for the program itself as
/// well as the child's environment.
///
/// **Must not be called while holding [`env_lock`].** The initializer takes
/// that lock, which is a plain non-reentrant `Mutex`, so a caller that
/// already holds it deadlocks — and only sometimes, since a sibling test that
/// initialized the `OnceLock` first makes the initializer never run. A test
/// that holds the lock across its spawns wants a `PATH` constant of its own
/// instead (`run_guidance::tests::CHILD_PATH`), or should call this before
/// taking the lock.
pub(crate) fn host_path() -> OsString {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin"))
    })
    .clone()
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

/// Drops `held`, an `flock`ed handle on `path`, and waits until the lock is
/// really free.
///
/// An `flock` belongs to the open file description, not the fd. When another
/// test in this process spawns a child while `held` is open, the forked child
/// shares that description until it execs, so the lock can outlive
/// `drop(held)` by a moment. A test that expects success right after the drop
/// fails then with "in use". This waits, with a fresh descriptor, until the
/// lock can be taken, and releases it again. Panics after 30 seconds.
#[cfg(unix)]
pub(crate) fn release_flock(held: std::fs::File, path: &Path) {
    use std::os::fd::AsRawFd;
    drop(held);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let probe = std::fs::File::open(path).unwrap();
        // SAFETY: the fd is valid for the life of `probe`; closing it unlocks.
        if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return;
        }
        let error = std::io::Error::last_os_error();
        assert!(
            std::time::Instant::now() < deadline,
            "{} still locked 30 s after release: {error}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A synthetic process table per test (issue #2196).
#[cfg(unix)]
#[path = "test_support_proc.rs"]
pub(crate) mod proc;

/// The six profile-redirecting variables and the guards that keep a test's
/// settings writes inside its own temporary home (issue #2922). Shared with
/// the other crates' `test_support`, with `cli_integration` and with the
/// golden-hook harness.
#[path = "../../../tests/support/claude_profile_isolation.rs"]
pub(crate) mod claude_profile_isolation;

#[path = "../tests/support/scratch_base.rs"]
#[allow(dead_code)]
mod scratch_base;

/// Issue #2931: the end of file on a pipe that a process group holds, as the
/// event "the group is gone".
#[cfg(unix)]
#[path = "../tests/support/exit_witness.rs"]
#[allow(dead_code)]
pub(crate) mod exit_witness;

/// Issue #2931: `ls -ldn` parsing for stubs that inspect a file the test
/// removes afterwards; one copy, shared with `cli_integration`.
#[cfg(unix)]
#[path = "../tests/support/ls_mode.rs"]
#[allow(dead_code)]
pub(crate) mod ls_mode;

/// Issue #2931: the one place that builds a non-UTF-8 file name and records
/// why an Apple target ignores the tests that need one.
#[cfg(unix)]
#[path = "../tests/support/non_utf8_name.rs"]
#[allow(dead_code)]
pub(crate) mod non_utf8_name;

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

    /// Issue #2931 (ENOTDIR): `/var/folders/..` is a link to `/private/var/..`
    /// on macOS, and the run directory opens every component without following
    /// links. The base must therefore come back in canonical form.
    #[test]
    fn the_scratch_base_is_returned_in_canonical_form() {
        let base = super::scratch_base::scratch_base();
        assert_eq!(
            base,
            base.canonicalize().expect("the base exists"),
            "a base that is reached through a link is refused by the run directory"
        );
    }

    /// A link whose target lies under the real `~/.claude` must not pass for a
    /// clean base because its own text does not start with the home path.
    #[cfg(unix)]
    #[test]
    fn a_base_reached_through_a_link_into_the_real_home_dirs_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let real = home_path.join(".claude").join("work");
        fs::create_dir_all(&real).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = outside.path().join("innocent");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let reason = scratch_base_rejection(&link, Some(&home_path));
        assert!(
            reason.as_deref().is_some_and(|r| r.contains(".claude")),
            "{}: {reason:?}",
            link.display()
        );
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
///
/// A guard that moves `HOME` also clears the six
/// [`claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS`] and restores them
/// with everything else it touched, so pointing `HOME` at a temporary folder
/// through this type cannot leave an inherited `CLAUDE_CONFIG_DIR` pointing at
/// a developer's live profile (issue #2922). Each variable is saved once, the
/// first time the guard touches it, so the restore does not depend on the
/// order in which the variables were touched.
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
        let mut guard = Self::capture(&[]);
        guard.clear_profile_if_home(values.iter().map(|(name, _)| *name));
        for (name, value) in values {
            guard.put(name, value);
        }
        guard
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

    /// Set `key` to `value` until the guard is dropped. The value `key` had
    /// before the guard first touched it is restored on drop, panics included.
    ///
    /// Assigning `HOME` also clears the six profile variables (issue #2922),
    /// so a guard built with [`EnvGuard::capture`] and then pointed at a
    /// temporary home is isolated in the same way as one built with
    /// [`EnvGuard::set`]. A profile variable must therefore be assigned after
    /// `HOME`, not before.
    pub(crate) fn assign(&mut self, key: &str, value: impl AsRef<OsStr>) {
        self.clear_profile_if_home([key]);
        self.put(key, value);
    }

    fn put(&mut self, key: &str, value: impl AsRef<OsStr>) {
        self.remember(key);
        unsafe { std::env::set_var(key, value) };
    }

    /// Remove `key` until the guard is dropped, restoring it on drop.
    pub(crate) fn clear(&mut self, key: &str) {
        self.remember(key);
        unsafe { std::env::remove_var(key) };
    }

    fn remember(&mut self, key: &str) {
        if !self.previous.iter().any(|(name, _)| name == key) {
            self.previous
                .push((OsString::from(key), std::env::var_os(key)));
        }
    }

    /// Clear the six [`claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS`]
    /// until the guard drops, if `keys` contains `HOME` (issue #2922).
    ///
    /// A guard that moves `HOME` may make installer code write a settings
    /// file, and these are the variables that would redirect that write out of
    /// the temporary home. A guard that leaves `HOME` alone leaves them alone
    /// too, so a test that deliberately exercises them still observes them.
    fn clear_profile_if_home<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) {
        if keys.into_iter().any(|key| key == "HOME") {
            for name in claude_profile_isolation::CLAUDE_PROFILE_ENV_VARS {
                self.clear(name);
            }
        }
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

/// Set or clear several path-valued variables until the guard drops: `Some`
/// sets, `None` removes. The caller must hold `env_lock()`, as for any
/// [`EnvGuard`].
///
/// This is the shape a fixture wants when it points `HOME`, `CODEX_HOME` or a
/// profile root at a temp directory, which [`EnvGuard::set`] cannot take
/// because its values are `&str`.
pub(crate) fn env_paths(vars: &[(&str, Option<&Path>)]) -> EnvGuard {
    let mut guard = EnvGuard::capture(&[]);
    guard.clear_profile_if_home(vars.iter().map(|(key, _)| *key));
    for (key, value) in vars {
        match value {
            Some(path) => guard.put(key, path),
            None => guard.clear(key),
        }
    }
    guard
}

/// Runs `f` under the shared env lock with each variable set (or removed when
/// `None`). The previous values are restored on return and on panic.
pub(crate) fn with_env<T>(vars: &[(&str, Option<&OsStr>)], f: impl FnOnce() -> T) -> T {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let keys: Vec<&str> = vars.iter().map(|(k, _)| *k).collect();
    let mut restore = EnvGuard::capture(&keys);
    restore.clear_profile_if_home(keys.iter().copied());
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

/// A private work root for tests that run a recipe (issue #2067).
///
/// `amplihack recipe run` creates its run folder under the work root, which
/// it takes from `AMPLIHACK_WORK_ROOT` (`cleanup::work_root::resolve_from_env`;
/// nothing reads an inherited `AMPLIHACK_RUN_ID` or `AMPLIHACK_RUN_DIR`). A
/// test run under a recipe inherits the caller's root, normally the real
/// `~/.amplihack/work`, so without this guard each test leaves a run folder
/// there. This guard points `AMPLIHACK_WORK_ROOT` at `<temp>/work`, removes
/// `AMPLIHACK_WORK_ROOT_EXPLICIT`, and turns the end-of-run sweep off: the
/// sweep also lists hook build folders under `$XDG_CACHE_HOME/amplihack` or
/// `~/.cache/amplihack`, which are outside the work root and are not
/// isolated here. On drop the variables are restored and then the folder is
/// removed.
///
/// This guard is the only owner of these three variables in any fixture that
/// uses it. Do not also capture or set them in another `EnvGuard` in the same
/// fixture: two guards saving one variable restore it in the wrong order.
/// For example, in a `(EnvGuard, IsolatedWorkRoot)` tuple the first element
/// drops first, so this guard would then write back the value the other guard
/// had set, and that value would leak into later tests.
///
/// Tests must hold `env_lock()` until this guard drops.
pub(crate) struct IsolatedWorkRoot {
    // Field order is drop order: restore the variables before removing the
    // folder they point at.
    _env: EnvGuard,
    dir: tempfile::TempDir,
}

impl IsolatedWorkRoot {
    pub(crate) fn new() -> Self {
        let dir = tempdir_outside_framework_roots();
        let env = EnvGuard::capture(&[
            "AMPLIHACK_WORK_ROOT",
            "AMPLIHACK_WORK_ROOT_EXPLICIT",
            "AMPLIHACK_CLEANUP_ON_EXIT",
            crate::run_guidance::GUIDANCE_DIRS_ENV,
        ]);
        unsafe {
            // Issue #2535: an outer run's guidance never reaches a test run.
            std::env::remove_var(crate::run_guidance::GUIDANCE_DIRS_ENV);
            std::env::set_var("AMPLIHACK_WORK_ROOT", dir.path().join("work"));
            std::env::remove_var("AMPLIHACK_WORK_ROOT_EXPLICIT");
            std::env::set_var("AMPLIHACK_CLEANUP_ON_EXIT", "0");
        }
        Self { _env: env, dir }
    }

    /// The work root this guard set, `<temp>/work`. It is created by the
    /// first run, not by the guard.
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.path().join("work")
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

/// Point `HOME` at `path`, returning the prior value for [`restore_home`].
///
/// This also fires [`claude_profile_isolation::purge_inherited`] (issue
/// #2922), so an inherited `CLAUDE_CONFIG_DIR` cannot send a settings write
/// out of `path`. The purge has no restore, which is why this function cannot
/// offer one: its `Option<OsString>` return is unwound by `restore_home` at
/// call sites that have no guard to hang a restore on. A fixture that needs
/// the six profile variables put back afterwards takes a [`HomeGuard`]
/// instead, or a [`claude_profile_isolation::ClaudeProfileIsolation`] of its
/// own.
///
/// The caller must hold `env_lock()`.
pub(crate) fn set_home(path: &Path) -> Option<std::ffi::OsString> {
    claude_profile_isolation::purge_inherited();
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
    // Issue #2922. Field order is drop order, and `Drop::drop` below runs
    // before either field, so `HOME` is restored first and the six profile
    // variables after it.
    _profile: claude_profile_isolation::ClaudeProfileIsolation,
}

impl HomeGuard {
    /// Set `HOME` to `path`, returning a guard that restores (or unsets) the
    /// prior value on drop.
    ///
    /// The guard also clears the six variables that would redirect a settings
    /// write out of `path` ([`claude_profile_isolation`], issue #2922) and
    /// restores them with `HOME`. A test that must keep `CLAUDE_CONFIG_DIR`
    /// set takes `claude_profile_isolation::with_config_dir` afterwards, which
    /// accepts a directory inside `path` only.
    pub(crate) fn set(path: &Path) -> Self {
        let profile = claude_profile_isolation::ClaudeProfileIsolation::new();
        let previous = std::env::var_os("HOME");
        // SAFETY: edition 2024 requires unsafe; tests serialise via env_lock().
        unsafe {
            std::env::set_var("HOME", path);
        }
        Self {
            previous,
            _profile: profile,
        }
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

/// [`write_executable`] with the permission bits given, for a stub that must
/// stay private to its owner (mode 0700). Setuid, setgid and sticky bits are
/// refused.
#[cfg(unix)]
pub(crate) fn write_executable_with_mode(path: &Path, body: &str, mode: u32) {
    exec_stub::write_executable(path, body, mode);
}

/// One word of POSIX shell text holding `value` byte for byte (issue #2931):
/// every path or name a lib test writes into the text of a stub goes through
/// this. The one definition is `tests/support/exec_stub.rs`.
#[cfg(unix)]
pub(crate) fn sh_quote(value: &str) -> String {
    exec_stub::sh_quote(value)
}

#[path = "test_support_proc_guard.rs"]
mod proc_guard;

/// The one hang guard of `tests/support/wait.rs`, for lib tests (issue #2019).
#[path = "../tests/support/wait.rs"]
pub(crate) mod wait;

/// The fake clocks shared by every test crate (issue #2019).
#[path = "../tests/support/virtual_clock.rs"]
pub(crate) mod virtual_clock;

#[cfg(unix)]
#[path = "../tests/support/unix_process.rs"]
pub(crate) mod unix_process;

/// A bare origin, clones of it, and the three states a folder's work tree can
/// be in: clean and pushed, uncommitted, or committed and not pushed (issue
/// #2785).
#[path = "test_support/origin_clones.rs"]
pub(crate) mod origin_clones;

/// A gate that holds one side of a test at a known point (issue #2019).
#[path = "test_support/test_gate.rs"]
pub(crate) mod test_gate;

/// Wait on and kill a process through a pidfd (issue #2019).
#[cfg(target_os = "linux")]
#[path = "test_support/pidfd_wait.rs"]
pub(crate) mod pidfd_wait;

// Issue #2019: contracts of the event-wait helpers (`test_gate`, `wait`,
// `pidfd_wait`) and of the stub writer's mode check.
#[cfg(unix)]
#[path = "test_support/tests_2019_helpers.rs"]
mod tests_2019_helpers;
