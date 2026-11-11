// crates/amplihack-cli/src/commands/recipe/run/tests_runner_timeout.rs
//
// Issue #2022: the recipe runner has no overall time limit unless
// `AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS` sets one.
//
// * AC2: `parse_runner_timeout` is a pure resolver. Unset, empty, `0`,
//   negative, non-numeric and overflowing values all give `None`; a digit
//   string N > 0 (after trimming) gives `Some(N s)`.
// * D1: a non-empty value that is not `0` and does not parse warns once per
//   process. The warning names the variable and never prints its value.
// * `wait_for_recipe_runner(child, None)` has no deadline arm: it returns
//   only when the child exits or a stop signal is pending.
// * AC3: with the variable unset (or `0`, empty, garbage), a runner that
//   blocks until the test releases it completes normally.
// * AC4: with `=1`, a runner that is never released ends with
//   `timed out after 1s`, the existing `indeterminate` classification, and
//   its process torn down.
//
// No stub sleeps. Each stub blocks on a FIFO inside the test's own tempdir
// (`Gate`) and exits only when the test releases it, or is killed by the
// limit under test. No test measures elapsed time (the 9 Nov timing rule).
// A gate that is never released means only the limit can end the wait, so
// the tests check the exact limit value, the message and the classification.
// `ChildGuard` and `Gate`'s drop release and
// reap every stub even when an assertion panics.
//
// Every test that touches the variable holds `home_env_lock()` and restores
// it.
//
// Wired from `recipe/run/mod.rs` with `#[cfg(test)] mod tests_runner_timeout;`.

use super::*;

mod execute {
    pub(super) use super::super::execute::{
        RUNNER_TIMEOUT_WARNING, RunnerWait, parse_runner_timeout, recipe_runner_timeout,
        runner_timeout_warns, wait_for_recipe_runner,
    };
    pub(super) use super::execute_recipe_via_rust_for_test as execute_recipe_via_rust;
}

use std::collections::BTreeMap;
use std::time::Duration;

const RUNNER_TIMEOUT_ENV: &str = "AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS";
const RUNNER_PATH_ENV: &str = "RECIPE_RUNNER_RS_PATH";
const SESSION_TREE_DIR_ENV: &str = "AMPLIHACK_SESSION_TREE_DIR";
const TEARDOWN_GRACE_ENV: &str = "AMPLIHACK_TEARDOWN_GRACE_SECS";

/// Snapshots one variable and restores it on drop. Callers hold
/// `home_env_lock()`.
struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The caller holds the shared environment lock through restoration.
///
/// The run's work root is pointed at `work_root`, a folder inside the test's
/// temp dir. Otherwise the end-of-run cleanup sweeps the host's real work root
/// (`git` and `gh` calls for every run folder there), which can take minutes.
fn fixture_environment(work_root: &std::path::Path) -> crate::test_support::EnvGuard {
    let saved = crate::test_support::EnvGuard::capture(&[
        "AMPLIHACK_AGENT_BINARY",
        "HOME",
        "AMPLIHACK_WORK_ROOT",
        "AMPLIHACK_WORK_ROOT_EXPLICIT",
    ]);
    unsafe {
        std::env::set_var("AMPLIHACK_AGENT_BINARY", "claude");
        std::env::set_var("AMPLIHACK_WORK_ROOT", work_root);
        std::env::remove_var("AMPLIHACK_WORK_ROOT_EXPLICIT");
    }
    saved
}

/// Single-quotes a path for embedding in a stub script; each `'` becomes
/// `'\''`.
#[cfg(unix)]
fn sh_quote(p: &std::path::Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

/// A FIFO in a per-test tempdir that a stub blocks on until the test
/// releases it. Dropping the gate always frees a blocked reader and a blocked
/// release thread: `open(O_RDWR)` never blocks on Linux and counts as both a
/// reader and a writer.
#[cfg(unix)]
struct Gate {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Gate {
    fn new(dir: &std::path::Path, name: &str) -> Self {
        use std::os::unix::ffi::OsStrExt as _;
        let path = dir.join(name);
        let c_path =
            std::ffi::CString::new(path.as_os_str().as_bytes()).expect("fifo path has no NUL");
        // SAFETY: `c_path` is a valid NUL-terminated string that outlives the
        // call; mkfifo only reads it.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Single-quoted for embedding in a stub script.
    fn quoted(&self) -> String {
        sh_quote(&self.path)
    }

    /// Blocks until the stub opens the FIFO, then lets it continue.
    fn release(path: &std::path::Path) {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = f.write_all(b"\n");
        }
    }

    fn release_in_background(&self) -> std::thread::JoinHandle<()> {
        let path = self.path.clone();
        std::thread::spawn(move || Self::release(&path))
    }
}

#[cfg(unix)]
impl Drop for Gate {
    fn drop(&mut self) {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.path)
        {
            let _ = f.write_all(b"\n");
        }
    }
}

/// Kills and reaps a directly spawned stub on drop, panic or not.
#[cfg(unix)]
struct ChildGuard(std::process::Child);

#[cfg(unix)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// -------------------------------------------------------------------------
// AC2: the resolver
// -------------------------------------------------------------------------

#[test]
fn ac2_unset_empty_zero_and_garbage_give_no_limit() {
    for raw in [
        None,
        Some(""),
        Some("   "),
        Some("0"),
        Some(" 0 "),
        Some("000"),
        Some("abc"),
        Some("-5"),
        Some("99999999999999999999"),
        Some("+7"),
        Some("7s"),
        Some("0x10"),
        Some("7.5"),
        Some("\u{0}7"),
    ] {
        assert_eq!(parse(raw), None, "{raw:?} must mean no limit");
    }
}

#[test]
fn ac2_a_positive_whole_number_is_a_limit_in_seconds() {
    assert_eq!(parse(Some("7")), Some(Duration::from_secs(7)));
    assert_eq!(parse(Some(" 7 ")), Some(Duration::from_secs(7)));
    assert_eq!(parse(Some("7\n")), Some(Duration::from_secs(7)));
    assert_eq!(parse(Some("1")), Some(Duration::from_secs(1)));
    assert_eq!(
        parse(Some("18446744073709551615")),
        Some(Duration::from_secs(u64::MAX)),
        "u64::MAX itself parses; only overflow is rejected"
    );
}

fn parse(raw: Option<&str>) -> Option<Duration> {
    execute::parse_runner_timeout(raw)
}

#[test]
fn the_former_six_hour_default_is_not_applied_when_unset() {
    let _guard = lock();
    let _env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);
    assert_eq!(execute::recipe_runner_timeout(), None);
}

#[test]
fn the_variable_is_read_through_the_resolver() {
    let _guard = lock();
    let _env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "42");
    assert_eq!(
        execute::recipe_runner_timeout(),
        Some(Duration::from_secs(42))
    );
    let _env2 = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "0");
    assert_eq!(execute::recipe_runner_timeout(), None, "=0 means none");
}

// -------------------------------------------------------------------------
// D1: the warning
// -------------------------------------------------------------------------

#[test]
fn only_a_non_empty_unparseable_value_warns() {
    for raw in [
        None,
        Some(""),
        Some("  "),
        Some("0"),
        Some("7"),
        Some(" 7 "),
    ] {
        assert!(!execute::runner_timeout_warns(raw), "{raw:?} must not warn");
    }
    for raw in [
        Some("abc"),
        Some("-5"),
        Some("99999999999999999999"),
        Some("7s"),
    ] {
        assert!(execute::runner_timeout_warns(raw), "{raw:?} must warn");
    }
}

#[test]
fn the_warning_names_the_variable_and_says_no_limit_applies() {
    assert_eq!(
        execute::RUNNER_TIMEOUT_WARNING,
        "warning: ignored AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS (not a whole number of \
         seconds above 0); no limit applies"
    );
}

// The warning is a fixed string (pinned above), so it cannot print the
// value, and a process-wide `Once` prints it at most once. An in-process
// stderr capture could not check "once": another test may already have
// fired the `Once`, and redirecting fd 2 would also swallow other tests'
// output.

// -------------------------------------------------------------------------
// wait_for_recipe_runner(None): no deadline arm
// -------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn an_unbounded_wait_returns_the_childs_exit() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let gate = Gate::new(temp.path(), "gate");
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", "read _ < \"$1\"; exit 3", "sh"])
        .arg(gate.path())
        .spawn()
        .expect("spawn child");
    let mut child = ChildGuard(child);
    let release = gate.release_in_background();
    let got = execute::wait_for_recipe_runner(&mut child.0, None).expect("wait");
    release.join().expect("release thread");
    match got {
        execute::RunnerWait::Exited(status) => assert_eq!(status.code(), Some(3)),
        _ => panic!("an unbounded wait must end only on exit or stop"),
    }
}

#[cfg(unix)]
#[test]
fn a_bounded_wait_reports_the_limit_it_applied() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    // Never released: only the limit can end this wait.
    let gate = Gate::new(temp.path(), "gate");
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", "read _ < \"$1\"", "sh"])
        .arg(gate.path())
        .spawn()
        .expect("spawn child");
    let mut child = ChildGuard(child);
    let got = execute::wait_for_recipe_runner(&mut child.0, Some(Duration::from_millis(300)))
        .expect("wait");
    match got {
        execute::RunnerWait::TimedOut(limit) => {
            assert_eq!(limit, Duration::from_millis(300))
        }
        _ => panic!("a bounded wait on a blocked child must time out"),
    }
}

// -------------------------------------------------------------------------
// AC3 / AC4: end to end through `execute_recipe_via_rust`
// -------------------------------------------------------------------------

#[cfg(unix)]
fn write_stub(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).expect("write stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

#[cfg(unix)]
fn gated_success_stub(gate: &Gate, recipe_name: &str) -> String {
    format!(
        "#!/bin/sh\nread _ < {}\ncat <<EOF\n{{\"recipe_name\":\"{recipe_name}\",\"success\":true,\"step_results\":[],\"context\":{{}}}}\nEOF\n",
        gate.quoted()
    )
}

#[cfg(unix)]
#[test]
fn ac3_with_the_variable_unset_a_blocked_runner_is_not_timed_out() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _fixture = fixture_environment(&temp.path().join("work"));
    let gate = Gate::new(temp.path(), "gate");
    let runner = temp.path().join("recipe-runner-rs");
    write_stub(&runner, &gated_success_stub(&gate, "no-limit"));
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: no-limit\nsteps: []\n").expect("recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

    let release = gate.release_in_background();
    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        false,
        temp.path(),
        &[],
        None,
    );
    drop(gate);
    release.join().expect("release thread");
    let result = result.expect("an unset limit must let a blocked runner finish");
    assert!(result.success);
    assert_eq!(result.recipe_name, "no-limit");
}

#[cfg(unix)]
#[test]
fn ac3_zero_and_garbage_values_also_mean_no_limit() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _fixture = fixture_environment(&temp.path().join("work"));
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: zero\nsteps: []\n").expect("recipe");
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));

    for (i, value) in ["0", "", "not-a-number"].into_iter().enumerate() {
        let gate = Gate::new(temp.path(), &format!("gate-{i}"));
        let runner = temp.path().join(format!("recipe-runner-rs-{i}"));
        write_stub(&runner, &gated_success_stub(&gate, "zero"));
        let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
        let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, value);

        let release = gate.release_in_background();
        let result = execute::execute_recipe_via_rust(
            &recipe,
            &BTreeMap::new(),
            false,
            false,
            temp.path(),
            &[],
            None,
        );
        drop(gate);
        release.join().expect("release thread");
        let result = result.unwrap_or_else(|e| panic!("={value:?} must mean no limit: {e:#}"));
        assert!(result.success, "={value:?}");
        assert_eq!(result.recipe_name, "zero", "={value:?}");
    }
}

#[cfg(unix)]
#[test]
fn ac4_an_explicit_limit_times_out_with_the_existing_message() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _fixture = fixture_environment(&temp.path().join("work"));
    // Never released: only the 1 s limit can end this run.
    let gate = Gate::new(temp.path(), "gate");
    let pidfile = temp.path().join("stub.pid");
    let runner = temp.path().join("recipe-runner-rs");
    write_stub(
        &runner,
        &format!(
            "#!/bin/sh\numask 077\necho $$ > {}\nread _ < {}\n",
            sh_quote(&pidfile),
            gate.quoted()
        ),
    );
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: limited\nsteps: []\n").expect("recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "1");
    let _grace_env = EnvVarGuard::set(TEARDOWN_GRACE_ENV, "0");

    let err = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        false,
        temp.path(),
        &[],
        None,
    )
    .expect_err("=1 must time out a runner that never finishes");
    let text = format!("{err:#}");
    assert!(
        text.contains("timed out after 1s"),
        "the message formats the applied limit: {text}"
    );
    let verdict = super::failure_class::classify_error(&err);
    assert_eq!(
        verdict.class,
        super::failure_class::FailureClass::Indeterminate,
        "a runner timeout keeps its existing classification: {text}"
    );

    // Teardown: if the stub got as far as recording its PID, that process
    // must be gone. Only signal 0 is ever sent.
    if let Some(pid) = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|s| s.trim().parse::<libc::pid_t>().ok())
    {
        // SAFETY: kill with signal 0 sends nothing; it only checks existence.
        let rc = unsafe { libc::kill(pid, 0) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert!(
            rc == -1 && errno == Some(libc::ESRCH),
            "the timed-out runner (pid {pid}) must be torn down"
        );
    }
}
