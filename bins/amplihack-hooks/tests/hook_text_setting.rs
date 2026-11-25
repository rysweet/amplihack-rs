//! AMPLIHACK_HOOK_TEXT at the multicall binary's process boundary.
//!
//! Every run starts the real `amplihack-hooks` in a fresh [`HookSandbox`], so
//! no run sees the state another wrote, and the sandbox's cleared environment
//! means the test process's own `AMPLIHACK_HOOK_TEXT` (a local model running
//! `cargo test` inside a local session has `none` exported) never reaches a
//! hook.
//!
//! Parity is checked against the same hook run without the variable in the
//! same test, never against stored text. Two things differ between two runs
//! of the same hook and are normalised, and only these:
//!
//! - the sandbox's own path, which `pre-tool-use` names in its denial and
//!   other hooks may name in their context, becomes `<SANDBOX>`;
//! - the number after `"timestamp":`, which `pre-compact` writes in seconds,
//!   becomes `0`.
//!
//! No test pins the wording of any text: model text is detected by its JSON
//! field, and the unknown-value record by the variable's name.
#![cfg(unix)]

#[path = "../../../tests/support/isolated_hook.rs"]
mod isolated_hook;

use isolated_hook::HookSandbox;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Stdio;

const BIN: &str = env!("CARGO_BIN_EXE_amplihack-hooks");
use amplihack_types::AMPLIHACK_HOOK_TEXT as VAR;
const BOGUS: &str = "bogus-value";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Protocol {
    Default,
    Codex,
}

#[derive(Clone, Copy, Debug)]
enum Payload {
    PreSafe,
    PreDestructive,
    Post,
    Stop,
    SessionStart,
    SessionEnd,
    Prompt,
    PreCompact,
    Drain,
    Malformed,
}

#[derive(Clone, Copy)]
struct Case {
    sub: &'static str,
    payload: Payload,
    prepare: fn(&HookSandbox),
}

#[derive(Clone, Copy, Debug)]
enum Stderr {
    Piped,
    /// fd 2 closed before exec (`2>&-`).
    Closed,
    /// Every write to stderr fails with ENOSPC.
    #[cfg(target_os = "linux")]
    Full,
}

/// What a hook gave its host: exit code and normalised stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    code: Option<i32>,
    stdout: String,
}

struct Run {
    outcome: Outcome,
    stderr: String,
    /// Regular files in the sandbox after the run, relative to its root.
    files: BTreeSet<String>,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.outcome.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout is exactly one JSON value ({e}): {:?}",
                self.outcome.stdout
            )
        })
    }

    fn lines_naming_var(&self) -> usize {
        self.stderr.lines().filter(|l| l.contains(VAR)).count()
    }
}

fn nothing(_: &HookSandbox) {}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A preferences file `user-prompt-submit` turns into context.
fn preferences(s: &HookSandbox) {
    write(
        s.project().join("USER_PREFERENCES.md"),
        "| Setting | Value |\n|---|---|\n| Verbosity | balanced |\n",
    );
}

/// Lock mode: the stop hook blocks the end of the turn (as in
/// `issue_2053_stop_state_root.rs`).
fn lock(s: &HookSandbox) {
    for control in [
        ".lock_active",
        ".lock_goal",
        ".lock_message",
        ".continuation_prompt",
    ] {
        write(
            s.project().join(".claude/runtime/locks").join(control),
            "Continue the isolated task",
        );
    }
}

fn payload(payload: Payload, protocol: Protocol) -> String {
    let codex = protocol == Protocol::Codex;
    match payload {
        Payload::PreSafe | Payload::PreDestructive => {
            let command = if matches!(payload, Payload::PreSafe) {
                "echo hello"
            } else {
                "rm -rf ."
            };
            if codex {
                json!({"hook_event_name":"PreToolUse","tool_name":"shell_command","tool_use_id":"tool-1",
                       "tool_input":{"command":command},"transcript_path":null,"session_id":"s1"})
            } else {
                json!({"hook_event_name":"PreToolUse","tool_name":"Bash",
                       "tool_input":{"command":command},"session_id":"s1"})
            }
        }
        Payload::Post => json!({"hook_event_name":"PostToolUse","tool_name":"Bash",
            "tool_input":{"command":"echo hello"},"tool_response":{"output":"hello\n"},"session_id":"s1"}),
        Payload::Stop => {
            json!({"hook_event_name":"Stop","session_id":"s1","stop_hook_active":false,"transcript_path":null})
        }
        Payload::SessionStart => {
            json!({"hook_event_name":"SessionStart","session_id":"s1","source":"startup"})
        }
        Payload::SessionEnd => {
            json!({"hook_event_name":"SessionEnd","session_id":"s1","reason":"exit","transcript_path":null})
        }
        Payload::Prompt => json!({"hook_event_name":"UserPromptSubmit","session_id":"s1",
            "prompt":"Fix the failing login test"}),
        Payload::PreCompact => {
            json!({"hook_event_name":"PreCompact","session_id":"s1","trigger":"manual"})
        }
        Payload::Drain => json!({}),
        Payload::Malformed => return "this is not json {".to_string(),
    }
    .to_string()
}

fn run(case: Case, protocol: Protocol, setting: Option<&OsStr>) -> Run {
    run_with(case, protocol, setting, Stderr::Piped)
}

fn run_with(case: Case, protocol: Protocol, setting: Option<&OsStr>, stderr: Stderr) -> Run {
    let sandbox = HookSandbox::new();
    (case.prepare)(&sandbox);
    let mut command = match stderr {
        Stderr::Closed => {
            let mut c = sandbox.command("/bin/sh");
            c.args(["-c", "exec \"$0\" \"$@\" 2>&-", BIN, case.sub]);
            c
        }
        _ => {
            let mut c = sandbox.command(BIN);
            c.arg(case.sub);
            c
        }
    };
    command
        // No background code-graph indexing from session-start.
        .env("AMPLIHACK_DISABLE_BLARIFY", "1")
        .env("CODEX_HOME", sandbox.home().join(".codex"))
        .env(
            "AMPLIHACK_AGENT_BINARY",
            if protocol == Protocol::Codex {
                "codex"
            } else {
                "claude"
            },
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if protocol == Protocol::Codex {
        command.env("AMPLIHACK_HOOK_PROTOCOL", "codex");
    }
    if let Some(value) = setting {
        command.env(VAR, value);
    }
    match stderr {
        Stderr::Piped => {
            command.stderr(Stdio::piped());
        }
        Stderr::Closed => {}
        #[cfg(target_os = "linux")]
        Stderr::Full => {
            command.stderr(Stdio::from(
                fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            ));
        }
    }
    let mut child = command.spawn().expect("start amplihack-hooks");
    {
        let mut stdin = child.stdin.take().unwrap();
        // A hook may exit before it reads everything; a broken pipe is fine.
        let _ = stdin.write_all(payload(case.payload, protocol).as_bytes());
    }
    let output = child.wait_with_output().unwrap();
    Run {
        outcome: Outcome {
            code: output.status.code(),
            stdout: normalise(&output.stdout, sandbox.root()),
        },
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        files: files_under(sandbox.root()),
    }
}

/// The two normalisations described in the module documentation.
fn normalise(stdout: &[u8], root: &Path) -> String {
    let mut text = String::from_utf8_lossy(stdout).into_owned();
    let mut roots = vec![root.to_string_lossy().into_owned()];
    if let Ok(canonical) = root.canonicalize() {
        roots.push(canonical.to_string_lossy().into_owned());
    }
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    for r in roots {
        text = text.replace(&r, "<SANDBOX>");
    }
    zero_numbers_after(&text, "\"timestamp\":")
}

fn zero_numbers_after(text: &str, key: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(key) {
        let (head, tail) = rest.split_at(at + key.len());
        out.push_str(head);
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push('0');
        }
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
}

fn files_under(root: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(&path, root, out);
            } else if kind.is_file() {
                out.insert(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

fn os(value: &str) -> Option<&OsStr> {
    Some(OsStr::new(value))
}

const fn case(sub: &'static str, payload: Payload, prepare: fn(&HookSandbox)) -> Case {
    Case {
        sub,
        payload,
        prepare,
    }
}

/// Every subcommand and alias that goes through `run_hook`, with a payload
/// that reaches its main path. `limit-resume` is in `hook_text_limit_resume.rs`.
const ALL_DEFAULT: &[Case] = &[
    case("pre-tool-use", Payload::PreSafe, nothing),
    case("pre-tool-use", Payload::PreDestructive, nothing),
    case("post-tool-use", Payload::Post, nothing),
    case("stop", Payload::Stop, nothing),
    case("stop", Payload::Stop, lock),
    case("agentStop", Payload::Stop, lock),
    case("session-start", Payload::SessionStart, preferences),
    case("session-end", Payload::SessionEnd, nothing),
    case("session-stop", Payload::SessionEnd, nothing),
    case("session-stop-event", Payload::SessionEnd, nothing),
    case("sessionEnd", Payload::SessionEnd, nothing),
    case("workflow-classification-reminder", Payload::Prompt, nothing),
    case("user-prompt", Payload::Prompt, preferences),
    case("user-prompt-submit", Payload::Prompt, preferences),
    case("pre-compact", Payload::PreCompact, nothing),
    case("precommit-prefs", Payload::Drain, nothing),
];

/// The four governed hooks (and aliases) and pre-tool-use, under Codex.
const CODEX: &[Case] = &[
    case("session-start", Payload::SessionStart, preferences),
    case("user-prompt-submit", Payload::Prompt, preferences),
    case("user-prompt", Payload::Prompt, preferences),
    case("workflow-classification-reminder", Payload::Prompt, nothing),
    case("stop", Payload::Stop, lock),
    case("agentStop", Payload::Stop, lock),
    case("pre-tool-use", Payload::PreSafe, nothing),
    case("pre-tool-use", Payload::PreDestructive, nothing),
];

const CONTEXT_HOOKS: &[Case] = &[
    case("session-start", Payload::SessionStart, preferences),
    case("user-prompt-submit", Payload::Prompt, preferences),
    case("user-prompt", Payload::Prompt, preferences),
    case("workflow-classification-reminder", Payload::Prompt, nothing),
];

fn governed(sub: &str) -> bool {
    matches!(
        sub,
        "session-start"
            | "user-prompt"
            | "user-prompt-submit"
            | "workflow-classification-reminder"
            | "stop"
            | "agentStop"
    )
}

fn model_text_fields(output: &Value) -> Vec<&'static str> {
    let mut found = Vec::new();
    if output.get("additionalContext").is_some() {
        found.push("additionalContext");
    }
    if output
        .pointer("/hookSpecificOutput/additionalContext")
        .is_some()
    {
        found.push("hookSpecificOutput.additionalContext");
    }
    if output.get("systemMessage").is_some() {
        found.push("systemMessage");
    }
    found
}

fn has_context(output: &Value) -> bool {
    [
        output.get("additionalContext"),
        output.pointer("/hookSpecificOutput/additionalContext"),
    ]
    .into_iter()
    .flatten()
    .any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
}

fn is_deny(output: &Value) -> bool {
    output.get("block") == Some(&json!(true))
        || output.get("decision") == Some(&json!("block"))
        || output.pointer("/hookSpecificOutput/permissionDecision") == Some(&json!("deny"))
}

// Explicit unsupported values preserve existing output and report a diagnostic.

fn assert_unsupported_matches_unset(cases: &[Case], protocol: Protocol) {
    for &c in cases {
        let unset = run(c, protocol, None);
        assert_eq!(unset.lines_naming_var(), 0);
        for value in ["full", "FULL", " Full\n", "", " ", "NONE", " none "] {
            let full = run(c, protocol, os(value));
            assert_eq!(
                full.outcome, unset.outcome,
                "{} {:?} {protocol:?} with {VAR}={value:?} differs from unset",
                c.sub, c.payload
            );
            assert_eq!(
                full.lines_naming_var(),
                usize::from(governed(c.sub)),
                "{} {value:?}",
                c.sub
            );
        }
    }
}

#[test]
fn unsupported_values_preserve_output_for_every_hook() {
    assert_unsupported_matches_unset(ALL_DEFAULT, Protocol::Default);
}

#[test]
fn unsupported_values_preserve_output_under_the_codex_protocol() {
    assert_unsupported_matches_unset(CODEX, Protocol::Codex);
}

// `none` removes the context hooks' model text.

#[test]
fn none_removes_the_model_text_of_the_context_hooks() {
    for protocol in [Protocol::Default, Protocol::Codex] {
        for &c in CONTEXT_HOOKS {
            let unset = run(c, protocol, None);
            assert_eq!(unset.outcome.code, Some(0), "{} {protocol:?}", c.sub);
            assert!(
                has_context(&unset.json()),
                "fixture check: {} {protocol:?} gives context when the variable is unset: {}",
                c.sub,
                unset.outcome.stdout
            );

            let none = run(c, protocol, os("none"));
            assert_eq!(none.outcome.code, Some(0), "{} {protocol:?}", c.sub);
            let output = none.json();
            assert!(output.is_object(), "{} {protocol:?}: {output}", c.sub);
            assert_eq!(
                model_text_fields(&output),
                Vec::<&str>::new(),
                "{} {protocol:?} under none: {output}",
                c.sub
            );
            if c.sub != "session-start" || protocol == Protocol::Codex {
                assert_eq!(output, json!({}), "{} {protocol:?}", c.sub);
            }
            assert_eq!(none.lines_naming_var(), 0, "{} {protocol:?}", c.sub);
        }
    }
}

/// Under the default protocol session-start still reports its indexing
/// status and warnings, which are diagnostics for the operator.
#[test]
fn none_keeps_session_start_diagnostics() {
    let c = case("session-start", Payload::SessionStart, preferences);
    let unset = run(c, Protocol::Default, None).json();
    let none = run(c, Protocol::Default, os("none")).json();
    let mut expected = json!({"hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "indexing_status": unset["hookSpecificOutput"]["indexing_status"].clone(),
    }});
    assert!(
        expected["hookSpecificOutput"]["indexing_status"].is_string(),
        "fixture check: {unset}"
    );
    if let Some(warnings) = unset.get("warnings") {
        expected["warnings"] = warnings.clone();
    }
    assert_eq!(none, expected);
}

/// The hooks still run under `none`: what they write is what they write
/// without it. (workflow-classification records its outcome, for one.)
#[test]
fn none_still_does_the_context_hooks_bookkeeping() {
    for &c in CONTEXT_HOOKS {
        let unset = run(c, Protocol::Default, None);
        let none = run(c, Protocol::Default, os("none"));
        assert_eq!(none.files, unset.files, "{}", c.sub);
    }
    let classification = run(
        case("workflow-classification-reminder", Payload::Prompt, nothing),
        Protocol::Default,
        os("none"),
    );
    assert!(
        classification
            .files
            .iter()
            .any(|f| f.ends_with("classification_state.json")),
        "the classification outcome is recorded under none: {:?}",
        classification.files
    );
}

#[test]
fn none_requires_exact_matching() {
    let c = case("user-prompt-submit", Payload::Prompt, preferences);
    let unset = run(c, Protocol::Default, None).outcome;
    for value in ["NONE", " None ", "\tnOnE\n"] {
        assert_eq!(
            run(c, Protocol::Default, os(value)).outcome,
            unset,
            "{value:?}"
        );
    }
}

// `none` approves stop without running it.

#[test]
fn none_approves_stop_without_running_it() {
    for sub in ["stop", "agentStop"] {
        let c = case(sub, Payload::Stop, lock);
        let unset = run(c, Protocol::Default, None);
        assert_eq!(
            unset.json()["decision"],
            "block",
            "fixture check: lock mode blocks {sub} when the variable is unset"
        );
        assert!(
            unset
                .files
                .iter()
                .any(|f| f.ends_with("lock_invocations.txt")),
            "fixture check: lock mode records its iterations: {:?}",
            unset.files
        );

        let none = run(c, Protocol::Default, os("none"));
        assert_eq!(none.outcome.code, Some(0), "{sub}");
        assert_eq!(none.json(), json!({"decision": "approve"}), "{sub}");
        assert!(
            !none
                .files
                .iter()
                .any(|f| f.ends_with("lock_invocations.txt")),
            "{sub} must not run lock mode under none: {:?}",
            none.files
        );

        let codex = run(c, Protocol::Codex, os("none"));
        assert_eq!(codex.outcome.code, Some(0), "{sub}");
        assert_eq!(codex.json(), json!({}), "{sub}");
    }
}

// `none` leaves the command check alone.

#[test]
fn none_still_blocks_a_destructive_command() {
    let c = case("pre-tool-use", Payload::PreDestructive, nothing);
    for protocol in [Protocol::Default, Protocol::Codex] {
        let unset = run(c, protocol, None);
        assert!(
            is_deny(&unset.json()),
            "fixture check: {protocol:?} denies the command when unset: {}",
            unset.outcome.stdout
        );
        for value in ["none", BOGUS] {
            let other = run(c, protocol, os(value));
            assert_eq!(other.outcome, unset.outcome, "{protocol:?} {value}");
            assert!(is_deny(&other.json()), "{protocol:?} {value}");
            assert_eq!(
                other.lines_naming_var(),
                0,
                "pre-tool-use never reads {VAR}"
            );
        }
    }
}

/// H6: the ungoverned hooks behave the same under `none`.
#[test]
fn ungoverned_hooks_ignore_none() {
    for &c in ALL_DEFAULT.iter().filter(|c| !governed(c.sub)) {
        let unset = run(c, Protocol::Default, None);
        let none = run(c, Protocol::Default, os("none"));
        assert_eq!(none.outcome, unset.outcome, "{} {:?}", c.sub, c.payload);
        assert_eq!(none.lines_naming_var(), 0, "{}", c.sub);
    }
}

// An unknown value preserves default behaviour.

fn assert_unknown_preserves_default(cases: &[Case], protocol: Protocol, value: &OsStr) {
    for &c in cases {
        let unset = run(c, protocol, None);
        let bogus = run(c, protocol, Some(value));
        assert_eq!(
            bogus.outcome, unset.outcome,
            "{} {:?} {protocol:?} with {value:?}",
            c.sub, c.payload
        );
        let shown = value.to_string_lossy();
        assert!(
            !bogus.outcome.stdout.contains(VAR) && !bogus.outcome.stdout.contains(shown.as_ref()),
            "the record never reaches stdout: {}",
            bogus.outcome.stdout
        );
        let expected = usize::from(governed(c.sub));
        assert_eq!(
            bogus.lines_naming_var(),
            expected,
            "{} {protocol:?}: one stderr line names {VAR} for a governed hook, none otherwise: {}",
            c.sub,
            bogus.stderr
        );
    }
}

#[test]
fn an_unknown_value_preserves_default_and_is_recorded_on_stderr_only() {
    assert_unknown_preserves_default(ALL_DEFAULT, Protocol::Default, OsStr::new(BOGUS));
    assert_unknown_preserves_default(CODEX, Protocol::Codex, OsStr::new(BOGUS));
}

#[test]
fn a_value_that_is_not_utf8_preserves_default() {
    let value = OsStr::from_bytes(b"\xffnone");
    assert_unknown_preserves_default(CONTEXT_HOOKS, Protocol::Default, value);
    assert_unknown_preserves_default(
        &[case("stop", Payload::Stop, lock)],
        Protocol::Default,
        value,
    );
}

/// The record is written before stdin is read, so a malformed payload still
/// gets it, under either protocol.
#[test]
fn the_record_is_written_whatever_the_input() {
    for protocol in [Protocol::Default, Protocol::Codex] {
        for sub in [
            "session-start",
            "user-prompt-submit",
            "workflow-classification-reminder",
            "stop",
        ] {
            let bogus = run(case(sub, Payload::Malformed, nothing), protocol, os(BOGUS));
            assert_eq!(
                bogus.lines_naming_var(),
                1,
                "{sub} {protocol:?}: {}",
                bogus.stderr
            );
        }
    }
}

// ---- H7: failures are unchanged under every value ----

#[test]
fn failures_print_the_same_output_under_every_value() {
    for protocol in [Protocol::Default, Protocol::Codex] {
        for sub in [
            "session-start",
            "user-prompt-submit",
            "workflow-classification-reminder",
            "stop",
            "agentStop",
        ] {
            let c = case(sub, Payload::Malformed, nothing);
            let unset = run(c, protocol, None);
            assert_eq!(unset.outcome.code, Some(0), "fail-open: {sub} {protocol:?}");
            for value in ["full", "none", BOGUS] {
                assert_eq!(
                    run(c, protocol, os(value)).outcome,
                    unset.outcome,
                    "{sub} {protocol:?} {value}"
                );
            }
        }
    }
}

// ---- SEC-H5: the record cannot change the exit status ----

#[test]
fn a_closed_stderr_does_not_fail_the_hook() {
    for &c in CONTEXT_HOOKS {
        let unset = run(c, Protocol::Default, None);
        let closed = run_with(c, Protocol::Default, os(BOGUS), Stderr::Closed);
        assert_eq!(closed.outcome, unset.outcome, "{}", c.sub);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_failing_stderr_does_not_fail_the_hook() {
    let cases = [
        case("session-start", Payload::SessionStart, preferences),
        case("user-prompt-submit", Payload::Prompt, preferences),
        case("workflow-classification-reminder", Payload::Prompt, nothing),
        case("stop", Payload::Stop, nothing),
    ];
    for c in cases {
        let unset = run(c, Protocol::Default, None);
        let full = run_with(c, Protocol::Default, os(BOGUS), Stderr::Full);
        assert_eq!(full.outcome, unset.outcome, "{}", c.sub);
        assert_eq!(full.outcome.code, Some(0), "{}", c.sub);
    }
}

/// The one place that can tell `AMPLIHACK_HOOK_TEXT` came from the sandbox
/// and not from the test process: the sandbox's environment is cleared.
#[test]
fn the_sandbox_does_not_pass_the_test_process_value_through() {
    let sandbox = HookSandbox::new();
    let script = format!("printf '%s' \"${{{VAR}-unset}}\"");
    let out = sandbox
        .command("/bin/sh")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "unset");
}
