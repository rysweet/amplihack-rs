//! TDD red-phase proving tests for the **clean result channel** end to end.
//!
//! This is the load-bearing evidence for the feature: a child floods **stdout**
//! with ANSI colour, a startup banner, and tracing-style log lines that
//! themselves contain JSON braces (a decoy `{ "verdict": "REJECT" }`), while it
//! writes its real answer (`{ "verdict": "MERGE" }`, itself full of ANSI/braces/
//! quotes) to the runner-provided sink named by `AMPLIHACK_RESULT_SINK`.
//!
//! The tests assert the answer is recovered **verbatim** from
//! `ProcessResult.result` with ZERO stdout scraping — no `strip_ansi`, no
//! `extract_json`, no balanced-brace scan, no `serde`. These are exactly the
//! conditions that break `extract_json_payload` today.
//!
//! They fail until the feature exists (`RunOptions::with_result_sink`,
//! `ProcessResult.result`, and the runner threading the sink env through
//! `run_delivered_command`). See docs/reference/clean-result-channel.md.
//!
//! POSIX-only: the fake agent is a `sh -c` script standing in for a real
//! claude/copilot binary that honours the invocation contract.

#![cfg(unix)]

use std::process::{Command as StdCommand, Stdio};
use std::time::Duration;

use amplihack_orchestration::claude_process::{ProcessResult, RunOptions, TokioProcessRunner};
use amplihack_orchestration::result_sink;
use amplihack_utils::prompt_delivery::{DeliveryCaps, PromptDelivery};

/// A fake agent: pours a noisy human/log firehose onto stdout, then — only if
/// the clean channel is enabled — copies its exact answer bytes (`$1`, an
/// answer-source file path delivered via argv) to `$AMPLIHACK_RESULT_SINK`.
///
/// The stdout stream deliberately carries ANSI SGR escapes, a banner, tracing
/// lines with real JSON braces, and a DECOY `{ "verdict": "REJECT" }` blob — so
/// any consumer that brace-scans stdout would recover the WRONG verdict.
const FAKE_AGENT_SCRIPT: &str = r#"
printf '\033[1;33m========== STARTUP BANNER ==========\033[0m\n'
printf '\033[31m2024-07-06T00:00:00Z INFO agent booting {"phase":"init","ok":true}\033[0m\n'
printf '\033[36mprogress: [####------] 40%%\033[0m\n'
printf 'noise { "verdict": "REJECT" } <- decoy brace-blob on stdout\n'
if [ -n "${AMPLIHACK_RESULT_SINK:-}" ]; then
  cat "$1" > "$AMPLIHACK_RESULT_SINK"
fi
"#;

/// The child's real answer. Contains ANSI escapes, JSON braces, a stray `}`,
/// and quotes — the payload shape that defeats stdout scraping.
fn agent_answer() -> String {
    "\u{1b}[32mFINAL\u{1b}[0m { \"verdict\": \"MERGE\", \"reason\": \"clean handoff; contains } and 'quotes'\" }\n"
        .to_string()
}

/// Migration example (mirrors docs/reference/clean-result-channel.md):
/// obtain the agent's answer from the clean channel with NO stdout scraping.
/// No strip_ansi. No extract_json. No brace scan. No serde. No guessing.
fn consume_answer_without_scraping(result: &ProcessResult) -> String {
    result
        .result
        .clone()
        .unwrap_or_else(|| result.output.clone())
}

/// Drive the fake agent through the real runner path. `sink` opts the run into
/// the clean channel; `answer_src` is the file whose exact bytes the agent
/// copies to the sink (delivered to the child as `$1` via argv).
async fn run_fake_agent(
    answer_src: &std::path::Path,
    sink: Option<std::path::PathBuf>,
) -> ProcessResult {
    let runner = TokioProcessRunner::new();
    let mut opts = RunOptions::new(
        answer_src.to_string_lossy().into_owned(),
        "clean-channel-proving".to_string(),
    );
    opts.timeout = Some(Duration::from_secs(10));
    if let Some(path) = sink {
        opts = opts.with_result_sink(path);
    }

    // args: `-c <script> sh` — after argv prompt-delivery appends the prompt,
    // the child sees argv = [sh, -c, <script>, sh, <answer_src>] so $1 is the
    // answer source path.
    runner
        .run_with_prompt_delivery_for_test(
            opts,
            PromptDelivery::Argv,
            DeliveryCaps::argv_only(),
            "sh",
            ["-c", FAKE_AGENT_SCRIPT, "sh"],
        )
        .await
}

#[tokio::test]
async fn result_recovered_verbatim_despite_ansi_log_and_banner_noise() {
    let dir = tempfile::tempdir().unwrap();
    let answer = agent_answer();
    let answer_src = dir.path().join("answer.txt");
    std::fs::write(&answer_src, answer.as_bytes()).unwrap();
    let sink = dir.path().join("result.sink");

    let result = run_fake_agent(&answer_src, Some(sink)).await;

    // The clean channel carries the answer EXACTLY — braces, ANSI, quotes,
    // trailing newline — with zero bleed from stdout.
    assert_eq!(
        result.result.as_deref(),
        Some(answer.as_str()),
        "the sink answer must be recovered byte-for-byte"
    );

    // stdout is still the full noisy firehose (capture unaffected)...
    assert!(
        result.output.contains("STARTUP BANNER"),
        "stdout capture must still contain the banner"
    );
    assert!(
        result.output.contains('\u{1b}'),
        "stdout capture must still contain raw ANSI escapes"
    );
    // ...including the DECOY verdict a brace-scanner would wrongly latch onto.
    assert!(
        result.output.contains("REJECT"),
        "stdout carries a decoy verdict; the clean channel must not"
    );
    assert!(
        !result.result.as_deref().unwrap().contains("REJECT"),
        "the clean answer must be free of stdout's decoy verdict"
    );
    assert_ne!(
        result.result.as_deref(),
        Some(result.output.as_str()),
        "result must be the clean channel, not a copy of noisy stdout"
    );
}

#[tokio::test]
async fn migration_example_reads_answer_with_no_stdout_scraping() {
    let dir = tempfile::tempdir().unwrap();
    let answer = agent_answer();
    let answer_src = dir.path().join("answer.txt");
    std::fs::write(&answer_src, answer.as_bytes()).unwrap();
    let sink = dir.path().join("result.sink");

    let result = run_fake_agent(&answer_src, Some(sink)).await;

    // The consumer recovers the verbatim verdict using ONLY the clean channel.
    let recovered = consume_answer_without_scraping(&result);
    assert_eq!(
        recovered, answer,
        "consumer must recover the answer verbatim via result.result alone"
    );
    assert!(
        recovered.contains("MERGE") && !recovered.contains("REJECT"),
        "consumer got the real MERGE verdict, not the stdout decoy"
    );
}

#[tokio::test]
async fn no_opt_in_yields_none_result_and_unchanged_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let answer = agent_answer();
    let answer_src = dir.path().join("answer.txt");
    std::fs::write(&answer_src, answer.as_bytes()).unwrap();

    // No sink => legacy behaviour: the agent writes nothing to the channel.
    let result = run_fake_agent(&answer_src, None).await;

    assert!(
        result.result.is_none(),
        "without opt-in, result must be None (zero regression)"
    );
    assert!(
        result.output.contains("STARTUP BANNER"),
        "stdout capture is unchanged when the channel is off"
    );

    // The safe consumer idiom degrades to stdout exactly as today.
    let recovered = consume_answer_without_scraping(&result);
    assert_eq!(
        recovered, result.output,
        "with no clean channel the consumer falls back to stdout verbatim"
    );
}

#[test]
fn inject_sink_env_exports_the_variable_to_the_child() {
    // The runner uses inject_sink_env on the std::process::Command before spawn.
    let dir = tempfile::tempdir().unwrap();
    let sink = dir.path().join("injected.sink");

    let mut cmd = StdCommand::new("sh");
    cmd.arg("-c")
        .arg("printf '%s' \"$AMPLIHACK_RESULT_SINK\"")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    result_sink::inject_sink_env(&mut cmd, &sink);

    let out = cmd.output().expect("child should run");
    assert!(out.status.success());
    let seen = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        seen,
        sink.to_string_lossy(),
        "inject_sink_env must export AMPLIHACK_RESULT_SINK=<path> to the child"
    );
}
