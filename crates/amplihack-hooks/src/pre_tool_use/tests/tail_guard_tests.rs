//! Issue #1794: the PreToolUse guard denies `tail` in follow mode on a device
//! that never ends, and lets through every command that only mentions it as
//! data. None of these tests runs a command; they only parse text.
//!
//! Contract (items in `tail_guard.rs`):
//! - `is_shell_tool(&str) -> bool`
//! - `check(&str) -> Option<Value>` for a command string
//! - `check_tool_input(&Value) -> Option<Value>` for `tool_input`
//! - `BLOCK_MESSAGE: &str`, the fixed reason text
//!
//! Every `Some` is a deny.

use super::*;
use crate::pre_tool_use::PreToolUseHook;
use crate::protocol::Hook;
use crate::test_support::env_lock;
use amplihack_types::HookInput;
use serde_json::{Value, json};

fn denied(command: &str) -> bool {
    check(command).is_some()
}

const EXPECTED_REASON: &str = "`tail -f` on /dev/null (or another device that never ends) \
leaves an orphaned process. Use `amplihack wait --seconds N`, or run the work in the \
background and use `amplihack wait --until pid-exited:<PID> --deadline <SECS>`.";

// ---------------------------------------------------------------- denied

#[test]
fn denies_the_acceptance_commands() {
    for cmd in [
        "tail -f /dev/null",
        "nohup tail -f /dev/null &",
        "sleep 5; tail -f /dev/null",
        "cd /repo && tail -f /dev/null",
        "timeout 600 tail -f /dev/null",
    ] {
        assert!(denied(cmd), "{cmd:?} must be denied");
    }
}

#[test]
fn denies_every_follow_flag_form() {
    for cmd in [
        "tail -F /dev/null",
        "tail --follow /dev/null",
        "tail --follow=name /dev/null",
        "tail --follow=descriptor /dev/null",
        "tail -F --retry /dev/null",
        "tail --retry -F /dev/null",
        "tail -fn0 /dev/null",
        "tail -Fq /dev/null",
        "tail -qf /dev/null",
        "tail -n 0 -f /dev/null",
        "tail -f -n 0 /dev/null",
        "tail -s 1 -f /dev/null",
    ] {
        assert!(denied(cmd), "{cmd:?} must be denied");
    }
}

#[test]
fn denies_every_listed_device() {
    for dev in ["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"] {
        let cmd = format!("tail -f {dev}");
        assert!(denied(&cmd), "{cmd:?} must be denied");
    }
}

#[test]
fn denies_through_wrappers() {
    for cmd in [
        "setsid tail -f /dev/null",
        "setsid -f tail -f /dev/null",
        "env tail -f /dev/null",
        "env -i FOO=1 tail -f /dev/null",
        "FOO=1 BAR=2 tail -f /dev/null",
        "sudo tail -f /dev/null",
        "sudo -u app tail -F /dev/zero",
        "sudo -g grp -n tail -f /dev/null",
        "nice tail -f /dev/null",
        "nice -n 5 tail -f /dev/null",
        "exec tail -f /dev/null",
        "command tail -f /dev/null",
        "stdbuf -oL tail -f /dev/null",
        "nohup nice -n 5 setsid tail -f /dev/null",
        "/usr/bin/tail -f /dev/null",
        "timeout --foreground 10 tail -f /dev/null",
        "timeout -s KILL 5 tail -f /dev/null",
        "timeout --signal=KILL 5 tail -f /dev/null",
        "timeout --signal KILL 5 tail -f /dev/null",
        "timeout -k 1 5 tail -f /dev/null",
        "timeout --kill-after=1 5 tail -f /dev/null",
        "timeout --preserve-status -v 5s tail -f /dev/null",
    ] {
        assert!(denied(cmd), "{cmd:?} must be denied");
    }
}

#[test]
fn denies_through_reserved_words_and_separators() {
    for cmd in [
        "tail -f /dev/null | cat",
        "true || tail -f /dev/null",
        "(tail -f /dev/null)",
        "echo hi\ntail -f /dev/null",
        "if true; then tail -f /dev/null; fi",
        "while true; do tail -f /dev/null; done",
        "{ tail -f /dev/null; }",
        "! tail -f /dev/null",
        "time tail -f /dev/null",
        "tail -f /dev/null 2>/dev/null",
        "tail -f /dev/null > out.log",
    ] {
        assert!(denied(cmd), "{cmd:?} must be denied");
    }
}

#[test]
fn denies_when_any_operand_is_a_device() {
    for cmd in [
        "tail -f a.log /dev/null",
        "tail -f /dev/null a.log",
        "tail -f -- /dev/null",
        "tail -f '/dev/null'",
        "tail -f \"/dev/null\"",
    ] {
        assert!(denied(cmd), "{cmd:?} must be denied");
    }
}

// --------------------------------------------------------------- allowed

#[test]
fn allows_ordinary_tail_use() {
    for cmd in [
        "tail -f some.log",
        "tail -F app.log",
        "tail -n 5 /dev/null",
        "tail -n5 /dev/null",
        "tail -c5f /dev/null",
        "tail /dev/null",
        "tail -- -f /dev/null.log",
        "tail -n 5 -- -f",
        "tail -f /dev/nullx",
        "tail -f /dev/null2",
        "tail -f /dev/stdin",
        "tail --retry /dev/null",
        "",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

#[test]
fn allows_mentions_as_data() {
    for cmd in [
        "echo 'tail -f /dev/null'",
        "echo tail -f /dev/null",
        "printf '%s\\n' 'tail -f /dev/null'",
        "printf 'CMD tail -f /dev/null\\n' > Dockerfile",
        "grep -r 'tail -f /dev/null' docs/",
        "git commit -m 'stop using tail -f /dev/null'",
        "# tail -f /dev/null",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

#[test]
fn allows_heredoc_bodies() {
    for cmd in [
        "cat > Dockerfile <<'EOF'\nFROM alpine\nCMD tail -f /dev/null\nEOF",
        "cat > Dockerfile <<EOF\nCMD tail -f /dev/null\nEOF",
        "cat > run.sh <<\"EOF\"\ntail -f /dev/null\nEOF",
        "cat > run.sh <<-EOF\n\ttail -f /dev/null\n\tEOF",
        "cat <<EOF | tee compose.yml\ncommand: tail -f /dev/null\nEOF",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

#[test]
fn heredoc_does_not_hide_a_later_command() {
    let cmd = "cat > f <<EOF\nbody\nEOF\ntail -f /dev/null";
    assert!(denied(cmd), "{cmd:?} must be denied");
}

/// The reviewer's reproduction: a `"` inside a quoted `$(cat <<'EOF' …)`
/// body must not end the outer quote and expose the next line as a command.
const QUOTED_HEREDOC_BODY: &str = "gh pr create --body \"$(cat <<'EOF'
The guard denies \"
tail -f /dev/null
\" and points to wait.
EOF
)\"";

#[test]
fn allows_substitution_inside_double_quotes() {
    let input = json!({"command": QUOTED_HEREDOC_BODY});
    assert!(
        check_tool_input(&input).is_none(),
        "{input} must be allowed"
    );
    for cmd in [
        QUOTED_HEREDOC_BODY,
        "git commit -m \"$(cat <<'EOF'\nThe guard doesn't deny \"\ntail -f /dev/null\n\" here.\nEOF\n)\"",
        "git commit -m \"$(printf '%s' \"tail -f /dev/null\")\"",
        "echo \"$(echo \"$(echo \"tail -f /dev/null\")\")\"",
        "echo \"$(echo \\\")\ntail -f /dev/null\n\\\")\"",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

#[test]
fn substitution_inside_double_quotes_does_not_hide_a_later_command() {
    for cmd in [
        "echo \"$(date)\"; tail -f /dev/null",
        "echo \"$(echo ')')\" && tail -f /dev/null",
    ] {
        assert!(denied(cmd), "{cmd:?} must be denied");
    }
}

#[test]
fn allows_compose_command_written_with_tee() {
    for cmd in [
        "echo '    command: tail -f /dev/null' | tee -a compose.yml",
        "tee compose.yml <<'EOF'\nservices:\n  app:\n    command: tail -f /dev/null\nEOF",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

#[test]
fn allows_container_keep_alive_arguments() {
    for cmd in [
        "docker run img tail -f /dev/null",
        "docker run -d --name c img tail -f /dev/null",
        "docker create img tail -f /dev/null",
        "podman run img tail -f /dev/null",
        "podman create img tail -F /dev/zero",
        "docker exec c tail -f /dev/null",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

#[test]
fn does_not_reparse_shell_c_strings() {
    for cmd in [
        "bash -c \"sleep 1 && tail -f /dev/null\"",
        "sh -c 'tail -f /dev/null'",
        "bash -lc 'tail -f /dev/null'",
    ] {
        assert!(!denied(cmd), "{cmd:?} must be allowed");
    }
}

// ----------------------------------------------------- tool input shapes

#[test]
fn tool_input_shapes() {
    let deny = [
        json!({"command": "tail -f /dev/null"}),
        json!({"cmd": "tail -f /dev/null"}),
        json!({"command": ["tail", "-f", "/dev/null"]}),
        json!({"cmd": ["tail", "-f", "/dev/null"]}),
        json!({"command": ["timeout", "--foreground", "10", "tail", "-f", "/dev/null"]}),
        json!({"command": "tail -f /dev/null", "description": "wait"}),
    ];
    for input in deny {
        assert!(check_tool_input(&input).is_some(), "{input} must be denied");
    }

    let allow = [
        json!({}),
        json!(null),
        json!("tail -f /dev/null"),
        json!({"command": 42}),
        json!({"command": null}),
        json!({"command": []}),
        json!({"command": [1, 2, 3]}),
        json!({"command": ["tail", "-f", "some.log"]}),
        // An argv is joined with shell quoting: the `-c` string stays one
        // word and is not re-parsed.
        json!({"command": ["bash", "-lc", "tail -f /dev/null"]}),
        // Quoting keeps a single argv word with spaces as one operand.
        json!({"command": ["tail", "-f", "/dev/null x"]}),
        json!({"command": ["echo", "tail -f /dev/null"]}),
        json!({"command": ["echo", "x; tail -f /dev/null"]}),
        json!({"description": "tail -f /dev/null"}),
    ];
    for input in allow {
        assert!(
            check_tool_input(&input).is_none(),
            "{input} must be allowed"
        );
    }
}

// ------------------------------------------------------------ tool names

#[test]
fn tool_names() {
    for name in [
        "bash",
        "Bash",
        "BASH",
        "shell",
        "Shell",
        "exec_command",
        "EXEC_COMMAND",
        "shell_command",
        "Monitor",
        "PowerShell",
    ] {
        assert!(is_shell_tool(name), "{name:?} must be checked");
    }
    for name in [
        "", "Read", "Write", "Edit", "Skill", "Task", "bash ", " bash", "bashx", "sh", "zsh",
    ] {
        assert!(!is_shell_tool(name), "{name:?} must not be checked");
    }
}

// ------------------------------------------------------------ deny shape

#[test]
fn deny_shape() {
    let out = check("tail -f /dev/null").expect("denied");
    let obj = out.as_object().expect("deny is a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "block",
            "decision",
            "hookSpecificOutput",
            "message",
            "permissionDecision",
            "permissionDecisionReason",
            "reason",
        ],
        "{out}"
    );
    assert_eq!(out["block"], true);
    assert_eq!(out["decision"], "block");
    assert_eq!(out["permissionDecision"], "deny");
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
    for reason in [
        &out["message"],
        &out["reason"],
        &out["permissionDecisionReason"],
        &out["hookSpecificOutput"]["permissionDecisionReason"],
    ] {
        assert_eq!(reason.as_str(), Some(EXPECTED_REASON), "{out}");
    }
    assert_eq!(BLOCK_MESSAGE, EXPECTED_REASON);
}

#[test]
fn deny_reason_is_constant_and_never_echoes_the_command() {
    let plain = check("tail -f /dev/null").expect("denied");
    let secret = check("GH_TOKEN=zq-secret-7f3a tail -f /dev/zero").expect("denied");
    assert_eq!(plain, secret, "the deny must not depend on the command");
    assert!(!secret.to_string().contains("zq-secret-7f3a"));
}

#[test]
fn every_some_is_deny() {
    let inputs = [
        "tail -f /dev/null",
        "tail -f some.log",
        "echo hi",
        "rm -rf /",
        "git commit --no-verify",
        "docker run img tail -f /dev/null",
        "sudo -u app tail -F /dev/zero",
        "",
    ];
    for cmd in inputs {
        if let Some(out) = check(cmd) {
            assert_eq!(out["permissionDecision"], "deny", "{cmd:?}: {out}");
            assert_eq!(
                out["hookSpecificOutput"]["permissionDecision"], "deny",
                "{cmd:?}: {out}"
            );
        }
    }
}

// ------------------------------------------------------------ robustness

#[test]
fn malformed_input_does_not_panic_and_is_not_matched() {
    for cmd in [
        "tail -f '/dev/null",
        "tail -f \"/dev/null",
        "tail -f /dev/null\\",
        "'",
        "\"",
        "\\",
        "<<",
        "<<EOF",
        "cat <<",
        "((((",
        "))))",
        "&&&&",
        ";;;;",
        "| | |",
        "tail -f $(",
        "tail -f `",
        "${",
        "--",
        "-",
        "tail",
        "tail -f",
        "tail -n",
        "timeout",
        "timeout -s",
        "sudo -u",
        "env -u",
        "nice -n",
        "tail -f /dev/null <<'EOF",
        "\"$(",
        "echo \"$(echo '",
        "echo \"$(echo \\",
        "echo \"$((((",
    ] {
        let _ = check(cmd);
    }
    // Unbalanced quotes mean "not matched" (fail open).
    assert!(!denied("tail -f '/dev/null"));
    assert!(!denied("echo 'tail -f /dev/null"));
    assert!(!denied("echo \"$(\ntail -f /dev/null"));
    assert!(!denied("echo \"$(echo '\ntail -f /dev/null"));
}

#[test]
fn non_ascii_input_does_not_panic() {
    for cmd in [
        "tail -f /dev/null # \u{1F600}",
        "echo 'é'; tail -f /dev/null",
        "\u{0}\u{7f}\u{feff}",
        "tail\u{a0}-f\u{a0}/dev/null",
        "ｔａｉｌ -f /dev/null",
    ] {
        let _ = check(cmd);
    }
    assert!(denied("echo 'é'; tail -f /dev/null"));
}

#[test]
fn every_two_character_input_is_safe() {
    let alphabet = [
        ' ', '\t', '\n', '\'', '"', '\\', ';', '&', '|', '(', ')', '<', '>', '-', '$', '`', '{',
        '}', '!', '#', 'f', 'é',
    ];
    for a in alphabet {
        for b in alphabet {
            let s: String = [a, b].iter().collect();
            let _ = check(&s);
            let _ = check(&format!("tail -f {s} /dev/null"));
            let _ = check(&format!("{s}tail -f /dev/null"));
        }
    }
}

#[test]
fn large_stress_inputs_finish_quickly() {
    let one_mib = 1024 * 1024;
    let inputs = [
        "<<a ".repeat(one_mib / 4),
        "'\"".repeat(one_mib / 2),
        "\\".repeat(one_mib),
        "(".repeat(one_mib),
        format!("{} tail -f /dev/null", "env ".repeat(one_mib / 4)),
        format!("tail {} /dev/null", "-f ".repeat(one_mib / 3)),
    ];
    let start = std::time::Instant::now();
    for input in &inputs {
        let _ = check(input);
    }
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "tokenising must be linear; took {:?}",
        start.elapsed()
    );
}

#[test]
fn guard_source_reads_no_environment() {
    let src = include_str!("../tail_guard.rs");
    for needle in ["std::env", "env::var", "var_os(", "env!("] {
        assert!(
            !src.contains(needle),
            "tail_guard.rs must not read the environment ({needle})"
        );
    }
    assert!(
        src.contains("TODO(#1735)"),
        "tail_guard.rs must carry the TODO to switch to shell::tokenise"
    );
}

// ------------------------------------------------------- hook integration

fn process(tool_name: &str, tool_input: Value) -> Value {
    // Other pre_tool_use tests change the environment (merge guard, XPIA);
    // hold the same lock so their settings cannot leak into these calls.
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let input = HookInput::PreToolUse {
        tool_name: tool_name.to_string(),
        tool_input,
        session_id: None,
    };
    PreToolUseHook.process(input).expect("process")
}

#[test]
fn process_denies_for_every_shell_tool_name() {
    for name in ["Bash", "bash", "shell", "exec_command", "shell_command"] {
        let out = process(name, json!({"command": "tail -f /dev/null"}));
        assert_eq!(
            out["hookSpecificOutput"]["permissionDecision"], "deny",
            "{name}: {out}"
        );
        assert_eq!(out["permissionDecisionReason"], EXPECTED_REASON, "{name}");
    }
}

#[test]
fn process_guard_runs_before_xpia() {
    // XPIA may also match this; the guard's deny shape must win.
    let out = process(
        "Bash",
        json!({"command": "tail -f /dev/null; rm -rf \"$SB\""}),
    );
    assert_eq!(
        out["hookSpecificOutput"]["permissionDecision"], "deny",
        "{out}"
    );
    assert_eq!(out["permissionDecisionReason"], EXPECTED_REASON, "{out}");
}

#[test]
fn process_leaves_other_tools_and_allowed_commands_alone() {
    let out = process("Read", json!({"command": "tail -f /dev/null"}));
    assert_ne!(out["permissionDecisionReason"], EXPECTED_REASON, "{out}");
    let out = process("Bash", json!({"command": "tail -n 5 /dev/null"}));
    assert_ne!(out["permissionDecisionReason"], EXPECTED_REASON, "{out}");
}
