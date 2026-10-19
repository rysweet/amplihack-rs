//! Issue #1570 (finding 9): `FleetAdmiral::start_agent` sends the prompt to the
//! VM on `azlin connect` stdin (stage 1) and launches the agent with only the
//! returned path in argv (stage 2).
//!
//! Contract: `docs/reference/large-payload-delivery.md`, "Fleet admiral
//! `start_agent`". A stub `azlin` records the argv and stdin of every call.

use super::*;
use std::os::unix::fs::PermissionsExt;

const ARG_MAX_STRLEN: usize = 131_072;
const STAGE1: &str = r#"umask 077; f=$(mktemp); cat > "$f"; printf '%s\n' "$f""#;
const REMOTE_PATH: &str = "/tmp/tmp.Ab3_x-9.Q";

struct StubAzlin {
    temp: tempfile::TempDir,
}

impl StubAzlin {
    /// `stage1_stdout` is what stage 1 prints; `stage2_exit` is the exit
    /// status of the `send-keys` call. Only stage 1 reads stdin, so a call
    /// that inherits the test runner's stdin cannot block.
    fn new(stage1_stdout: &str, stage2_exit: i32) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let out = temp.path();
        fs::write(out.join("stage1_out"), stage1_stdout).unwrap();
        let script = format!(
            r#"#!/bin/sh
out='{out}'
n=$(cat "$out/count" 2>/dev/null || echo 0); n=$((n+1)); printf '%s' "$n" > "$out/count"
printf '%s\0' "$@" > "$out/call$n.argv"
last=""
for a in "$@"; do last=$a; done
case "$last" in
  *mktemp*) cat > "$out/call$n.stdin"; cat "$out/stage1_out"; exit 0 ;;
  *send-keys*) exit {stage2_exit} ;;
  *) exit 0 ;;
esac
"#,
            out = out.display()
        );
        let azlin = out.join("azlin");
        fs::write(&azlin, script).unwrap();
        fs::set_permissions(&azlin, fs::Permissions::from_mode(0o755)).unwrap();
        Self { temp }
    }

    fn path(&self) -> PathBuf {
        self.temp.path().join("azlin")
    }

    fn calls(&self) -> usize {
        fs::read_to_string(self.temp.path().join("count"))
            .map(|n| n.trim().parse().unwrap())
            .unwrap_or(0)
    }

    fn argv(&self, n: usize) -> Vec<String> {
        let raw = fs::read(self.temp.path().join(format!("call{n}.argv")))
            .unwrap_or_else(|_| panic!("azlin call {n} was not made"));
        raw.split(|b| *b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect()
    }

    fn stdin(&self, n: usize) -> Vec<u8> {
        fs::read(self.temp.path().join(format!("call{n}.stdin"))).unwrap_or_default()
    }
}

fn task(prompt: &str, mode: &str, max_turns: u32) -> FleetTask {
    FleetTask {
        id: "t1570".to_string(),
        prompt: prompt.to_string(),
        repo_url: String::new(),
        branch: String::new(),
        priority: TaskPriority::Medium,
        status: TaskStatus::Queued,
        agent_command: "claude".to_string(),
        agent_mode: mode.to_string(),
        max_turns,
        protected: false,
        assigned_vm: None,
        assigned_session: None,
        assigned_at: None,
        created_at: now_isoformat(),
        started_at: None,
        completed_at: None,
        result: None,
        pr_url: None,
        error: None,
    }
}

/// Run `start_agent` once and return its result and the task's final status.
fn start(stub: &StubAzlin, task: FleetTask) -> (Result<String>, TaskStatus) {
    let logs = stub.temp.path().join("logs");
    let mut admiral = FleetAdmiral::new(
        stub.path(),
        TaskQueue {
            tasks: vec![task.clone()],
            persist_path: Some(stub.temp.path().join("task_queue.json")),
        },
        Some(logs),
    )
    .unwrap();
    admiral.coordination_dir = stub.temp.path().join("coordination");
    let action = DirectorAction::new(
        ActionType::StartAgent,
        Some(task),
        Some("vm-1".to_string()),
        Some("fleet-t1570".to_string()),
        "issue 1570",
    );
    let result = admiral.start_agent(&action);
    (result, admiral.task_queue.tasks[0].status)
}

fn expected_stage2(turns: u32) -> String {
    format!(
        // Expected text only: start_agent quotes every value with shell_single_quote.
        "tmux new-session -d -s 'fleet-t1570' && tmux send-keys -t 'fleet-t1570' \
         \"amplihack 'claude' --auto --max-turns '{turns}' --prompt-file '{REMOTE_PATH}'; \
         rm -f -- '{REMOTE_PATH}'\" C-m"
    )
}

fn is_error(result: &Result<String>) -> bool {
    match result {
        Ok(outcome) => outcome.starts_with("ERROR"),
        Err(_) => true,
    }
}

#[test]
fn start_agent_sends_prompt_on_stage1_stdin_and_only_the_path_in_stage2() {
    let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 0);
    let prompt = "Implement auth";
    let (result, status) = start(&stub, task(prompt, "auto", 20));
    assert_eq!(result.unwrap(), "Agent started: fleet-t1570 on vm-1");
    assert_eq!(status, TaskStatus::Running);
    assert_eq!(stub.calls(), 2, "stage 1 then stage 2");
    assert_eq!(stub.argv(1).last().unwrap(), STAGE1);
    assert_eq!(stub.stdin(1), prompt.as_bytes());
    assert_eq!(stub.argv(2).last().unwrap(), &expected_stage2(20));
    assert!(stub.argv(2).iter().all(|arg| !arg.contains(prompt)));
}

#[test]
fn hostile_prompt_reaches_only_stdin() {
    let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 0);
    let prompt = "'; touch /tmp/pwned; '";
    let (result, _) = start(&stub, task(prompt, "auto", 5));
    assert!(!is_error(&result), "{result:?}");
    assert_eq!(stub.stdin(1), prompt.as_bytes());
    for n in 1..=stub.calls() {
        assert!(
            stub.argv(n).iter().all(|arg| !arg.contains("pwned")),
            "prompt text in argv of call {n}: {:?}",
            stub.argv(n)
        );
    }
}

#[test]
fn large_prompt_never_reaches_argv() {
    let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 0);
    let mut prompt = String::from("ADMIRAL-1570 é🚀\n");
    while prompt.len() < 200 * 1024 {
        prompt.push_str("admiral task prompt line with 'quotes' and $VARS\n");
    }
    let (result, _) = start(&stub, task(&prompt, "auto", 20));
    assert!(!is_error(&result), "{result:?}");
    for n in 1..=stub.calls() {
        assert!(stub.argv(n).iter().all(|arg| arg.len() <= ARG_MAX_STRLEN));
    }
    assert!(
        stub.stdin(1) == prompt.as_bytes(),
        "stage 1 stdin byte for byte"
    );
}

#[test]
fn only_auto_mode_is_accepted() {
    let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 0);
    let (result, status) = start(&stub, task("p", "ultrathink", 20));
    assert_eq!(
        result.unwrap(),
        r#"ERROR: Invalid agent mode: "ultrathink""#
    );
    assert_eq!(status, TaskStatus::Queued);
    assert_eq!(stub.calls(), 0, "nothing may be sent to the VM");
}

#[test]
fn max_turns_must_be_between_1_and_1000() {
    for turns in [0, 1001] {
        let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 0);
        let (result, _) = start(&stub, task("p", "auto", turns));
        assert!(is_error(&result), "{turns}: {result:?}");
        assert_eq!(stub.calls(), 0);
    }
    for turns in [1, 1000] {
        let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 0);
        let (result, _) = start(&stub, task("p", "auto", turns));
        assert!(!is_error(&result), "{turns}: {result:?}");
        assert_eq!(stub.argv(2).last().unwrap(), &expected_stage2(turns));
    }
}

#[test]
fn an_invalid_stage1_path_stops_before_stage2() {
    for printed in [
        "",
        "\n",
        "relative/tmp.x\n",
        "/tmp/../etc/passwd\n",
        "/tmp/a b\n",
        "/tmp/a'b\n",
        "/tmp/$(id)\n",
        "/tmp/a`id`\n",
        "/tmp/a\\b\n",
        "Welcome to vm-1\n/tmp/a b\n",
        "/tmp/a\nWelcome to vm-1\n",
    ] {
        let stub = StubAzlin::new(printed, 0);
        let (result, status) = start(&stub, task("p", "auto", 5));
        assert!(is_error(&result), "{printed:?} accepted: {result:?}");
        assert_ne!(status, TaskStatus::Running, "{printed:?}");
        assert_eq!(stub.calls(), 1, "{printed:?}: stage 2 must not run");
        assert_eq!(stub.argv(1).last().unwrap(), STAGE1);
    }
}

#[test]
fn a_banner_before_the_stage1_path_is_tolerated() {
    for printed in [
        format!("Welcome to vm-1\n\n{REMOTE_PATH}\n"),
        format!("motd line one\r\nmotd line two\r\n{REMOTE_PATH}\r\n\n"),
    ] {
        let stub = StubAzlin::new(&printed, 0);
        let (result, status) = start(&stub, task("p", "auto", 5));
        assert!(!is_error(&result), "{printed:?} rejected: {result:?}");
        assert_eq!(status, TaskStatus::Running, "{printed:?}");
        assert_eq!(stub.argv(2).last().unwrap(), &expected_stage2(5));
    }
}

#[test]
fn a_stage2_failure_removes_the_remote_prompt_file_once() {
    let stub = StubAzlin::new(&format!("{REMOTE_PATH}\n"), 1);
    let (result, status) = start(&stub, task("p", "auto", 5));
    assert!(is_error(&result), "{result:?}");
    assert_ne!(status, TaskStatus::Running);
    assert_eq!(stub.calls(), 3, "stage 1, stage 2, then one clean-up call");
    assert_eq!(
        stub.argv(3).last().unwrap(),
        &format!("rm -f -- '{REMOTE_PATH}'")
    );
}
