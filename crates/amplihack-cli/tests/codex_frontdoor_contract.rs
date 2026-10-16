//! Cold-start must reach existing tool installation before native registration.
#![cfg(unix)]
#[path = "codex_frontdoor_contract/fixture.rs"]
mod fixture;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};
fn executable(path: &std::path::Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
#[test]
fn fresh_codex_launch_attempts_client_install_before_plugin_preflight() {
    let home = tempfile::tempdir().unwrap();
    let tools = home.path().join("tools");
    fs::create_dir(&tools).unwrap();
    executable(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n");
    executable(
        &tools.join("npm"),
        r#"#!/bin/sh
case "$*" in
  *install*) printf '%s\n' "$*" >> "$HOME/client-install-attempt"; exit 101;;
  *view*) echo 1.0.0;;
  *--version*) echo 10.9.0;;
esac
"#,
    );
    // No host PATH suffix: no accidental host Codex and no network utilities.
    for tool in [
        "sh", "mkdir", "cat", "cp", "chmod", "dirname", "rm", "mv", "git",
    ] {
        let out = Command::new("/bin/sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .unwrap();
        assert!(out.status.success());
        std::os::unix::fs::symlink(
            String::from_utf8(out.stdout).unwrap().trim(),
            tools.join(tool),
        )
        .unwrap();
    }
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
    let out = Command::new(cli)
        .args(["codex", "--no-reflection", "--", "exec", "-"])
        .env("HOME", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("PATH", &tools)
        .env("AMPLIHACK_AGENT_BINARY", "codex")
        .env(
            "AMPLIHACK_SOURCE_ROOT",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap(),
        )
        .env_remove("AMPLIHACK_ORCHESTRATION_BOOTSTRAP")
        .env_remove("AMPLIHACK_DEFAULT_MODEL")
        .env_remove("AMPLIHACK_HOME")
        .env_remove("AMPLIHACK_ASSET_RESOLVER")
        .env_remove("AMPLIHACK_SESSION_ID")
        .env_remove("AMPLIHACK_SESSION_DEPTH")
        .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
        .env_remove("AMPLIHACK_CODEX_BINARY_PATH")
        .env_remove("CODEX_BINARY_PATH")
        .env_remove("AMPLIHACK_NONINTERACTIVE")
        .env_remove("CI")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("RECIPE_RUNNER_RS_PATH")
        .current_dir(home.path())
        .output()
        .unwrap();
    assert!(!out.status.success(), "fake npm deliberately fails install");
    let attempt =
        fs::read_to_string(home.path().join("client-install-attempt")).unwrap_or_else(|_| {
            panic!(
                "Codex install was never reached: stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
    assert!(attempt.contains("@openai/codex"), "{attempt}");
    assert!(!home.path().join(".amplihack/codex/ownership.json").exists());
}
#[test]
fn installer_modules_obey_the_accepted_size_boundary() {
    let install = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/install");
    let old = install.join("codex_plugin.rs");
    let mut modules = vec![];
    if old.exists() {
        modules.push(old);
    }
    let split = install.join("codex_plugin");
    if split.exists() {
        for entry in fs::read_dir(split).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                modules.push(path);
            }
        }
    }
    assert!(!modules.is_empty());
    for module in modules {
        let lines = fs::read_to_string(&module).unwrap().lines().count();
        assert!(
            lines <= 300,
            "{} has {lines} lines; accepted maximum is 300",
            module.display()
        );
    }
}

#[test]
fn cold_client_install_registers_hooks_and_launches_with_fresh_or_staged_framework() {
    fixture::cold_client_install_registers_hooks_and_launches_with_fresh_or_staged_framework();
}
