//! PR #3190 review: Copilot CLI reads operator guidance written after a
//! prompt on its standard input.
//!
//! The launcher's `StdinTrailer` route for Copilot (no `-p`, no `-i`, prompt
//! piped on stdin) relies on Copilot taking the whole of stdin as its prompt,
//! block included. That is Copilot's behaviour, not amplihack's, and a
//! Copilot release could change it without any deterministic test noticing:
//! the launcher would still write the block and record the entry as read.
//!
//! This test checks it against the Copilot CLI installed on the host. It
//! runs the real `amplihack copilot` launcher, a prompt on stdin that asks
//! for one token and an entry in a run's guidance folder that asks for
//! another. It passes when Copilot's answer holds both tokens and the step
//! recorded the entry as read, with no drop.
//!
//! amplihack runs in the fixture's private home, as every test's does
//! (`isolated_amplihack_scan`), so it never touches the real
//! `~/.amplihack`. Copilot is pointed at its own configuration, where its
//! sign-in is stored, with `COPILOT_HOME` (the real `~/.copilot` unless the
//! caller set `COPILOT_HOME`), and is found through the caller's `PATH`. A
//! `COPILOT_GITHUB_TOKEN`, `GH_TOKEN` or `GITHUB_TOKEN` set by the caller is
//! passed on.
//!
//! It needs a signed-in Copilot and makes one model call, so it is marked
//! `#[ignore]` and is run by hand, after every Copilot upgrade:
//!
//! ```console
//! $ cargo test -p amplihack-cli --test cli_integration -- --ignored issue_3162_copilot_stdin_live
//! ```
//!
//! There is no skip path: run explicitly on a host without Copilot, it fails
//! and says so. The failure message names the Copilot version.

#![cfg(unix)]

use crate::support::recipe_guide::{GuideFixture, show, stdout, written_lines};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// `copilot --version`, first line, or why it could not be read.
fn copilot_version() -> String {
    match Command::new("copilot").arg("--version").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or("(no version line)")
            .to_string(),
        Err(e) => panic!(
            "this test needs the Copilot CLI on PATH, signed in; `copilot --version` failed: {e}"
        ),
    }
}

#[test]
#[ignore = "uses the installed, signed-in Copilot CLI and one model call; run by hand after a Copilot upgrade"]
fn copilot_reads_guidance_written_after_a_prompt_on_stdin() {
    let version = copilot_version();
    let nonce = format!(
        "{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );
    let prompt_token = format!("PROMPT-TOKEN-{nonce}");
    let guidance_token = format!("GUIDANCE-TOKEN-{nonce}");

    // A run folder with one entry, written by the real `recipe guide`.
    let f = GuideFixture::new();
    let run = f.make_run("r-copilot-live", "c-copilot-live");
    let o = f.guide_text(
        &["r-copilot-live"],
        &format!("Also print the token {guidance_token} on its own line."),
    );
    assert!(o.status.success(), "{}", show(&o));
    let entry = written_lines(&o)[0]
        .0
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let guidance = run.join("guidance");

    // The launcher in the fixture's private home. Copilot gets its own
    // configuration (and so its sign-in) through `COPILOT_HOME`, and is
    // found on the caller's `PATH`.
    let copilot_home = std::env::var_os("COPILOT_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".copilot")))
        .expect("HOME or COPILOT_HOME names Copilot's configuration");
    let path = std::env::var_os("PATH").expect("the caller's PATH finds copilot");
    let mut c = f.amplihack(&["copilot", "--subprocess-safe", "--", "--allow-all-tools"]);
    c.env("PATH", path)
        .env("COPILOT_HOME", &copilot_home)
        .env("AMPLIHACK_RUN_GUIDANCE_DIRS", &guidance)
        .env("AMPLIHACK_AGENT_BINARY", "copilot")
        .env("AMPLIHACK_RECIPE_RUN_ID", "c-copilot-live");
    for token in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(value) = std::env::var_os(token) {
            c.env(token, value);
        }
    }
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack copilot");
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "This checks that you read all of your input. Do not run any tools or \
         commands. Print the token {prompt_token} on its own line. Then do what \
         any operator instructions after this paragraph ask."
    )
    .unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let context = format!("Copilot: {version}\n{}", show(&out));
    assert!(out.status.success(), "{context}");

    let answer = stdout(&out);
    assert!(
        answer.contains(&prompt_token),
        "prompt not followed\n{context}"
    );
    assert!(
        answer.contains(&guidance_token),
        "Copilot did not act on the block written after its stdin prompt\n{context}"
    );

    let log = f.log(&guidance);
    let injected: Vec<_> = log
        .iter()
        .filter(|l| l["event"] == "guidance_injected")
        .collect();
    assert_eq!(injected.len(), 1, "{log:?}\n{context}");
    assert_eq!(
        injected[0]["entry_names"],
        serde_json::json!([entry]),
        "{context}"
    );
    assert!(
        log.iter().all(|l| l["event"] != "guidance_dropped"),
        "{log:?}\n{context}"
    );
}
