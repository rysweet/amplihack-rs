//! TDD red-phase contract tests for the **clean result channel** module.
//!
//! These tests specify the public surface of
//! `amplihack_orchestration::result_sink` and the two additive struct fields
//! (`RunOptions.result_sink`, `ProcessResult.result`) documented in
//! `docs/reference/clean-result-channel.md`.
//!
//! They fail until the feature is implemented:
//!   * `amplihack_orchestration::result_sink` module does not exist yet,
//!   * `RunOptions::with_result_sink` / `RunOptions.result_sink` do not exist,
//!   * `ProcessResult.result` does not exist.
//!
//! This is the expected RED state — the file defines the contract Step 8 builds.
//!
//! Scope of this file: pure, process-free contract checks that hold on every
//! platform (verbatim reader semantics, path allocation, field defaults).
//! End-to-end "verbatim under stdout noise" proving tests live in
//! `result_sink_channel.rs`; the stale-inheritance (SEC-10) test lives in
//! `result_sink_env_isolation.rs`.

use std::fs;
use std::time::Duration;

use amplihack_orchestration::claude_process::{ProcessResult, RunOptions};
use amplihack_orchestration::result_sink;

// ── Env-var contract ──────────────────────────────────────────────────

#[test]
fn result_sink_env_const_is_amplihack_result_sink() {
    // The single stable env var the runner exports to the child. External
    // consumers (recipe-runner-rs, Simard) key off this exact name.
    assert_eq!(result_sink::RESULT_SINK_ENV, "AMPLIHACK_RESULT_SINK");
}

// ── allocate_sink_path ────────────────────────────────────────────────

#[test]
fn allocate_sink_path_creates_missing_runtime_dir() {
    let base = tempfile::tempdir().unwrap();
    let runtime_dir = base.path().join("does-not-exist-yet");
    assert!(!runtime_dir.exists(), "precondition: runtime dir absent");

    let sink = result_sink::allocate_sink_path(&runtime_dir)
        .expect("allocate_sink_path should create the runtime dir and return a path");

    assert!(
        runtime_dir.exists(),
        "allocate_sink_path must create the runtime dir if needed"
    );
    assert!(
        sink.starts_with(&runtime_dir),
        "allocated sink {sink:?} must live under the runtime dir {runtime_dir:?}"
    );
    assert!(
        sink.parent().is_some_and(|p| p.exists()),
        "the parent directory of the allocated sink must exist"
    );
}

#[test]
fn allocate_sink_path_returns_unique_paths() {
    // Patterns that fan out (debate / n_version / expert_panel) allocate one
    // sink per parallel spawn; reused paths would race concurrent writes.
    let runtime_dir = tempfile::tempdir().unwrap();
    let a = result_sink::allocate_sink_path(runtime_dir.path()).unwrap();
    let b = result_sink::allocate_sink_path(runtime_dir.path()).unwrap();
    assert_ne!(a, b, "each allocation must yield a distinct sink path");
}

#[cfg(unix)]
#[test]
fn allocate_sink_path_creates_runtime_dir_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let base = tempfile::tempdir().unwrap();
    let runtime_dir = base.path().join("runtime-owner-only");
    let _sink = result_sink::allocate_sink_path(&runtime_dir).unwrap();

    let mode = fs::metadata(&runtime_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "SEC-6: a runner-created runtime dir must be owner-only (0700), got {mode:o}"
    );
}

// ── read_sink_verbatim ────────────────────────────────────────────────

#[test]
fn read_sink_verbatim_missing_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never-written.sink");
    assert!(
        result_sink::read_sink_verbatim(&path).is_none(),
        "an unwritten sink must read back as None (consumer falls back to stdout)"
    );
}

#[test]
fn read_sink_verbatim_empty_file_is_none() {
    // Empty and unwritten collapse to the same signal, so a consumer never
    // observes Some(\"\") and need not distinguish "opted out" from "wrote nothing".
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.sink");
    fs::write(&path, b"").unwrap();
    assert!(
        result_sink::read_sink_verbatim(&path).is_none(),
        "a zero-length sink must read back as None, never Some(String::new())"
    );
}

#[test]
fn read_sink_verbatim_recovers_answer_byte_for_byte() {
    // The exact conditions that break extract_json today: the answer itself
    // contains ANSI escapes, braces, JSON, and quotes. Verbatim capture must
    // return them unchanged — no strip, no trim, no parse.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("answer.sink");
    let answer = "\u{1b}[32mVERDICT\u{1b}[0m: { \"verdict\": \"MERGE\", \"note\": \"has } brace and 'quotes'\" }";
    fs::write(&path, answer.as_bytes()).unwrap();

    let got = result_sink::read_sink_verbatim(&path)
        .expect("a written, valid-UTF-8 sink must read back as Some");
    assert_eq!(got, answer, "sink contents must be recovered byte-for-byte");
}

#[test]
fn read_sink_verbatim_preserves_whitespace_and_newlines() {
    // No trimming, no CRLF/LF normalization.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("whitespace.sink");
    let answer = "  leading and trailing  \r\nmiddle\nline\n";
    fs::write(&path, answer.as_bytes()).unwrap();

    let got = result_sink::read_sink_verbatim(&path).unwrap();
    assert_eq!(
        got, answer,
        "leading/trailing whitespace and CRLF/LF must be preserved verbatim"
    );
}

#[test]
fn read_sink_verbatim_non_utf8_is_none() {
    // result is a String; non-UTF-8 bytes yield None rather than a lossy
    // replacement (binary payloads are out of scope for v1).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binary.sink");
    fs::write(&path, [0xff, 0xfe, 0x00, 0x80]).unwrap();
    assert!(
        result_sink::read_sink_verbatim(&path).is_none(),
        "invalid UTF-8 must read back as None"
    );
}

#[test]
fn read_sink_verbatim_accepts_reasonably_large_answer() {
    // An answer well within any sane cap round-trips intact.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.sink");
    let answer = "x".repeat(128 * 1024); // 128 KiB — comfortably below any sane cap
    fs::write(&path, answer.as_bytes()).unwrap();

    let got = result_sink::read_sink_verbatim(&path).unwrap();
    assert_eq!(got.len(), answer.len());
    assert_eq!(got, answer);
}

#[test]
fn read_sink_verbatim_oversize_is_none() {
    // SEC-3: reads are bounded. A sink far larger than any sane cap yields
    // None (consumers fall back to output) rather than an unbounded read.
    // A sparse file (set_len) proves a correct impl checks the size before
    // slurping the whole thing into memory.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversize.sink");
    let f = fs::File::create(&path).unwrap();
    f.set_len(256 * 1024 * 1024).unwrap(); // 256 MiB, sparse
    drop(f);

    assert!(
        result_sink::read_sink_verbatim(&path).is_none(),
        "a sink beyond the bounded read cap must yield None"
    );
}

#[cfg(unix)]
#[test]
fn read_sink_verbatim_refuses_symlinked_sink() {
    // SEC-13: a sink swapped for a symlink must never be followed. Otherwise a
    // symlink pointing at a secret would let the runner read that secret and
    // hand it to a downstream consumer as the "clean" answer. Refuse before
    // opening so the target file's bytes are never read.
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.txt");
    fs::write(
        &secret,
        b"TOP SECRET: the runner must not read this via a symlinked sink",
    )
    .unwrap();

    let sink = dir.path().join("evil.sink");
    symlink(&secret, &sink).unwrap();

    assert!(
        result_sink::read_sink_verbatim(&sink).is_none(),
        "a symlinked sink must yield None; its target must never be read"
    );
}

// ── Additive struct fields default off ────────────────────────────────

#[test]
fn run_options_new_has_no_result_sink() {
    let opts = RunOptions::new("prompt".into(), "id".into());
    assert!(
        opts.result_sink.is_none(),
        "RunOptions::new must default result_sink to None so existing callers are unchanged"
    );
}

#[test]
fn run_options_with_result_sink_sets_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sink");
    let opts = RunOptions::new("prompt".into(), "id".into()).with_result_sink(path.clone());
    assert_eq!(
        opts.result_sink.as_deref(),
        Some(path.as_path()),
        "with_result_sink must enable the clean channel for this run"
    );
    // Other fields keep their prior defaults.
    assert!(opts.timeout.is_none());
    assert!(opts.model.is_none());
    assert!(opts.working_dir.is_none());
}

#[test]
fn process_result_ok_defaults_result_to_none() {
    let r = ProcessResult::ok("stdout".into(), "pid".into(), Duration::ZERO);
    assert!(
        r.result.is_none(),
        "ProcessResult::ok must default result to None (opt-in feature)"
    );
    assert_eq!(r.output, "stdout", "existing fields keep their semantics");
}

#[test]
fn process_result_err_defaults_result_to_none() {
    let r = ProcessResult::err("boom".into(), "pid".into(), Duration::ZERO);
    assert!(
        r.result.is_none(),
        "ProcessResult::err must default result to None"
    );
}
