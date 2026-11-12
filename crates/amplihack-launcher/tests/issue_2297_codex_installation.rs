//! Legacy public launcher contracts. Child test processes isolate all environment changes.
#![cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn scenario(name: &str) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("codex"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.166.0; exit 0; fi\nprintf session > \"$HOME/session\"\n",
    );
    // Keep discovery independent of host tools and PATH.
    executable(
        &bin.join("which"),
        "#!/bin/sh\nprintf '%s/bin/codex\\n' \"$HOME\"\n",
    );
    executable(
        &bin.join("npm"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/installs\"\ncase \"$CASE\" in\n repair) printf '#!/bin/sh\\nexit 0\\n' > \"$HOME/bin/codex-code-mode-host\"; /bin/chmod 755 \"$HOME/bin/codex-code-mode-host\";;\n installer_failure) echo fake-installer-failed >&2; exit 42;;\nesac\n",
    );
    if name == "complete" {
        executable(&bin.join("codex-code-mode-host"), "#!/bin/sh\nexit 0\n");
    }
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.env_clear();
    if name == "override" {
        child.env("AMPLIHACK_CODEX_BINARY_PATH", bin.join("codex"));
    }
    let output = child
        .args(["--exact", "child_contract", "--nocapture"])
        .env("HOME", root)
        .env("CODEX_HOME", root.join(".codex"))
        .env("PATH", &bin)
        .env("CASE", name)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn incomplete_install_is_not_installed() {
    scenario("check");
}
#[test]
fn disabled_recovery_rejects_before_session() {
    scenario("disabled");
}
#[test]
fn complete_install_launches_retained_absolute_target() {
    scenario("complete");
}
#[test]
fn unusable_install_is_repaired_once_before_launch() {
    scenario("repair");
}
#[test]
fn successful_installer_with_incomplete_replacement_is_rejected() {
    scenario("bad_replacement");
}
#[test]
fn installer_failure_preserves_original_diagnostic() {
    scenario("installer_failure");
}

#[test]
fn external_override_refuses_ineffective_managed_repair() {
    scenario("override");
}

#[test]
fn child_contract() {
    let Ok(case) = std::env::var("CASE") else {
        return;
    };
    let home = std::env::var_os("HOME").unwrap();
    let root = Path::new(&home);
    let codex = root.join("bin/codex");
    if case == "check" {
        assert!(
            !amplihack_launcher::codex::check_codex().installed,
            "--version is insufficient"
        );
    } else {
        let result = amplihack_launcher::codex::ensure_and_build(
            "task",
            root,
            &[],
            case != "disabled" && case != "complete",
        );
        if case == "complete" || case == "repair" {
            let mut command = result.expect("complete or repaired install must launch");
            assert_eq!(
                command.get_program(),
                codex.as_os_str(),
                "retain validated target"
            );
            // Changing ambient search paths after validation must not redirect launch.
            command.env("PATH", root.join("absent-path"));
            assert!(command.status().unwrap().success());
            assert!(root.join("session").exists());
        } else {
            let diagnostic = result.expect_err("must reject before session").to_string();
            assert!(
                diagnostic.contains(&codex.display().to_string()),
                "{diagnostic}"
            );
            assert!(diagnostic.contains("codex-code-mode-host"), "{diagnostic}");
            assert!(
                diagnostic.contains("npm") || diagnostic.contains("repair"),
                "{diagnostic}"
            );
            if case == "installer_failure" {
                assert!(
                    diagnostic.contains("42") || diagnostic.contains("fake-installer-failed"),
                    "{diagnostic}"
                );
            }
        }
    }
    if case != "complete" && case != "repair" {
        assert!(!root.join("session").exists());
    }
    if matches!(
        case.as_str(),
        "repair" | "bad_replacement" | "installer_failure"
    ) {
        let calls = fs::read_to_string(root.join("installs"))
            .expect("unusable discovery must attempt repair");
        assert_eq!(calls.lines().count(), 1, "only one recovery attempt");
        assert!(calls.contains("install -g @openai/codex"));
    } else {
        assert!(!root.join("installs").exists());
    }
}
