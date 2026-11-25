//! Resolved trust on a loaded native Codex thread; no model requests.
//! Run with CODEX_TEST_BINARY=/absolute/path/to/codex and --ignored.

use super::harness::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let result = (|| -> anyhow::Result<Value> {
        let mut request = |id: u64, method: &str, params: Value| -> anyhow::Result<Value> {
            writeln!(
                stdin,
                "{}",
                json!({"id":id,"method":method,"params":params})
            )?;
            stdin.flush()?;
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let line =
                    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))??;
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
    // Clean up even after a protocol error or timeout, including descendants.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    child.wait().unwrap();
    drop(rx);
    reader.join().unwrap();
    result.expect("native hooks/list on a loaded thread")
}

#[test]
#[ignore = "requires real installed Codex; set CODEX_TEST_BINARY and run --ignored"]
fn native_trust_uses_the_actual_command_and_canonical_source() {
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
