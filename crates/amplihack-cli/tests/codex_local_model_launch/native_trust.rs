//! Resolved trust on a loaded native Codex thread; no model requests.
//! Run with CODEX_TEST_BINARY=/absolute/path/to/codex and --ignored.

use super::harness::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn resolved_hooks(f: &Fixture) -> Value {
    let binary = std::env::var("CODEX_TEST_BINARY").expect("set CODEX_TEST_BINARY");
    assert!(std::path::Path::new(&binary).is_absolute());
    let mut child = Command::new(binary)
        .args(["-c", HOOKS_ON, "app-server", "--stdio"])
        .env_clear()
        .env("HOME", f.home())
        .env("CODEX_HOME", f.generated(NAME))
        .env("PATH", std::env::var_os("PATH").unwrap())
        .current_dir(f.home())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let result = (|| -> anyhow::Result<Value> {
        let mut request = |id: u64, method: &str, params: Value| -> anyhow::Result<Value> {
            writeln!(
                stdin,
                "{}",
                json!({"id":id,"method":method,"params":params})
            )?;
            stdin.flush()?;
            loop {
                let mut line = String::new();
                anyhow::ensure!(stdout.read_line(&mut line)? > 0, "Codex closed its output");
                let response: Value = serde_json::from_str(&line)?;
                if response["id"] == id {
                    anyhow::ensure!(response.get("error").is_none(), "{response}");
                    if id == 1 {
                        writeln!(stdin, "{}", json!({"method":"initialized","params":{}}))?;
                        stdin.flush()?;
                    }
                    return Ok(response["result"].clone());
                }
            }
        };
        request(
            1,
            "initialize",
            json!({
                "clientInfo":{"name":"amplihack-trust-regression","version":"1"},
                "capabilities":{"experimentalApi":true}
            }),
        )?;
        let thread = request(2, "thread/start", json!({"cwd":f.home().to_str().unwrap()}))?;
        request(3, "hooks/list", json!({"threadId":thread["thread"]["id"]}))
    })();
    // Reap only the server this test started, including after a protocol error.
    child.kill().unwrap();
    child.wait().unwrap();
    result.expect("native hooks/list on a loaded thread")
}

/// The Codex versions whose hook-trust format amplihack has checked, as
/// `KNOWN_CODEX_VERSIONS` in the launcher gives them.
const KNOWN_CODEX_VERSIONS: [&str; 1] = ["0.166.0"];

/// The `--version` output of the Codex under test.
fn native_version() -> String {
    let binary = std::env::var("CODEX_TEST_BINARY").expect("set CODEX_TEST_BINARY");
    let out = Command::new(binary)
        .arg("--version")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
#[ignore = "requires real installed Codex; set CODEX_TEST_BINARY and run --ignored"]
fn native_trust_uses_the_actual_command_and_canonical_source() {
    let version = native_version();
    let known = version
        .split_whitespace()
        .last()
        .is_some_and(|v| KNOWN_CODEX_VERSIONS.contains(&v));
    if !known {
        // On a Codex whose trust format is not known, an interactive local
        // launch is rejected with the version, never started without the
        // command check.
        let f = Fixture::new();
        fs::write(f.home().join("codex-version"), format!("{version}\n")).unwrap();
        let launch = f.local(NAME, &[]);
        launch.assert_refused_at_run_time(&f, "an unknown native Codex version");
        let shown = version.split_whitespace().last().unwrap_or_default();
        assert!(launch.stderr().contains(shown), "{}", launch.stderr());
        return;
    }
    for linked in [false, true] {
        let f = Fixture::new();
        if linked {
            let root = f.home().join(".amplihack");
            let target = f.home().join("amplihack-target");
            fs::rename(&root, &target).unwrap();
            std::os::unix::fs::symlink(target, root).unwrap();
        }
        f.local(NAME, &[]).assert_started();
        let hooks_path = f.generated(NAME).join("hooks.json");
        let canonical = fs::canonicalize(&hooks_path).unwrap();
        let original = fs::read_to_string(&hooks_path).unwrap();
        for (command_change, expected_trust) in [
            (None, "trusted"),
            (Some("env  "), "modified"),
            (Some("'env' "), "modified"),
        ] {
            let text = command_change
                .map_or_else(|| original.clone(), |s| original.replacen("env ", s, 1));
            fs::write(&hooks_path, text).unwrap();
            let inventory = resolved_hooks(&f);
            let entry = &inventory["data"][0];
            assert_eq!(entry["errors"], json!([]), "{inventory:#}");
            let hooks = entry["hooks"].as_array().unwrap();
            assert_eq!(hooks.len(), 4, "{inventory:#}");
            let command_check = hooks
                .iter()
                .find(|h| h["trustStatus"] == expected_trust)
                .unwrap_or_else(|| {
                    panic!("linked={linked}, expected {expected_trust}: {inventory:#}")
                });
            assert_eq!(command_check["sourcePath"].as_str(), canonical.to_str());
            assert_eq!(command_check["enabled"], true);
            assert_eq!(
                hooks
                    .iter()
                    .filter(|h| h["trustStatus"] == "untrusted")
                    .count(),
                3,
                "user handlers retain trust review: {inventory:#}"
            );
            let launch = f.launch(
                f.command(&["codex", "--local-model", NAME])
                    .env("CODEX_HOME", f.generated(NAME)),
            );
            if command_change.is_none() {
                launch.assert_started();
                assert_eq!(launch.count(HOOK_TRUST), 0);
            } else {
                assert!(!launch.out.status.success());
                assert!(
                    !launch.started(),
                    "refuse a modified interactive command check"
                );
            }
        }
    }
}
