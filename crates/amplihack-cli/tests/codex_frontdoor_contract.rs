//! Cold-start must reach existing tool installation before native registration.
#![cfg(unix)]
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
    use std::io::Write;
    use std::process::Stdio;
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
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
    for staged in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let tools = home.path().join("tools");
        fs::create_dir(&tools).unwrap();
        for tool in [
            "sh",
            "mkdir",
            "cat",
            "cp",
            "chmod",
            "dirname",
            "rm",
            "mv",
            "git",
            "uname",
            "head",
            "sed",
            "date",
            "cut",
            "sha256sum",
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
        executable(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n");
        executable(&tools.join("amplihack-hooks"), "#!/bin/sh\necho '{}'\n");
        executable(
            &tools.join("recipe-runner-rs"),
            "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}'\n",
        );
        // npm installs into the fixture PATH, which production resolves again.
        executable(
            &tools.join("npm"),
            r#"#!/bin/sh
case "$*" in
 *install*@openai/codex*) echo install >> "$HOME/events"; cp "$HOME/client-template" "$HOME/tools/codex"; chmod +x "$HOME/tools/codex";;
 *install*) echo other-install >> "$HOME/events";;
 *view*) echo 1.0.0;;
 *--version*) echo 10.9.0;;
esac
"#,
        );
        executable(
            &home.path().join("client-template"),
            r#"#!/bin/sh
printf '%s\n' "$0|$*" >> "$HOME/events"
case "$*" in
 *--version*) echo 'codex-cli 0.160.0';;
 'plugin list --json')
  if [ -f "$HOME/registered" ]; then
   printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"source":{"path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
  else printf '{"installed":[]}\n'; fi;;
 'plugin marketplace add '*) echo '{"ok":true}';;
 'plugin add '*) echo yes > "$HOME/registered"; echo '{"ok":true}';;
 'plugin remove '*) rm -f "$HOME/registered"; echo '{"ok":true}';;
 *) printf '%s\n' "$@" > "$HOME/launch-argv"; cat > "$HOME/launch-stdin"; echo COLD_LAUNCH_OK;;
esac
"#,
        );
        // Framework is staged by a different provider, before Codex is present.
        if staged {
            let out = Command::new(&cli)
                .args(["install", "--local", source.to_str().unwrap()])
                .env("HOME", home.path())
                .env("PATH", &tools)
                .env("AMPLIHACK_AGENT_BINARY", "claude")
                .env("AMPLIHACK_SOURCE_ROOT", &source)
                .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
                .env(
                    "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                    tools.join("amplihack-hooks"),
                )
                .env_remove("AMPLIHACK_CODEX_BINARY_PATH")
                .env_remove("CODEX_BINARY_PATH")
                .env_remove("CODEX_HOME")
                .env("RECIPE_RUNNER_RS_PATH", tools.join("recipe-runner-rs"))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "staging failed: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(!tools.join("codex").exists());
        }
        // Bootstrap requires a terminal. Only stdin uses a PTY; output remains captured.
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut input = unsafe { fs::File::from_raw_fd(master) };
        let terminal = unsafe { fs::File::from_raw_fd(slave) };
        let child = Command::new(&cli)
            .args([
                "codex",
                "--no-reflection",
                "--",
                "exec",
                "--model",
                "fixture-model",
                "-",
            ])
            .env("HOME", home.path())
            .env("CODEX_HOME", home.path().join(".codex"))
            .env("PATH", &tools)
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_SOURCE_ROOT", &source)
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                tools.join("amplihack-hooks"),
            )
            .env_remove("AMPLIHACK_CODEX_BINARY_PATH")
            .env_remove("CODEX_BINARY_PATH")
            .env_remove("AMPLIHACK_NONINTERACTIVE")
            .env_remove("CI")
            .env_remove("CODEX_THREAD_ID")
            .env("RECIPE_RUNNER_RS_PATH", tools.join("recipe-runner-rs"))
            .current_dir(home.path())
            .stdin(Stdio::from(terminal))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        input
            .write_all(b"cold stdin\nUnicode: \xc3\xa9\n\x04")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "staged={staged}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let events = fs::read_to_string(home.path().join("events")).unwrap();
        assert!(
            events
                .find("install\n")
                .is_some_and(|i| events.find("plugin list").is_some_and(|p| i < p)),
            "{events}"
        );
        assert!(
            events.contains(&format!("{}/codex|plugin add", tools.display())),
            "{events}"
        );
        assert_eq!(
            fs::read(home.path().join("launch-stdin")).unwrap(),
            b"cold stdin\nUnicode: \xc3\xa9\n"
        );
        let argv = fs::read_to_string(home.path().join("launch-argv")).unwrap();
        assert!(argv.contains("exec\n--model\nfixture-model\n-\n"), "{argv}");
        assert!(
            home.path()
                .join(".amplihack/codex/ownership.json")
                .is_file()
        );
        assert!(
            home.path()
                .join(".amplihack/codex/market/plugin/bin/hook")
                .is_file()
        );
        let hooks: serde_json::Value =
            serde_json::from_slice(&fs::read(home.path().join(".codex/hooks.json")).unwrap())
                .unwrap();
        assert!(hooks["hooks"]["SessionStart"].is_array());
        assert!(home.path().join("registered").is_file());
    }
}
