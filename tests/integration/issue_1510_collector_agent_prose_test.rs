//! Issue #1510: the loop-evidence collector must not treat agent prose as a
//! terminal policy refusal.
//!
//! On PR #1586 a Crusty review quoted the token `BLOCKED_TERMINAL` (and the
//! phrase "exit 79") while explaining how the loop stops. Agent output lines
//! carry the `[HH:MM:SS] [amplihack:...]` prefix. `step-01-collect-loop-evidence`
//! matched the bare token and the exit-79 regex over that prose, so the loop
//! stopped as if the guard had refused.
//!
//! The fix applies the rule `terminal_refusal()` in
//! `amplifier-bundle/tools/autodrive_loop.sh` already uses: drop agent output
//! lines before BOTH text detectors. Only those two detectors read the filtered
//! text; the duration / still-running / failure-class checks keep the full blob.
//!
//! These tests run the real step-01 command from the YAML through `bash`, in a
//! cleared environment, and parse the single JSON line it prints. They need
//! `bash` and `git`, not the `amplihack` binary.

use serde_json::Value as Json;
use serde_yaml::Value as Yaml;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Prefixed agent prose quoting the bare token (the PR #1586 shape).
const AGENT_QUOTES_TOKEN: &str = "[10:13:17] [amplihack:claude:3] the loop only stops for good on the policy-refusal exit code or BLOCKED_TERMINAL (see terminal_refusal in autodrive_loop.sh)";
/// Prefixed agent prose matching the exit-79 regex.
const AGENT_QUOTES_EXIT_79: &str =
    "[10:13:18] [amplihack:claude:3] the guard returns exit 79 on refusal";
/// The real guard line, as it reaches the log unprefixed.
const GUARD_LINE: &str = "BLOCKED_TERMINAL orchestration_unavailable: nested orchestration refused";

const REASON_BLOCKED: &str = "BLOCKED_TERMINAL reported by a child";
const REASON_EXIT_TEXT: &str = "exit code 79 present in round output";
const REASON_EXIT_CODE: &str = "child process exited 79";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

/// The exact `command` of step-01 — no templating, no substitution.
fn step01_command() -> String {
    let path = workspace_root().join("amplifier-bundle/recipes/loop-evidence-collector.yaml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let yaml: Yaml =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    yaml["steps"]
        .as_sequence()
        .expect("collector must have a steps sequence")
        .iter()
        .find(|step| step["id"].as_str() == Some("step-01-collect-loop-evidence"))
        .and_then(|step| step["command"].as_str())
        .expect("step-01-collect-loop-evidence must have a command")
        .to_string()
}

/// Per-test scratch dir under `std::env::temp_dir()` (i.e. `$TMPDIR`).
/// `create_dir`, not `create_dir_all`, so a pre-planted path fails loudly.
/// Removed on drop, including when an assertion panics.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("i1510-{}-{n}", std::process::id()));
        std::fs::create_dir(&path)
            .unwrap_or_else(|e| panic!("create scratch dir {}: {e}", path.display()));
        Self(path)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Cleared environment: only PATH and TMPDIR come from the parent, so tokens
/// such as GH_TOKEN never reach the child or a failure message.
fn isolated(cmd: &mut Command, scratch: &Path) {
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TMPDIR", std::env::temp_dir())
        .env("HOME", scratch)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
}

/// Run step-01 with `vars` and return its JSON evidence object.
fn run_collector(vars: &[(&str, &str)]) -> Json {
    let scratch = ScratchDir::new();

    let mut git = amplihack_git::command_in(&scratch.0);
    isolated(&mut git, &scratch.0);
    let status = git.args(["init", "-q"]).status().expect("spawn git init");
    assert!(
        status.success(),
        "git init failed in {}",
        scratch.0.display()
    );

    // step-01 runs git itself, so the repository variables a hook would set
    // are removed here as well (isolated() also clears the environment).
    let mut bash = Command::new("bash");
    amplihack_git::scrub(&mut bash);
    isolated(&mut bash, &scratch.0);
    bash.arg("-c")
        .arg(step01_command())
        .current_dir(&scratch.0)
        .env("LOOP_REPO_PATH", &scratch.0);
    for (key, value) in vars {
        bash.env(key, value);
    }
    let out = bash.output().expect("spawn bash");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "step-01 exited {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON line on stdout:\n{stdout}\nstderr:\n{stderr}"));
    serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("invalid JSON from step-01 ({e}): {line}\nstderr:\n{stderr}"))
}

fn terminal(evidence: &Json) -> &str {
    evidence["terminal_refusal"]
        .as_str()
        .unwrap_or_else(|| panic!("terminal_refusal must be a string: {evidence}"))
}

fn reason(evidence: &Json) -> &str {
    evidence["terminal_reason"]
        .as_str()
        .unwrap_or_else(|| panic!("terminal_reason must be a string: {evidence}"))
}

fn assert_not_terminal(evidence: &Json, case: &str) {
    assert_eq!(
        terminal(evidence),
        "false",
        "{case}: agent prose must not be a terminal refusal; evidence: {evidence}"
    );
}

fn assert_terminal_with(evidence: &Json, needle: &str, case: &str) {
    assert_eq!(
        terminal(evidence),
        "true",
        "{case}: must be a terminal refusal; evidence: {evidence}"
    );
    assert!(
        reason(evidence).contains(needle),
        "{case}: terminal_reason must contain {needle:?}; got {:?}",
        reason(evidence)
    );
}

// --- Agent prose is not terminal (fail before the fix) ---------------------

#[test]
fn issue_1510_agent_prose_quoting_blocked_terminal_is_not_terminal() {
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", AGENT_QUOTES_TOKEN)]);
    assert_not_terminal(&ev, "case 1");
    assert!(
        !reason(&ev).contains(REASON_BLOCKED),
        "case 1: reason must not cite BLOCKED_TERMINAL; got {:?}",
        reason(&ev)
    );
}

#[test]
fn issue_1510_agent_prose_quoting_exit_79_in_last_round_is_not_terminal() {
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", AGENT_QUOTES_EXIT_79)]);
    assert_not_terminal(&ev, "case 2a (last round)");
    assert!(!reason(&ev).contains(REASON_EXIT_TEXT), "case 2a: {ev}");
}

#[test]
fn issue_1510_agent_prose_quoting_exit_79_in_history_is_not_terminal() {
    let ev = run_collector(&[("LOOP_HISTORY", AGENT_QUOTES_EXIT_79)]);
    assert_not_terminal(&ev, "case 2b (history)");
    assert!(!reason(&ev).contains(REASON_EXIT_TEXT), "case 2b: {ev}");
}

#[test]
fn issue_1510_indented_agent_prose_is_still_dropped() {
    // The ERE allows leading whitespace before the timestamp, same as
    // terminal_refusal(); a log indented by a wrapper is still agent output.
    let text = format!("  {AGENT_QUOTES_TOKEN}\n\t{AGENT_QUOTES_EXIT_79}");
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    assert_not_terminal(&ev, "indented agent prose");
}

// --- Real refusals stay terminal (fail-closed) -----------------------------

#[test]
fn issue_1510_guard_line_at_column_zero_is_terminal() {
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", GUARD_LINE)]);
    assert_terminal_with(&ev, REASON_BLOCKED, "case 3 (column 0)");
}

#[test]
fn issue_1510_guard_line_indented_with_spaces_is_terminal() {
    let text = format!("    {GUARD_LINE}");
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    assert_terminal_with(&ev, REASON_BLOCKED, "case 3 (4 spaces)");
}

#[test]
fn issue_1510_guard_line_indented_with_tab_is_terminal() {
    let text = format!("\t{GUARD_LINE}");
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    assert_terminal_with(&ev, REASON_BLOCKED, "case 3 (tab)");
}

#[test]
fn issue_1510_guard_line_in_history_is_terminal() {
    let ev = run_collector(&[("LOOP_HISTORY", GUARD_LINE)]);
    assert_terminal_with(&ev, REASON_BLOCKED, "case 3 (history)");
}

#[test]
fn issue_1510_child_exit_code_79_with_empty_text_is_terminal() {
    let ev = run_collector(&[("LOOP_CHILD_EXIT_CODE", "79")]);
    assert_terminal_with(&ev, REASON_EXIT_CODE, "case 4a");
}

#[test]
fn issue_1510_child_exit_code_79_with_only_agent_prose_is_terminal() {
    // The structural signal is authoritative: filtering text must never hide it.
    let text = format!("{AGENT_QUOTES_TOKEN}\n{AGENT_QUOTES_EXIT_79}");
    let ev = run_collector(&[
        ("LOOP_CHILD_EXIT_CODE", "79"),
        ("LOOP_LAST_ROUND_OUTPUT", &text),
        ("LOOP_HISTORY", AGENT_QUOTES_EXIT_79),
    ]);
    assert_terminal_with(&ev, REASON_EXIT_CODE, "case 4b");
}

#[test]
fn issue_1510_unprefixed_exit_status_79_is_terminal() {
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", "child exited with status 79")]);
    assert_terminal_with(&ev, REASON_EXIT_TEXT, "case 5");
}

#[test]
fn issue_1510_agent_prose_followed_by_guard_line_is_terminal() {
    let text = format!("{AGENT_QUOTES_TOKEN}\n{GUARD_LINE}");
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    assert_terminal_with(&ev, REASON_BLOCKED, "case 8");
}

#[test]
fn issue_1510_near_miss_prefixes_are_not_filtered() {
    // SR-2: only the exact `[HH:MM:SS] [amplihack:` prefix is dropped. A
    // partial timestamp, another tag, or a different case is NOT agent output,
    // so the token on such a line still counts as a refusal.
    for (case, line) in [
        (
            "short timestamp",
            format!("[10:13] [amplihack:claude:3] {GUARD_LINE}"),
        ),
        (
            "other tag",
            format!("[10:13:17] [other:claude:3] {GUARD_LINE}"),
        ),
        (
            "upper-case tag",
            format!("[10:13:17] [AMPLIHACK:claude:3] {GUARD_LINE}"),
        ),
        (
            "no space",
            format!("[10:13:17][amplihack:claude:3] {GUARD_LINE}"),
        ),
        (
            "prefix mid-line",
            format!("note: [10:13:17] [amplihack:claude:3] {GUARD_LINE}"),
        ),
    ] {
        let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &line)]);
        assert_terminal_with(&ev, REASON_BLOCKED, case);
    }
}

// --- Unchanged behaviour of the rest of the step ---------------------------

#[test]
fn issue_1510_duration_check_still_reads_agent_lines() {
    let text = (1..=3)
        .map(|n| format!("[10:13:1{n}] [amplihack:claude:3] step took 10m 0s"))
        .collect::<Vec<_>>()
        .join("\n");
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    let count = ev["repeated_duration_count"]
        .as_u64()
        .unwrap_or_else(|| panic!("repeated_duration_count must be a number: {ev}"));
    assert!(
        count >= 3,
        "case 6: the duration check must keep reading the unfiltered blob; evidence: {ev}"
    );
    assert_not_terminal(&ev, "case 6");
}

#[test]
fn issue_1510_self_authored_lines_are_still_dropped() {
    let ev = run_collector(&[(
        "LOOP_HISTORY",
        "[loop-health-evaluator] child process exited 79 (policy refusal, #1327/#1332)",
    )]);
    assert_not_terminal(&ev, "case 7 (self-poisoning)");
}

#[test]
fn issue_1510_agent_only_log_does_not_abort_the_step() {
    // SR-6: a log made only of agent lines leaves the filtered blob empty;
    // `grep -v` then exits 1 and must not abort or change the step.
    let ev = run_collector(&[
        ("LOOP_LAST_ROUND_OUTPUT", AGENT_QUOTES_TOKEN),
        ("LOOP_HISTORY", AGENT_QUOTES_EXIT_79),
    ]);
    assert_not_terminal(&ev, "agent-only log");
    assert_eq!(
        reason(&ev),
        "",
        "agent-only log: reason must be empty: {ev}"
    );
}

#[test]
fn issue_1510_filter_ere_matches_autodrive_loop_byte_for_byte() {
    // The two filters must not drift apart.
    const ERE: &str = r"'^[[:space:]]*\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:'";
    let root = workspace_root();
    let loop_sh = std::fs::read_to_string(root.join("amplifier-bundle/tools/autodrive_loop.sh"))
        .expect("read autodrive_loop.sh");
    assert!(
        loop_sh.contains(ERE),
        "autodrive_loop.sh terminal_refusal() ERE changed; update this test and the collector together"
    );
    let cmd = step01_command();
    assert!(
        cmd.contains(&format!("grep -vE {ERE}")),
        "step-01 must drop agent output lines with the same ERE as terminal_refusal()"
    );
    for detector in [
        "grep -qF 'BLOCKED_TERMINAL' \"$WORK/terminal_blob\"",
        "79([^0-9]|$)' \"$WORK/terminal_blob\"",
    ] {
        assert!(
            cmd.contains(detector),
            "both terminal detectors must read $WORK/terminal_blob; missing {detector:?}"
        );
    }
}
