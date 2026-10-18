//! Run one test alone in a fresh process of the `cli_integration` binary.
//!
//! Every module of `cli_integration` shares one process under `cargo test`.
//! Two kinds of test must not run in that shared process:
//!
//! - Tests that change process-global state (environment variables or the
//!   working directory). `std::env::set_var` is sound only while no other
//!   thread reads the environment, and every sibling test that spawns a child
//!   reads it. The `child_*` functions below are the only way a test in this
//!   binary may change that state, and they panic outside a quiet child.
//! - Tests that count process-wide resources, such as the entries of
//!   `/proc/self/fd`, which sibling tests change concurrently.
//!
//! `in_quiet_process` re-runs the calling test by itself in a child process
//! of the same binary (`--exact <name> --test-threads=1`), under both
//! `cargo test` and `cargo nextest`.

use super::test_path::test_path;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Set in the child process, where the test body actually runs, to the PID of
/// the process that spawned it.
const QUIET_CHILD_ENV: &str = "AMPLIHACK_CLI_INTEGRATION_QUIET_CHILD";

/// Whether this process is a quiet child, decided once.
///
/// The marker counts only if it names this process's parent, so a value
/// exported into a shell, or inherited from a quiet child by a grandchild,
/// can never make a test body run in a shared process or switch off the
/// `child_*` checks. A marker naming some other PID panics. A genuine child
/// removes the marker from its own environment at once, so nothing it spawns
/// inherits it.
fn in_quiet_child() -> bool {
    static QUIET: OnceLock<bool> = OnceLock::new();
    *QUIET.get_or_init(|| {
        let Some(marker) = std::env::var_os(QUIET_CHILD_ENV) else {
            return false;
        };
        let parent = std::os::unix::process::parent_id().to_string();
        assert!(
            marker == *parent,
            "{QUIET_CHILD_ENV}={marker:?} is set but this process's parent is {parent}: \
             the marker was inherited or exported, not set by in_quiet_process. \
             Unset it; refusing to run test bodies in a shared process."
        );
        // SAFETY: this is a genuine quiet child (its parent set the marker), in
        // which the calling test is the only test and runs on libtest's one
        // test thread, before the body starts anything that reads the
        // environment.
        unsafe { std::env::remove_var(QUIET_CHILD_ENV) };
        true
    })
}

/// Runs `body` in a child process in which test `name` of the module at
/// `module_path` (pass `module_path!()`) is the only test. In the child
/// itself, runs `body` directly.
pub fn in_quiet_process(module_path: &str, name: &str, body: impl FnOnce()) {
    if in_quiet_child() {
        body();
        return;
    }
    let name = test_path(module_path, name);
    let exe = std::env::current_exe().expect("resolve the running test binary");
    let output = Command::new(&exe)
        .args(["--exact", &name, "--test-threads=1", "--nocapture"])
        .env(QUIET_CHILD_ENV, std::process::id().to_string())
        .output()
        .unwrap_or_else(|e| panic!("cannot re-run {name} in {}: {e}", exe.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{name} failed in its own process ({}):\n--- stdout\n{stdout}\n--- stderr\n{stderr}",
        output.status
    );
    // `--exact` with a wrong name exits 0 having run nothing; insist on one pass.
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "re-running {name} in its own process ran no test; is the name right?\n{stdout}"
    );
}

fn require_quiet_child(what: &str) {
    assert!(
        in_quiet_child(),
        "{what} changes process-global state; call it only inside in_quiet_process"
    );
}

/// `std::env::set_var`, allowed only inside an `in_quiet_process` child.
pub fn child_set_var(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
    require_quiet_child("child_set_var");
    // SAFETY: this is a quiet child, where the calling test is the only test
    // and libtest runs it on its one test thread. Nothing else in the process
    // reads the environment concurrently.
    unsafe { std::env::set_var(key, value) };
}

/// `std::env::remove_var`, allowed only inside an `in_quiet_process` child.
pub fn child_remove_var(key: impl AsRef<OsStr>) {
    require_quiet_child("child_remove_var");
    // SAFETY: as in `child_set_var`.
    unsafe { std::env::remove_var(key) };
}

/// `std::env::set_current_dir`, allowed only inside an `in_quiet_process` child.
pub fn child_set_current_dir(path: impl AsRef<Path>) {
    require_quiet_child("child_set_current_dir");
    let path = path.as_ref();
    std::env::set_current_dir(path)
        .unwrap_or_else(|e| panic!("cannot change directory to {}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::QUIET_CHILD_ENV;
    use std::process::Command;

    /// A marker that does not name the parent (here an exported `0`) must make
    /// a quiet test fail loudly, not run its body in this process.
    #[test]
    fn a_marker_that_does_not_name_the_parent_is_refused() {
        let quiet_test = "bugfix_341_install_freshness::\
                          install_stages_amplifier_bundle_from_local_checkout_not_stale_source";
        let out = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", quiet_test, "--test-threads=1", "--nocapture"])
            .env(QUIET_CHILD_ENV, "0")
            .output()
            .expect("re-run the test binary");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.status.success(),
            "the forged marker was obeyed:\n{text}"
        );
        assert!(
            text.contains("refusing to run test bodies in a shared process"),
            "expected the refusal message:\n{text}"
        );
    }
}
