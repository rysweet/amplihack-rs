//! Build `cargo build -p amplihack --bin amplihack` first, or set AMPLIHACK_TEST_BINARY.
//! Recipe agents enter through `amplihack codex -- exec`; rejection must precede work.
#![cfg(unix)]
use std::{fs, process::Stdio};

#[test]
fn recipe_codex_startup_rejects_missing_helper_before_agent_work() {
    startup_rejection(true);
}

#[test]
fn disabled_noninteractive_discovery_refuses_recovery_before_agent_work() {
    startup_rejection(false);
}

fn startup_rejection(explicit_override: bool) {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let codex = tools.join("codex");
    crate::support::exec_stub::write_executable(
        &codex,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.166.0; exit 0; fi\nprintf agent-work > \"$HOME/agent-work\"\n",
        0o700,
    );
    crate::support::exec_stub::write_executable(
        &tools.join("npm"),
        "#!/bin/sh\nprintf forbidden > \"$HOME/installer\"\nexit 91\n",
        0o700,
    );
    let cli = std::env::var_os("AMPLIHACK_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("amplihack")
        });
    let mut command = crate::isolated_amplihack::isolated(cli);
    if explicit_override {
        command.env("AMPLIHACK_CODEX_BINARY_PATH", &codex);
    }
    let output = command
        .env("HOME", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("PATH", &tools)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .args(["codex", "--no-reflection", "--", "exec", "-"])
        .stdin(Stdio::null())
        .current_dir(home.path())
        .output()
        .unwrap();
    assert!(
        !home.path().join("agent-work").exists(),
        "agent work ran before validation"
    );
    assert!(
        !home.path().join("installer").exists(),
        "disabled repair invoked npm"
    );
    assert!(!output.status.success(), "recipe startup must fail");
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains(&codex.display().to_string()),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("codex-code-mode-host"), "{diagnostic}");
    if explicit_override {
        assert!(
            diagnostic.contains("unset AMPLIHACK_CODEX_BINARY_PATH CODEX_BINARY_PATH"),
            "{diagnostic}"
        );
    }
}

#[test]
fn managed_recovery_revalidates_incomplete_replacement() {
    failed_recovery(false);
}

#[test]
fn managed_installer_failure_retains_original_failure_and_is_not_retried() {
    failed_recovery(true);
}

fn failed_recovery(installer_fails: bool) {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let codex = tools.join("codex");
    crate::support::exec_stub::write_executable(
        &codex,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.166.0; exit 0; fi\nprintf forbidden > \"$HOME/agent-work\"\n",
        0o700,
    );
    crate::support::exec_stub::write_executable(
        &tools.join("npm"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 10.9.0; exit 0; fi\nprintf '%s\\n' \"$*\" >> \"$HOME/installs\"\nif [ \"$FAIL_INSTALLER\" = 1 ]; then exit 42; fi\n/bin/cp \"$HOME/tools/codex\" \"$HOME/.npm-global/bin/codex\"\n/bin/chmod 700 \"$HOME/.npm-global/bin/codex\"\n",
        0o700,
    );
    let cli = std::env::var_os("AMPLIHACK_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("amplihack")
        });
    let output = crate::isolated_amplihack::isolated(cli)
        .env("HOME", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("PATH", &tools)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("FAIL_INSTALLER", if installer_fails { "1" } else { "0" })
        .args(["codex", "--no-reflection", "--", "exec", "-"])
        .stdin(Stdio::null())
        .current_dir(home.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!home.path().join("agent-work").exists());
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains(&codex.display().to_string()),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("codex-code-mode-host"), "{diagnostic}");
    let installs = fs::read_to_string(home.path().join("installs"))
        .unwrap_or_else(|e| panic!("{e}: {diagnostic}"));
    assert_eq!(installs.lines().count(), 1, "{installs}");
    assert!(
        !installs.contains("--omit=optional"),
        "Codex needs its platform dependency"
    );
    assert!(installs.contains("@openai/codex"));
    if installer_fails {
        assert!(diagnostic.contains("42"), "{diagnostic}");
    } else {
        assert!(
            diagnostic.contains(
                &home
                    .path()
                    .join(".npm-global/bin/codex")
                    .display()
                    .to_string()
            ),
            "{diagnostic}"
        );
    }
}
