//! Native app-server inventory protocol with bounded reads and owned cleanup.
use super::*;

// Query the installed CLI's discovery protocol, independently of Rust loaders.
pub(super) fn native_inventory(binary: &str, cwd: &Path) -> serde_json::Value {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(binary)
        .process_group(0)
        .args(["app-server", "--stdio"])
        .current_dir(cwd)
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
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let requests = [
            serde_json::json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"amplihack-acceptance","version":"1"},"capabilities":{"experimentalApi":true}}}),
            serde_json::json!({"id":2,"method":"skills/list","params":{"cwds":[cwd],"forceReload":true}}),
            serde_json::json!({"id":3,"method":"hooks/list","params":{"cwds":[cwd]}}),
        ];
        let mut inventory = serde_json::json!({});
        for request in requests {
            writeln!(stdin, "{request}")?;
            stdin.flush()?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let line = rx.recv_timeout(
                    deadline.saturating_duration_since(std::time::Instant::now()),
                )??;
                let response: serde_json::Value = serde_json::from_str(&line)?;
                if response["id"] == request["id"] {
                    anyhow::ensure!(
                        response.get("error").is_none(),
                        "native inventory error: {response}"
                    );
                    inventory[request["method"].as_str().unwrap()] = response["result"].clone();
                    break;
                }
            }
            if request["id"] == 1 {
                writeln!(stdin, "{{\"method\":\"initialized\",\"params\":{{}}}}")?;
                stdin.flush()?;
            }
        }
        Ok(inventory)
    })();
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    child.wait().unwrap();
    drop(rx);
    reader.join().unwrap();
    result.unwrap()
}
