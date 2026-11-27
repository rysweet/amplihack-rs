//! Run installation tests alone with destinations selected before process start.

const CHILD: &str = "AMPLIHACK_INSTALL_TEST_CHILD";

/// Select exactly one test with an explicit home and no inherited destinations.
pub(crate) fn command(home: &std::path::Path, name: &str) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            name,
            "--nocapture",
            "--test-threads=1",
            "--include-ignored",
        ])
        .env_clear()
        .env("HOME", home)
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env(CHILD, name);
    // The ignored native acceptance test takes an explicit executable input.
    // Its configuration and destinations still come from its fixture HOME.
    if let Some(binary) = std::env::var_os("CODEX_TEST_BINARY") {
        command.env("CODEX_TEST_BINARY", binary);
    }
    command
}

/// Returns true after the parent has run this test in a child. Call before
/// acquiring environment locks or creating installation fixtures, and return
/// from the test when true. The child runs the original test body alone.
pub(crate) fn isolated_install_test() -> bool {
    let thread = std::thread::current();
    let name = thread.name().expect("installation test has a name");
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        return false;
    }

    let home = tempfile::tempdir().unwrap();
    let output = command(home.path(), name)
        .output()
        .expect("start isolated installation test");
    assert!(
        output.status.success(),
        "isolated test {name} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
