//! Issues #1510 and #1437: the loop-evidence collector must not treat round
//! text as a terminal policy refusal.
//!
//! On PR #1586 a Crusty review quoted the token `BLOCKED_TERMINAL` (and the
//! phrase "exit 79") while explaining how the loop stops. Agent output lines
//! carry the `[HH:MM:SS] [amplihack:...]` prefix. `step-01-collect-loop-evidence`
//! matched the bare token and the exit-79 regex over that prose, so the loop
//! stopped as if the guard had refused.
//!
//! #1510 filtered the text: it cut the runner's run summary, dropped agent
//! and `amplihack.recipe.log_pointer` lines, and required the guard's full
//! phrase. Each filter closed one way in, and #1437 came through another: an
//! agent handled a descendant's refusal inline and then lost its session, and
//! the phrase in its transcript ended the run. Since #1437 step-01 reads no
//! text for this. It sources `amplifier-bundle/tools/autodrive_refusal.sh`,
//! the helper `autodrive_loop.sh` uses, and applies `terminal_refusal` to
//! `loop_child_exit_code` and `loop_last_round_output` only: exit 79, or a
//! failed child whose LAST line beginning `amplihack.recipe.failure_class `
//! has `"structural_refusal": true`.
//!
//! The #1510 cases are kept. Every text that was not a refusal is still not
//! one. Every text that used to be one (the guard's phrase at column 0, an
//! unprefixed "exit status 79") is no longer one, and is checked again with
//! the runner's final marker after it, which is. The filter-sharing tests are
//! replaced by one that checks both readers source the same helper.
//!
//! These tests run the real step-01 command from the YAML through `bash`, in a
//! cleared environment, and parse the single JSON line it prints. They need
//! `bash`, `git` and the built `amplihack` binary, which the helper calls to
//! read a marker's fields.

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

/// The reasons step-01 gave before #1437 for a text match. Neither may appear.
const REASON_BLOCKED: &str = "BLOCKED_TERMINAL reported by a child";
const REASON_EXIT_TEXT: &str = "exit code 79 present in round output";
const REASON_EXIT_CODE: &str = "child process exited 79";
const REASON_MARKER: &str = "structural_refusal in failure_class marker";

/// The runner's own final marker for a run a structural guard refused (#1437).
const REFUSED_MARKER: &str = r#"amplihack.recipe.failure_class {"action":"terminal","attempt":1,"class":"work","retryable":false,"signal":"blocked_terminal orchestration_unavailable","structural_refusal":true}"#;

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
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // #1437: step-01 sources autodrive_refusal.sh from AMPLIHACK_HOME,
        // and the helper reads marker fields with `amplihack orch helper`.
        .env("AMPLIHACK_HOME", workspace_root())
        .env("AMPLIHACK_BIN", env!("CARGO_BIN_EXE_amplihack"));
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
        "{case}: text echoed from inputs must not be a terminal refusal; evidence: {evidence}"
    );
}

/// Not terminal, and not for any of the text-match reasons #1437 removed.
fn assert_text_is_not_terminal(evidence: &Json, case: &str) {
    assert_not_terminal(evidence, case);
    for old in [REASON_BLOCKED, REASON_EXIT_TEXT] {
        assert!(
            !reason(evidence).contains(old),
            "{case}: no text match may set terminal_reason; got {:?}",
            reason(evidence)
        );
    }
}

/// `text` alone is not a refusal, whatever the exit status; with the
/// runner's final marker after it and a failed child, it is (#1437).
fn assert_only_the_marker_refuses(var: &str, text: &str, case: &str) {
    assert_text_is_not_terminal(&run_collector(&[(var, text)]), case);
    for rc in ["1", "2"] {
        let ev = run_collector(&[(var, text), ("LOOP_CHILD_EXIT_CODE", rc)]);
        assert_text_is_not_terminal(&ev, &format!("{case}, exit {rc}"));
    }
    if var == "LOOP_LAST_ROUND_OUTPUT" {
        let with_marker = format!("{text}\n{REFUSED_MARKER}");
        let ev = run_collector(&[(var, &with_marker), ("LOOP_CHILD_EXIT_CODE", "1")]);
        assert_terminal_with(
            &ev,
            REASON_MARKER,
            &format!("{case}, then the final marker"),
        );
    }
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

#[test]
fn issue_1510_runner_failure_tail_with_bare_token_is_not_terminal() {
    // When an agent step fails, recipe-runner-rs reprints the agent's stdout
    // WITHOUT the `[HH:MM:SS] [amplihack:...]` prefix, indented under
    // `--- stdout (tail) ---`. The agent-line filter cannot see that it is
    // agent output, so the token check must require the guard's full marker,
    // as terminal_refusal() does.
    let text = "    --- stdout (tail) ---\n    the loop only stops on BLOCKED_TERMINAL";
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", text)]);
    assert_not_terminal(&ev, "runner failure tail, bare token");
    assert!(
        !reason(&ev).contains(REASON_BLOCKED),
        "runner failure tail: reason must not cite BLOCKED_TERMINAL; got {:?}",
        reason(&ev)
    );
}

#[test]
fn issue_1510_bare_token_without_marker_is_not_terminal() {
    // Unprefixed text at column 0 that names the token but is not the guard's
    // `BLOCKED_TERMINAL orchestration_unavailable` line.
    for line in [
        "BLOCKED_TERMINAL",
        "child returned BLOCKED_TERMINAL at depth 4/3",
        "BLOCKED_TERMINAL: something else",
    ] {
        let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", line)]);
        assert_not_terminal(&ev, line);
    }
}

// --- The guard's text is not a refusal; the runner's marker is (#1437) ------

#[test]
fn issue_1437_guard_line_text_is_not_terminal_in_any_position() {
    for (case, text) in [
        ("column 0", GUARD_LINE.to_string()),
        ("4 spaces", format!("    {GUARD_LINE}")),
        ("tab", format!("\t{GUARD_LINE}")),
        (
            "after agent prose",
            format!("{AGENT_QUOTES_TOKEN}\n{GUARD_LINE}"),
        ),
        (
            "unprefixed exit status",
            "child exited with status 79".to_string(),
        ),
    ] {
        assert_only_the_marker_refuses("LOOP_LAST_ROUND_OUTPUT", &text, case);
    }
    assert_only_the_marker_refuses("LOOP_HISTORY", GUARD_LINE, "history");
}

#[test]
fn issue_1510_child_exit_code_79_with_empty_text_is_terminal() {
    let ev = run_collector(&[("LOOP_CHILD_EXIT_CODE", "79")]);
    assert_terminal_with(&ev, REASON_EXIT_CODE, "case 4a");
}

#[test]
fn issue_1510_child_exit_code_79_with_only_agent_prose_is_terminal() {
    // The structural signal is authoritative: no text can hide it.
    let text = format!("{AGENT_QUOTES_TOKEN}\n{AGENT_QUOTES_EXIT_79}");
    let ev = run_collector(&[
        ("LOOP_CHILD_EXIT_CODE", "79"),
        ("LOOP_LAST_ROUND_OUTPUT", &text),
        ("LOOP_HISTORY", AGENT_QUOTES_EXIT_79),
    ]);
    assert_terminal_with(&ev, REASON_EXIT_CODE, "case 4b");
}

#[test]
fn issue_1437_final_marker_needs_a_failed_child_a_boolean_and_column_0() {
    let ev = run_collector(&[
        ("LOOP_LAST_ROUND_OUTPUT", REFUSED_MARKER),
        ("LOOP_CHILD_EXIT_CODE", "1"),
    ]);
    assert_terminal_with(&ev, REASON_MARKER, "the final marker of a failed child");
    let later_false = format!(
        "{REFUSED_MARKER}\n{}",
        REFUSED_MARKER.replace(
            "\"structural_refusal\":true",
            "\"structural_refusal\":false"
        )
    );
    for (case, rc, text) in [
        ("exit 0", "0", REFUSED_MARKER.to_string()),
        ("no exit code", "", REFUSED_MARKER.to_string()),
        ("non-numeric exit code", "x1", REFUSED_MARKER.to_string()),
        ("indented", "1", format!("  {REFUSED_MARKER}")),
        (
            "string \"true\"",
            "1",
            REFUSED_MARKER.replace(
                "\"structural_refusal\":true",
                "\"structural_refusal\":\"true\"",
            ),
        ),
        ("a later false marker", "1", later_false),
    ] {
        let ev = run_collector(&[
            ("LOOP_LAST_ROUND_OUTPUT", &text),
            ("LOOP_CHILD_EXIT_CODE", rc),
        ]);
        assert_not_terminal(&ev, case);
    }
    let ev = run_collector(&[
        ("LOOP_HISTORY", REFUSED_MARKER),
        ("LOOP_CHILD_EXIT_CODE", "1"),
    ]);
    assert_not_terminal(&ev, "a marker in history only");
}

#[test]
fn issue_1510_near_miss_prefixes_are_not_terminal_either() {
    // Before #1437 these were kept from the agent-line filter so the guard's
    // phrase on them still counted. Text no longer counts on any line.
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
        assert_only_the_marker_refuses("LOOP_LAST_ROUND_OUTPUT", &line, case);
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
fn issue_1437_collector_and_loop_share_one_refusal_helper() {
    // The two readers cannot drift apart: both call terminal_refusal from
    // autodrive_refusal.sh, and neither reads the guard's text.
    let root = workspace_root();
    let loop_sh = std::fs::read_to_string(root.join("amplifier-bundle/tools/autodrive_loop.sh"))
        .expect("read autodrive_loop.sh");
    let code = |text: &str| -> String {
        text.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(code(&loop_sh).contains(". \"${LOOP_HOME}/autodrive_refusal.sh\""));
    assert!(!code(&loop_sh).contains("terminal_refusal() {"));
    let cmd = code(&step01_command());
    assert!(
        cmd.contains("amplifier-bundle/tools/autodrive_refusal.sh"),
        "step-01 must source autodrive_refusal.sh"
    );
    assert!(cmd.contains("terminal_refusal \"${LOOP_CHILD_EXIT_CODE:-}\" \"$WORK/last\""));
    for gone in ["BLOCKED_TERMINAL", "terminal_blob", "79([^0-9]|$)"] {
        assert!(
            !loop_sh
                .lines()
                .any(|l| !l.trim_start().starts_with('#') && l.contains(gone))
                && !cmd.contains(gone),
            "no reader may match {gone:?} in text (#1437)"
        );
    }
}

// --- Runner log_pointer lines echo task_description (round 1 of PR #1632) --

/// The task text that stopped round 1 of PR #1632: it names the full marker
/// and an exit-79 phrase, and the runner echoes it in every log_pointer line.
const TASK_NAMING_MARKER: &str = "Fix #1510: a review quoting BLOCKED_TERMINAL orchestration_unavailable or saying the guard returns exit 79 must not stop the loop";

/// A log_pointer line built the way `correlation.rs` builds it: the
/// `amplihack.recipe.log_pointer ` prefix and one line of serde_json.
fn log_pointer_line(event: &str, exit_code: Option<i32>) -> String {
    let mut payload = serde_json::json!({
        "schema_version": 1,
        "event": event,
        "run_id": "00000000-0000-0000-0000-000000000000",
        "recipe_name": "default-workflow",
        "cwd": "/tmp/w",
        "task_description": TASK_NAMING_MARKER,
        "runner_path": "/usr/bin/amplihack",
        "timestamp": "2026-10-19T00:00:00Z",
    });
    if let Some(code) = exit_code {
        payload["exit_code"] = code.into();
        payload["status"] = "failure".into();
    }
    format!("amplihack.recipe.log_pointer {payload}")
}

fn log_pointer_round() -> String {
    format!(
        "{}\nordinary runner output\n{}",
        log_pointer_line("early", None),
        log_pointer_line("final", Some(79))
    )
}

#[test]
fn issue_1510_log_pointer_task_description_naming_the_marker_is_not_terminal() {
    let text = log_pointer_round();
    assert!(text.contains("BLOCKED_TERMINAL orchestration_unavailable"));
    assert!(text.contains("exit 79"));
    for var in ["LOOP_LAST_ROUND_OUTPUT", "LOOP_HISTORY"] {
        let ev = run_collector(&[(var, &text)]);
        assert_not_terminal(&ev, &format!("log_pointer in {var}"));
    }
}

#[test]
fn issue_1510_indented_log_pointer_line_is_not_terminal() {
    let text = format!(
        "  {}\n\t{}",
        log_pointer_line("early", None),
        log_pointer_line("final", Some(79))
    );
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    assert_not_terminal(&ev, "indented log_pointer");
}

#[test]
fn issue_1437_guard_line_after_log_pointer_is_not_terminal() {
    let text = format!("{}\n{GUARD_LINE}", log_pointer_round());
    assert_only_the_marker_refuses(
        "LOOP_LAST_ROUND_OUTPUT",
        &text,
        "log_pointer then guard line",
    );
}

#[test]
fn issue_1510_log_pointer_exit_79_defers_to_the_child_exit_code() {
    // The log_pointer exit_code is dropped with its line on purpose; the
    // structural signal is loop_child_exit_code, which reads no text.
    let text = log_pointer_line("final", Some(79));
    let ev = run_collector(&[
        ("LOOP_LAST_ROUND_OUTPUT", &text),
        ("LOOP_CHILD_EXIT_CODE", "79"),
    ]);
    assert_terminal_with(&ev, REASON_EXIT_CODE, "log_pointer with child exit 79");
}

#[test]
fn issue_1437_guard_line_in_any_json_is_not_terminal() {
    // Before #1437 only a line that began `amplihack.recipe.log_pointer {` was
    // dropped, so these were refusals. No text is one now.
    for (case, line) in [
        ("other JSON object", format!(r#"{{"note":"{GUARD_LINE}"}}"#)),
        (
            "key mid-line",
            format!(r#"echo amplihack.recipe.log_pointer {{"x":"{GUARD_LINE}"}}"#),
        ),
        (
            "key without object",
            format!("amplihack.recipe.log_pointer {GUARD_LINE}"),
        ),
        (
            "upper-case key",
            format!(r#"AMPLIHACK.RECIPE.LOG_POINTER {{"x":"{GUARD_LINE}"}}"#),
        ),
        (
            "other key",
            format!(r#"amplihack.recipe.other {{"x":"{GUARD_LINE}"}}"#),
        ),
    ] {
        assert_only_the_marker_refuses("LOOP_LAST_ROUND_OUTPUT", &line, case);
    }
}

// --- terminal_refusal() from autodrive_refusal.sh, sourced and run ----------

/// Run `terminal_refusal <exit_code> <log>` from `autodrive_refusal.sh`, the
/// helper `autodrive_loop.sh` sources, and return whether it called the child
/// a refusal.
fn terminal_refusal(exit_code: &str, log: &str) -> bool {
    let scratch = ScratchDir::new();
    let log_path = scratch.0.join("round.log");
    std::fs::write(&log_path, log).expect("write round log");
    let helper = workspace_root().join("amplifier-bundle/tools/autodrive_refusal.sh");
    let mut bash = Command::new("bash");
    isolated(&mut bash, &scratch.0);
    let out = bash
        .arg("-c")
        .arg(". \"$3\" && terminal_refusal \"$1\" \"$2\"")
        .arg("terminal_refusal")
        .arg(exit_code)
        .arg(&log_path)
        .arg(&helper)
        .output()
        .expect("spawn bash");
    match out.status.code() {
        Some(0) => true,
        Some(1) => false,
        other => panic!(
            "terminal_refusal exited {other:?}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

#[test]
fn issue_1510_terminal_refusal_ignores_log_pointer_task_description() {
    assert!(
        !terminal_refusal("1", &log_pointer_round()),
        "a log_pointer line echoing a task that names the marker is not a refusal"
    );
    let indented = format!("    {}", log_pointer_line("final", Some(79)));
    assert!(
        !terminal_refusal("1", &indented),
        "indented log_pointer line"
    );
}

#[test]
fn issue_1437_terminal_refusal_reads_the_marker_not_the_guard_line() {
    let log = format!("{}\n{GUARD_LINE}\n", log_pointer_round());
    assert!(
        !terminal_refusal("1", &log),
        "the guard's phrase is text; only the runner's marker refuses (#1437)"
    );
    assert!(
        terminal_refusal("1", &format!("{log}{REFUSED_MARKER}\n")),
        "the runner's final marker after it does"
    );
}

#[test]
fn issue_1510_terminal_refusal_exit_79_needs_no_text() {
    assert!(
        terminal_refusal("79", ""),
        "exit 79 is the structural signal"
    );
    assert!(
        terminal_refusal("79", &log_pointer_round()),
        "exit 79 stops the loop whatever the log says"
    );
}

#[test]
fn issue_1510_terminal_refusal_ignores_agent_prose_and_bare_token() {
    let log = format!("{AGENT_QUOTES_TOKEN}\nchild returned BLOCKED_TERMINAL at depth 4/3\n");
    assert!(!terminal_refusal("0", &log));
}

#[test]
fn issue_1437_terminal_refusal_ignores_the_guard_line_inside_json() {
    let log = format!("{{\"note\":\"{GUARD_LINE}\"}}\n");
    assert!(!terminal_refusal("0", &log));
    assert!(!terminal_refusal("1", &log));
}

// --- The byte tail handed to the collector ---------------------------------

fn run_round_tail(log: &[u8]) -> Vec<u8> {
    let scratch = ScratchDir::new();
    let path = scratch.0.join("round.log");
    std::fs::write(&path, log).expect("write round log");
    let tool = workspace_root().join("amplifier-bundle/tools/autodrive_round_tail.sh");
    let mut bash = Command::new("bash");
    isolated(&mut bash, &scratch.0);
    let out = bash.arg(&tool).arg(&path).output().expect("spawn bash");
    assert!(
        out.status.success(),
        "autodrive_round_tail.sh exited {:?}",
        out.status.code()
    );
    out.stdout
}

#[test]
fn issue_1510_autodrive_loop_passes_autodrive_round_tail_not_a_raw_byte_tail() {
    let loop_sh =
        std::fs::read_to_string(workspace_root().join("amplifier-bundle/tools/autodrive_loop.sh"))
            .expect("read autodrive_loop.sh");
    assert!(
        loop_sh.contains(
            "-c \"loop_last_round_output=$(bash \"${LOOP_HOME}/autodrive_round_tail.sh\" \"$LOG\")\""
        ),
        "loop_last_round_output must come from autodrive_round_tail.sh, which drops the partial first line"
    );
    assert!(
        !loop_sh.contains("loop_last_round_output=$(tail -c"),
        "a raw byte tail leaves a prefix-less fragment as the first line"
    );
}

#[test]
fn issue_1510_round_tail_drops_the_partial_first_line_of_a_long_log() {
    // The cut lands inside an agent line, leaving only its unprefixed end.
    let agent = "[10:13:17] [amplihack:claude:3] the loop only stops on exit 79 or BLOCKED_TERMINAL orchestration_unavailable\n";
    // Whole agent lines after it, sized so the cut falls mid-way through it.
    let want = 20_000 - agent.len() / 2;
    let mut rest = String::new();
    while rest.len() + 200 < want {
        rest.push_str("[10:13:18] [amplihack:claude:3] filler line of ordinary review prose\n");
    }
    let pad = want - rest.len() - "[10:13:19] [amplihack:claude:3] \n".len();
    rest.push_str(&format!(
        "[10:13:19] [amplihack:claude:3] {}\n",
        "x".repeat(pad)
    ));
    assert_eq!(rest.len(), want);
    let long = format!("{agent}{rest}").into_bytes();
    let cut = long.len() - 20_000;
    assert!(
        cut > 0 && cut < agent.len(),
        "the cut must fall inside the agent line"
    );

    let tail = run_round_tail(&long);
    let text = String::from_utf8_lossy(&tail);
    assert!(
        text.lines().all(|l| l.starts_with("[10:13:")),
        "every line autodrive_round_tail.sh prints must be whole; first line: {:?}",
        text.lines().next()
    );
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &text)]);
    assert_not_terminal(&ev, "byte tail cut inside an agent line");
}

#[test]
fn issue_1510_round_tail_keeps_a_short_log_whole() {
    let log = format!("{GUARD_LINE}\nsecond line\n");
    assert_eq!(run_round_tail(log.as_bytes()), log.as_bytes());
}

// --- The runner's run summary echoes step output (round 3 of PR #1632) -----
//
// On success the runner prints a summary after the run: `Recipe:`, `Run ID:`,
// `Status:`, `Log pointer:`, then `Steps:` with each step's first 200
// characters under `    Output: `. An agent step's output is multi-line and
// its first line is the `ℹ NODE_OPTIONS` notice, so the agent's own prose
// follows at column 0 with no prefix. #1510 cut the log at the summary
// header for that reason; since #1437 no text is read at all, so the summary
// cannot stop the loop whatever it reprints.

/// The tail of a real successful crusty round log (crusty-round-2.log of
/// PR #1632, paths replaced), edited the way crusty asked: the review's
/// first line quotes the full marker and "exit 79", and step-01 echoes
/// resolved_concerns ids that contain "exit-79".
fn real_round_tail() -> String {
    let path = workspace_root().join("tests/integration/fixtures/issue_1510_crusty_round_tail.txt");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn issue_1510_fixture_carries_the_echo_at_column_zero_inside_the_summary() {
    // Guards the fixture itself: if it stops carrying the shape, the tests
    // below stop proving anything.
    let log = real_round_tail();
    let lines: Vec<&str> = log.lines().collect();
    let header = lines
        .iter()
        .position(|l| l.starts_with("Recipe: "))
        .expect("fixture must contain the runner summary header");
    assert!(lines[header + 1].starts_with("Run ID: "));
    for needle in [
        "Steps:",
        "    Output: ℹ NODE_OPTIONS",
        "\"resolved_concerns\":\"exit-79-regex-reads-unprefixed-prose",
        "Log pointer: status=success",
    ] {
        assert!(log.contains(needle), "fixture must contain {needle:?}");
    }
    let echo = lines
        .iter()
        .position(|l| {
            l.starts_with("CLEAN: echoed BLOCKED_TERMINAL orchestration_unavailable and exit 79")
        })
        .expect("fixture must carry the agent's echo at column 0");
    assert!(echo > header, "the echo must sit inside the summary");
    // The echo is not hidden by the older rules: it has no agent prefix and
    // no log_pointer key, so only the summary cut can drop it.
    assert!(!lines[echo].starts_with('['));
}

#[test]
fn issue_1510_collector_real_round_summary_echo_is_not_terminal() {
    let log = real_round_tail();
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &log)]);
    assert_not_terminal(&ev, "real round summary quoting the marker and exit 79");
    let ev = run_collector(&[
        ("LOOP_LAST_ROUND_OUTPUT", &log),
        ("LOOP_CHILD_EXIT_CODE", "0"),
    ]);
    assert_not_terminal(&ev, "real round summary, child exit 0");
}

#[test]
fn issue_1510_terminal_refusal_real_round_summary_echo_is_not_terminal() {
    assert!(
        !terminal_refusal("0", &real_round_tail()),
        "a clean round whose summary echoes the marker is not a refusal"
    );
}

/// The fixture with `line` inserted just before the runner's final
/// log_pointer line, i.e. before the summary.
fn real_round_tail_with_line_before_summary(line: &str) -> String {
    let log = real_round_tail();
    let at = log
        .find("amplihack.recipe.log_pointer {\"schema_version\":1,\"event\":\"final\"")
        .expect("fixture must contain the final log_pointer line");
    format!("{}{line}\n{}", &log[..at], &log[at..])
}

#[test]
fn issue_1437_guard_line_before_the_summary_is_not_terminal() {
    for line in [GUARD_LINE, "child exit status 79"] {
        let log = real_round_tail_with_line_before_summary(line);
        assert_only_the_marker_refuses("LOOP_LAST_ROUND_OUTPUT", &log, line);
        assert!(!terminal_refusal("1", &log), "terminal_refusal: {line}");
    }
}

#[test]
fn issue_1510_exit_79_still_stops_a_round_whose_summary_echoes_the_marker() {
    let log = real_round_tail();
    let ev = run_collector(&[
        ("LOOP_LAST_ROUND_OUTPUT", &log),
        ("LOOP_CHILD_EXIT_CODE", "79"),
    ]);
    assert_terminal_with(&ev, REASON_EXIT_CODE, "summary cut, child exit 79");
    assert!(terminal_refusal("79", &log));
}

#[test]
fn issue_1437_a_summary_header_neither_hides_nor_makes_a_refusal() {
    // #1510 cut the log at the exact header pair, so a guard line after a
    // near-miss header was a refusal and one after the real header was not.
    // Neither is one now, and the final marker after either one still is.
    for (case, before) in [
        (
            "Recipe: alone",
            "Recipe: autodrive-crusty-round\nsomething else",
        ),
        ("indented header", "  Recipe: x\n  Status: ✓ Success"),
        ("indented second line", "Recipe: x\n  Run ID: y"),
        (
            "pair split by a blank line",
            "Recipe: x\n\nStatus: ✓ Success",
        ),
        ("other key", "Recipe: x\nSteps:"),
        ("lower-case header", "recipe: x\nstatus: ✓ Success"),
        ("Run ID form", "Recipe: x\nRun ID: 1"),
        ("Status form (no run id)", "Recipe: x\nStatus: ✗ Failed"),
    ] {
        let log = format!("{before}\n{GUARD_LINE}");
        assert_only_the_marker_refuses("LOOP_LAST_ROUND_OUTPUT", &log, case);
        assert!(!terminal_refusal("0", &log), "terminal_refusal: {case}");
        assert!(
            terminal_refusal("1", &format!("{log}\n{REFUSED_MARKER}")),
            "terminal_refusal with the final marker: {case}"
        );
    }
}

#[test]
fn issue_1437_a_guard_line_in_the_last_round_is_not_terminal_beside_a_history_header() {
    let ev = run_collector(&[
        ("LOOP_HISTORY", "Recipe: x\nStatus: ✓ Success"),
        ("LOOP_LAST_ROUND_OUTPUT", GUARD_LINE),
        ("LOOP_CHILD_EXIT_CODE", "1"),
    ]);
    assert_text_is_not_terminal(&ev, "header in history, guard in last round");
}

#[test]
fn issue_1510_round_tail_keeps_the_summary_header_of_a_long_summary() {
    // A summary longer than the 20000-byte window: the window starts inside
    // the summary and would lose its header, leaving its body unanchored.
    let mut log = String::from("[10:00:00] [amplihack:claude:3] review prose\n");
    log.push_str("Recipe: autodrive-crusty-round\nRun ID: 1\nStatus: ✗ Failed\n\nSteps:\n");
    log.push_str("  ✗ step-02-crusty-review: failed\n    Error: agent step failed\n");
    while log.len() < 30_000 {
        log.push_str("    stdout line of the failed agent step, reprinted by the runner\n");
    }
    log.push_str("BLOCKED_TERMINAL orchestration_unavailable quoted by the agent, exit 79\n");
    log.push_str("  ⊘ step-03: skipped\n");

    let tail = String::from_utf8(run_round_tail(log.as_bytes())).expect("utf-8 tail");
    assert!(
        tail.starts_with("Recipe: autodrive-crusty-round\nRun ID: 1\n"),
        "the summary header must lead the tail; got {:?}",
        tail.lines().next()
    );
    let ev = run_collector(&[("LOOP_LAST_ROUND_OUTPUT", &tail)]);
    assert_not_terminal(&ev, "tail of a summary longer than the window");
}

#[test]
fn issue_1510_round_tail_does_not_repeat_a_header_already_in_the_window() {
    let mut log = String::new();
    while log.len() < 25_000 {
        log.push_str("[10:00:00] [amplihack:claude:3] filler review prose line\n");
    }
    let summary = "Recipe: r\nRun ID: 1\nStatus: ✓ Success\n\nSteps:\n  ✓ s: completed\n";
    log.push_str(summary);
    let tail = String::from_utf8(run_round_tail(log.as_bytes())).expect("utf-8 tail");
    assert_eq!(tail.matches("Recipe: r\n").count(), 1);
    assert!(tail.ends_with(summary));
    assert!(tail.lines().next().unwrap().starts_with("[10:00:00]"));
}
