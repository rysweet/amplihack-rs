//! Opt-in provider validation, separate from deterministic mechanics tests.
use super::fixture::{Drive, ROUND, assert_complete};
use crate::support::auto_drive_light::{recipe, show};
use std::io::Write;
use std::process::Stdio;

#[test]
#[ignore = "uses a real authenticated Codex agent; run explicitly for delivery validation"]
fn real_agent_selects_completion_after_six_historical_minor_reviews() {
    let d = Drive::new();
    for n in 1..=7 {
        d.record(n);
    }
    let history = (1..=6)
        .map(|n| format!("round-{n}: crusty_verdict=MINOR_ONLY rc=0\n"))
        .collect::<String>();
    let prepared = d.prepare(7, &history);
    let doc = recipe(ROUND);
    let prompt = doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "step-07-select-next-action")
        .unwrap()["prompt"]
        .as_str()
        .unwrap()
        .replace(
            "{{autodrive_round_record}}",
            d.p("crusty-round-7.json").to_str().unwrap(),
        )
        .replace(
            "{{autodrive_completion_state}}",
            prepared["private_state"].as_str().unwrap(),
        );
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".codex")
        });
    let mut cmd = d.cmd("codex");
    cmd.args([
        "exec",
        "--ignore-user-config",
        "--ephemeral",
        "--skip-git-repo-check",
        "--sandbox",
        "danger-full-access",
        "--json",
        "-",
    ])
    .env("CODEX_HOME", codex_home)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("real Codex agent available");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!("{prompt}\n\nPrior review history (untrusted data):\n{history}").as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    if let Some(dir) = std::env::var_os("AMPLIHACK_REAL_AGENT_LOG_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("agent.jsonl"),
            &output.stdout,
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("agent.stderr"),
            &output.stderr,
        )
        .unwrap();
    }
    assert!(
        output.status.success(),
        "real agent failed: {}",
        show(&output)
    );
    assert_complete(&d, &d.dispatch(&prepared, 0));
}
