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
// * AC3: with the variable unset, a runner that sleeps about 2 s completes.
// * AC4: with `=1`, a runner that sleeps 30 s ends within a few seconds with
//   `timed out after 1s` and the existing classification.
//
// Every test that touches the variable holds `home_env_lock()` and restores
// it. No test sleeps longer than about 3 s.
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
use std::time::{Duration, Instant};

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

#[cfg(unix)]
#[test]
fn the_warning_is_printed_at_most_once_and_never_prints_the_value() {
    use std::io::Write as _;
    use std::os::unix::io::AsRawFd;

    let _guard = lock();
    let secret = "s3cr3t-token-value";
    let _env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, secret);

    let file = tempfile::NamedTempFile::new().expect("stderr capture file");
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    assert!(saved >= 0);
    assert!(unsafe { libc::dup2(file.as_file().as_raw_fd(), libc::STDERR_FILENO) } >= 0);
    let first = execute::recipe_runner_timeout();
    let second = execute::recipe_runner_timeout();
    let third = execute::recipe_runner_timeout();
    let _ = std::io::stderr().lock().flush();
    assert!(unsafe { libc::dup2(saved, libc::STDERR_FILENO) } >= 0);
    unsafe { libc::close(saved) };
    let stderr = std::fs::read_to_string(file.path()).expect("read capture");

    assert_eq!((first, second, third), (None, None, None));
    assert!(
        stderr.matches(execute::RUNNER_TIMEOUT_WARNING).count() <= 1,
        "printed once per process at most: {stderr:?}"
    );
    assert!(!stderr.contains(secret), "the value must never be printed");
}

// -------------------------------------------------------------------------
// wait_for_recipe_runner(None): no deadline arm
// -------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn an_unbounded_wait_returns_the_childs_exit() {
    let _guard = lock();
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 1; exit 3"])
        .spawn()
        .expect("spawn child");
    let started = Instant::now();
    let got = execute::wait_for_recipe_runner(&mut child, None).expect("wait");
    match got {
        execute::RunnerWait::Exited(status) => assert_eq!(status.code(), Some(3)),
        _ => panic!("an unbounded wait must end only on exit or stop"),
    }
    assert!(started.elapsed() >= Duration::from_millis(900));
}

#[cfg(unix)]
#[test]
fn a_bounded_wait_reports_the_limit_it_applied() {
    let _guard = lock();
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .expect("spawn child");
    let started = Instant::now();
    let got = execute::wait_for_recipe_runner(&mut child, Some(Duration::from_millis(300)))
        .expect("wait");
    let _ = child.kill();
    let _ = child.wait();
    match got {
        execute::RunnerWait::TimedOut(limit) => {
            assert_eq!(limit, Duration::from_millis(300))
        }
        _ => panic!("a bounded wait on a hung child must time out"),
    }
    assert!(started.elapsed() < Duration::from_secs(3));
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
#[test]
fn ac3_with_the_variable_unset_a_slow_runner_is_not_timed_out() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _fixture = fixture_environment(&temp.path().join("work"));
    let runner = temp.path().join("recipe-runner-rs");
    write_stub(
        &runner,
        "#!/bin/sh\n/bin/sleep 2\ncat <<EOF\n{\"recipe_name\":\"no-limit\",\"success\":true,\"step_results\":[],\"context\":{}}\nEOF\n",
    );
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: no-limit\nsteps: []\n").expect("recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        false,
        temp.path(),
        &[],
        None,
    )
    .expect("an unset limit must let a 2 s runner finish");
    assert!(result.success);
    assert_eq!(result.recipe_name, "no-limit");
}

#[cfg(unix)]
#[test]
fn ac3_zero_and_garbage_values_also_mean_no_limit() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _fixture = fixture_environment(&temp.path().join("work"));
    let runner = temp.path().join("recipe-runner-rs");
    write_stub(
        &runner,
        "#!/bin/sh\n/bin/sleep 1\ncat <<EOF\n{\"recipe_name\":\"zero\",\"success\":true,\"step_results\":[],\"context\":{}}\nEOF\n",
    );
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: zero\nsteps: []\n").expect("recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    for value in ["0", "", "not-a-number"] {
        let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, value);
        let result = execute::execute_recipe_via_rust(
            &recipe,
            &BTreeMap::new(),
            false,
            false,
            temp.path(),
            &[],
            None,
        )
        .unwrap_or_else(|e| panic!("={value:?} must mean no limit: {e:#}"));
        assert!(result.success, "={value:?}");
    }
}

#[cfg(unix)]
#[test]
fn ac4_an_explicit_limit_times_out_with_the_existing_message() {
    let _guard = lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let _fixture = fixture_environment(&temp.path().join("work"));
    let runner = temp.path().join("recipe-runner-rs");
    write_stub(&runner, "#!/bin/sh\n/bin/sleep 30\n");
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: limited\nsteps: []\n").expect("recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "1");
    let _grace_env = EnvVarGuard::set(TEARDOWN_GRACE_ENV, "0");

    let started = Instant::now();
    let err = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        false,
        temp.path(),
        &[],
        None,
    )
    .expect_err("=1 must time out a 30 s runner");
    let elapsed = started.elapsed();
    let text = format!("{err:#}");
    assert!(
        text.contains("timed out after 1s"),
        "the message formats the applied limit: {text}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "ended within a few seconds, took {elapsed:?}"
    );
}
