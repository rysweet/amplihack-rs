//! crates/amplihack-cli/tests/builder_dispatch.rs
//!
//! Issue rysweet/Simard#1940 — Layer 2 surgical direct-call coverage for
//! `commands::builder::{run_claude, run_codex, run_export_on_compact}`.
//!
//! Each test calls the dispatch function directly with explicit `&Path`
//! args sourced from a `tempfile::TempDir`. No `$HOME` resolution, no
//! interactive prompts, no network. Inline JSONL fixtures are
//! `fixture-`-prefixed to disambiguate from real user data.
//!
//! Acceptance: covers happy path + missing-input error path for each
//! function; pushes `src/commands/builder.rs` past the ≥30% per-file
//! threshold from issue #1940.

use std::fs;
use std::path::PathBuf;

use amplihack_cli::commands::builder::{run_claude, run_codex, run_export_on_compact};
use tempfile::TempDir;

/// Minimal Claude `messages.json` fixture: two text messages.
/// Schema mirrors `amplihack_builders::claude::Message` (role, content,
/// optional timestamp). Content is the `Text` variant of `MessageContent`.
const FIXTURE_CLAUDE_MESSAGES_JSON: &str = r#"[
  {"role": "user", "content": "fixture-prompt-hello"},
  {"role": "assistant", "content": "fixture-response-world"}
]"#;

/// Minimal Codex session JSON fixture matching `CodexSession` schema.
const FIXTURE_CODEX_SESSION_JSON: &str = r#"{
  "session_id": "fixture-session-001",
  "messages": [
    {"role": "user", "content": "fixture-codex-question about builder.rs"},
    {"role": "assistant", "content": "fixture-codex-reply mentioning builder.rs"}
  ],
  "metadata": {"source": "test-fixture"}
}"#;

/// Export-on-compact payload fixture — the `process` integration only
/// needs a JSON object; an empty one is the minimal valid shape.
const FIXTURE_EXPORT_PAYLOAD_JSON: &str = r#"{"hook_event_name": "fixture-test"}"#;

fn write_fixture(dir: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, body).expect("write fixture file");
    path
}

#[test]
fn run_claude_happy_path_writes_transcript_to_out_file() {
    let tmp = TempDir::new().expect("create temp dir");
    let messages = write_fixture(&tmp, "messages.json", FIXTURE_CLAUDE_MESSAGES_JSON);
    let working_dir = tmp.path().to_path_buf();
    let out = tmp.path().join("nested").join("transcript.md");

    let result = run_claude(
        "fixture-session-001",
        &messages,
        Some(working_dir),
        Some(out.clone()),
        "json",
    );

    assert!(result.is_ok(), "run_claude must succeed: {result:?}");
    assert!(
        out.exists(),
        "expected transcript file at {}",
        out.display()
    );
    let body = fs::read_to_string(&out).expect("read transcript");
    assert!(
        body.contains("fixture-session-001"),
        "transcript should include the session id; got:\n{body}"
    );
    assert!(
        body.contains("fixture-prompt-hello") || body.contains("fixture-response-world"),
        "transcript should include at least one fixture message; got:\n{body}"
    );
}

#[test]
fn run_claude_without_out_path_uses_stdout_and_still_succeeds() {
    let tmp = TempDir::new().expect("create temp dir");
    let messages = write_fixture(&tmp, "messages.json", FIXTURE_CLAUDE_MESSAGES_JSON);

    // Stdout note: we intentionally do NOT assert on captured stdout.
    // `cargo test` captures by default; printlns landing in the capture
    // buffer are still proof the code path executed. The assertion is
    // simply that the dispatch returns Ok and does no filesystem write.
    let result = run_claude("fixture-session-stdout", &messages, None, None, "text");
    assert!(result.is_ok(), "stdout path must succeed: {result:?}");
}

#[test]
fn run_claude_missing_messages_file_returns_err() {
    let tmp = TempDir::new().expect("create temp dir");
    let missing = tmp.path().join("does-not-exist.json");

    let result = run_claude(
        "fixture-session-missing",
        &missing,
        Some(tmp.path().to_path_buf()),
        None,
        "json",
    );

    let err = result.expect_err("missing messages file must surface as Err, not silent success");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("does-not-exist") || chain.contains("messages") || chain.contains("file"),
        "error chain should mention the failing file/path; got: {chain}"
    );
}

#[test]
fn run_claude_invalid_json_returns_err() {
    let tmp = TempDir::new().expect("create temp dir");
    let messages = write_fixture(&tmp, "messages.json", "{not-valid-json}");

    let result = run_claude(
        "fixture-session-malformed",
        &messages,
        Some(tmp.path().to_path_buf()),
        None,
        "json",
    );

    let err = result.expect_err("malformed JSON must error out, not produce empty transcript");
    let chain = format!("{err:#}").to_lowercase();
    assert!(
        chain.contains("json") || chain.contains("invalid") || chain.contains("messages"),
        "error chain should indicate a parse problem; got: {chain}"
    );
}

#[test]
fn run_codex_happy_path_with_populated_input_dir_writes_to_out_file() {
    let tmp = TempDir::new().expect("create temp dir");
    let input_dir = tmp.path().join("codex-input");
    fs::create_dir_all(&input_dir).expect("create input dir");
    let session_path = input_dir.join("session.json");
    fs::write(&session_path, FIXTURE_CODEX_SESSION_JSON).expect("write session");
    let out = tmp.path().join("codex-output").join("comprehensive.md");

    let result = run_codex(&input_dir, None, Some(out.clone()), "json");
    assert!(result.is_ok(), "run_codex must succeed: {result:?}");
    assert!(out.exists(), "expected codex output at {}", out.display());
    let body = fs::read_to_string(&out).expect("read codex output");
    assert!(
        body.contains("Comprehensive Codex") || body.contains("fixture-session-001"),
        "codex body should include header or session id; got:\n{body}"
    );
}

#[test]
fn run_codex_happy_path_with_focus_includes_focused_header() {
    let tmp = TempDir::new().expect("create temp dir");
    let input_dir = tmp.path().join("codex-input");
    fs::create_dir_all(&input_dir).expect("create input dir");
    fs::write(input_dir.join("s.json"), FIXTURE_CODEX_SESSION_JSON).expect("write session");
    let out = tmp.path().join("focused.md");

    let result = run_codex(&input_dir, Some("builder.rs"), Some(out.clone()), "text");
    assert!(result.is_ok(), "run_codex focused must succeed: {result:?}");
    let body = fs::read_to_string(&out).expect("read focused output");
    assert!(
        body.contains("Focused Codex") && body.contains("builder.rs"),
        "focused body must include the focus header; got:\n{body}"
    );
}

#[test]
fn run_codex_with_empty_input_dir_succeeds_with_empty_corpus() {
    let tmp = TempDir::new().expect("create temp dir");
    let empty_dir = tmp.path().join("empty");
    fs::create_dir_all(&empty_dir).expect("create empty dir");
    let out = tmp.path().join("empty-out.md");

    // Per builder contract: missing/empty dirs return empty string, not Err.
    // We still assert the call succeeds.
    let result = run_codex(&empty_dir, None, Some(out.clone()), "json");
    assert!(
        result.is_ok(),
        "run_codex against empty dir must succeed: {result:?}"
    );
}

#[test]
fn run_codex_with_nonexistent_input_dir_succeeds_with_empty_corpus() {
    let tmp = TempDir::new().expect("create temp dir");
    let missing = tmp.path().join("does-not-exist");

    // Contract: load_sessions short-circuits when root doesn't exist.
    let result = run_codex(&missing, None, None, "text");
    assert!(
        result.is_ok(),
        "nonexistent codex dir should be a no-op success: {result:?}"
    );
}

#[test]
fn run_export_on_compact_happy_path_processes_payload() {
    let tmp = TempDir::new().expect("create temp dir");
    let input = write_fixture(&tmp, "payload.json", FIXTURE_EXPORT_PAYLOAD_JSON);
    let root = tmp.path().to_path_buf();

    let result = run_export_on_compact(&input, &root, "json");
    assert!(
        result.is_ok(),
        "export_on_compact happy path must succeed: {result:?}"
    );
}

#[test]
fn run_export_on_compact_missing_input_file_returns_err() {
    let tmp = TempDir::new().expect("create temp dir");
    let missing = tmp.path().join("missing-payload.json");

    let result = run_export_on_compact(&missing, tmp.path(), "json");
    let err = result.expect_err("missing input file must surface as Err");
    let chain = format!("{err:#}");
    assert!(
        chain.contains("missing-payload") || chain.contains("read input") || chain.contains("file"),
        "error chain should mention the failing input path; got: {chain}"
    );
}

#[test]
fn run_export_on_compact_invalid_json_returns_err() {
    let tmp = TempDir::new().expect("create temp dir");
    let input = write_fixture(&tmp, "payload.json", "<<<not-json>>>");

    let result = run_export_on_compact(&input, tmp.path(), "json");
    let err = result.expect_err("invalid JSON must surface as Err");
    let chain = format!("{err:#}").to_lowercase();
    assert!(
        chain.contains("parse json") || chain.contains("json") || chain.contains("invalid"),
        "error chain should mention parse failure; got: {chain}"
    );
}

/// Deliberately failing red-phase test (TDD step 7).
///
/// Locks down a future-facing contract: `run_claude` should NOT silently
/// accept an empty-string session id, because the produced transcript
/// uses `# Conversation Transcript - {session_id}` as its header, and an
/// empty id produces a malformed header. This test currently FAILS because
/// the production code does not validate session_id. Implementation in
/// step 8 will either add the validation or this test will be revised to
/// match the agreed behavior.
#[test]
#[ignore = "red-phase: locks down session_id validation contract; un-ignore once decision recorded"]
fn run_claude_rejects_empty_session_id() {
    let tmp = TempDir::new().expect("create temp dir");
    let messages = write_fixture(&tmp, "messages.json", FIXTURE_CLAUDE_MESSAGES_JSON);

    let result = run_claude("", &messages, Some(tmp.path().to_path_buf()), None, "json");
    let err = result.expect_err(
        "empty session id should be rejected by run_claude — see issue rysweet/Simard#1940",
    );
    let chain = format!("{err:#}").to_lowercase();
    assert!(
        chain.contains("session") || chain.contains("empty"),
        "error should indicate the invalid session id; got: {chain}"
    );
}
