//! Issue #1794: the hook denies `tail` with a follow option on a device that
//! never ends (`tail -f /dev/null` and friends), which agents use as a sleep
//! and which outlives the tool call as an orphan. Text that only mentions the
//! form, and `tail -f` on a real file, pass. Every limit fails open.

use super::*;
use serde_json::{Value, json};

fn denied(command: &str) -> bool {
    check(command).is_some()
}

/// Single-quotes `text` for a POSIX shell.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `text` wrapped in `levels` nested `bash -c` calls.
fn nest(text: &str, levels: usize) -> String {
    (0..levels).fold(text.to_string(), |inner, _| {
        format!("bash -c {}", sh_quote(&inner))
    })
}

/// Measures the CPU time of the calling thread, so the cost test does not
/// depend on the host's load (see the same timer in `tmux_guard_tests.rs`).
#[cfg(unix)]
fn thread_cpu_time() -> std::time::Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the call's duration.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    assert_eq!(rc, 0, "clock_gettime(CLOCK_THREAD_CPUTIME_ID) failed");
    std::time::Duration::new(
        u64::try_from(ts.tv_sec).unwrap_or_default(),
        u32::try_from(ts.tv_nsec).unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// Denied: an executed tail with a follow option on a never-ending device.
// ---------------------------------------------------------------------------

const DENIED: &[&str] = &[
    // The forms seen in the incident.
    "tail -f /dev/null",
    "timeout 600 tail -f /dev/null",
    "timeout --foreground 600 tail -f /dev/null",
    "timeout --kill-after=60 600 tail -f /dev/null",
    // Every follow option.
    "tail -F /dev/null",
    "tail --follow /dev/null",
    "tail --follow=name /dev/null",
    "tail --follow=descriptor /dev/null",
    // `tail` accepts any unique prefix of a long option.
    "tail --f /dev/null",
    "tail --fo /dev/null",
    "tail --foll=name /dev/null",
    "timeout 5 tail --fo /dev/zero",
    // Short clusters, read left to right up to an option that takes a value.
    "tail -fn0 /dev/null",
    "tail -qf /dev/null",
    "tail -Fq /dev/null",
    // Old-style numeric cluster: `-5f` is "last 5 lines, follow".
    "tail -5f /dev/null",
    // Options that take a value, joined or as the next word.
    "tail -n 0 -f /dev/null",
    "tail -n0 -f /dev/null",
    "tail -c 10 -f /dev/null",
    "tail -s 1 -f /dev/null",
    "tail --lines 5 -f /dev/null",
    "tail --lines=5 --follow /dev/null",
    "tail --bytes 5 -f /dev/null",
    "tail --sleep-interval 1 -f /dev/null",
    "tail --max-unchanged-stats 5 -F /dev/null",
    // `--pid` is still denied (D6).
    "tail --pid 4242 -f /dev/null",
    "tail --pid=4242 -f /dev/null",
    "tail -f --pid=$$ /dev/null",
    // `--` ends options; the operand after it is still a file.
    "tail -f -- /dev/null",
    // Options after the operand, as GNU and uutils accept.
    "tail /dev/null -f",
    // Any operand that is a device, among real files.
    "tail -f build.log /dev/null",
    // Every never-ending device.
    "tail -f /dev/zero",
    "tail -f /dev/random",
    "tail -f /dev/urandom",
    "tail -f /dev/full",
    "tail -f /dev/stdin",
    "tail -f /dev/tty",
    "tail -f /dev/fd/0",
    "tail -f /proc/self/fd/0",
    // Repeated `/` and `/./` are collapsed.
    "tail -f //dev//null",
    "tail -f /dev/./null",
    "tail -f /./dev/null",
    // Quoting and paths to the program.
    "/usr/bin/tail -f /dev/null",
    "\"tail\" -f \"/dev/null\"",
    "t'ai'l -f '/dev/null'",
    // Wrappers that `shell::program` already strips.
    "nohup tail -f /dev/null &",
    "env FOO=1 tail -f /dev/null",
    "FOO=1 tail -f /dev/null",
    "exec tail -f /dev/null",
    "nice -n 10 tail -f /dev/null",
    "command tail -f /dev/null",
    "time tail -f /dev/null",
    // Wrappers the guard strips itself (D8).
    "setsid tail -f /dev/null",
    "setsid -w tail -f /dev/null",
    "setsid -f tail -f /dev/null",
    "setsid -c tail -f /dev/null",
    "setsid -w timeout 600 tail -f /dev/null",
    "timeout 600 setsid -w tail -f /dev/null",
    "stdbuf -oL tail -f /dev/null",
    "stdbuf -o L tail -f /dev/null",
    "stdbuf -i0 -o0 -e0 tail -f /dev/null",
    "stdbuf -i 0 -o L -e 0 tail -f /dev/null",
    "setsid stdbuf -oL tail -f /dev/null",
    "stdbuf -oL setsid -w tail -f /dev/null",
    // Launch wrappers that set scheduling or buffering, which the guard
    // strips itself.
    "ionice tail -f /dev/null",
    "ionice -c3 tail -f /dev/null",
    "ionice -c 3 tail -f /dev/null",
    "ionice -c2 -n7 tail -f /dev/null",
    "ionice --class=idle tail -f /dev/null",
    "ionice --class idle -t tail -f /dev/null",
    "nice -n 15 ionice -c3 tail -f /dev/null",
    "chrt -i 0 tail -f /dev/null",
    "chrt --idle 0 tail -f /dev/null",
    "chrt -b tail -f /dev/null",
    "chrt -f 10 tail -f /dev/null",
    "chrt -d -T 1000 -P 2000 -D 2000 0 tail -f /dev/null",
    "taskset 1 tail -f /dev/null",
    "taskset 0x3 tail -f /dev/null",
    "taskset -c 0-3 tail -f /dev/null",
    "taskset -a -c 0,1 tail -f /dev/null",
    "unbuffer tail -f /dev/null",
    "unbuffer -p tail -f /dev/null",
    "setsid -w ionice -c3 chrt -i 0 taskset 1 unbuffer tail -f /dev/null",
    // Any simple command in a list or pipeline.
    "echo start; tail -f /dev/null",
    "ls && tail -f /dev/null",
    "false || tail -f /dev/null",
    "tail -f /dev/null 2>&1 &",
    "tail -f /dev/null > /dev/null 2>&1 & echo started",
    "echo x | tail -f /dev/null",
    "cd /tmp\ntail -f /dev/null",
    // A shell's `-c` operand is parsed again (D9).
    "bash -c 'tail -f /dev/null'",
    "bash -lc 'tail -f /dev/null'",
    "bash -c -- 'tail -f /dev/null'",
    "bash -e -c 'tail -f /dev/null'",
    "bash --norc -c 'tail -f /dev/null'",
    // `-o NAME` alone or ending a cluster: the name is not a script.
    "bash -o pipefail -c 'tail -f /dev/null'",
    "bash -euo pipefail -c 'tail -f /dev/null'",
    "bash +eo pipefail -c 'tail -f /dev/null'",
    "bash -O extglob -c 'tail -f /dev/null'",
    "bash -co pipefail 'tail -f /dev/null'",
    // `--rcfile F` and `--init-file F`: the file is not a script.
    "bash --rcfile F -c 'tail -f /dev/null'",
    "bash --init-file F -c 'tail -f /dev/null'",
    "bash -c 'tail -f /dev/null' arg0",
    "sh -c 'tail -f /dev/null'",
    "zsh -c 'tail -f /dev/null'",
    "dash -c 'tail -f /dev/null'",
    "ksh -c 'tail -f /dev/null'",
    "/bin/bash -c 'tail -f /dev/null'",
    "timeout 600 bash -c 'tail -f /dev/null'",
    "setsid -w bash -lc 'timeout 600 tail -f /dev/null'",
    "bash -c \"echo hi; tail -f /dev/null\"",
    // A heredoc fed to a shell is executed (D10).
    "bash <<'EOF'\ntail -f /dev/null\nEOF",
    "sh <<EOF\necho hi\ntail -f /dev/null\nEOF",
    "bash -s arg <<'EOF'\ntail -f /dev/null\nEOF",
    // A command substitution runs its body: `$(…)` and backticks, bare, in
    // double quotes, nested, and inside a `-c` operand.
    "x=$(timeout 5 tail -f /dev/null)",
    "x=$(tail -f /dev/null)",
    "echo $(tail -f /dev/null)",
    "echo \"$(tail -f /dev/null)\"",
    "echo `tail -f /dev/null`",
    "echo \"`timeout 5 tail -f /dev/null`\"",
    "echo $(echo $(tail -f /dev/null))",
    "bash -c 'x=$(tail -f /dev/null)'",
    "bash -c \"x=\\$(tail -f /dev/null)\"",
    "echo 'quoted' $(tail -f /dev/null)",
    // A function body, in both definition forms.
    "f() { tail -f /dev/null; }; f",
    "function f { tail -f /dev/null; }",
    "function f() { tail -f /dev/null; }",
    "function f\n{\ntail -f /dev/null\n}",
    // Process substitution, which the shell does not wait for at all.
    "cat <(tail -f /dev/null)",
    "echo x > >(timeout 5 tail -f /dev/null)",
];

#[test]
fn denies_every_executed_follow_of_a_device() {
    for command in DENIED {
        assert!(denied(command), "not denied: {command:?}");
    }
}

// ---------------------------------------------------------------------------
// Allowed: data, real files, containers, and the documented limits.
// ---------------------------------------------------------------------------

const ALLOWED: &[&str] = &[
    // `tail -f` on a real file, or without a follow option.
    "tail -f build.log",
    "tail -F /var/log/syslog",
    "tail -f build.log | grep --line-buffered error",
    "tail -n 20 /dev/null",
    "tail /dev/null",
    "tail -c 0 /dev/zero",
    "tail --lines=5 /dev/null",
    // Launch wrappers around a harmless `tail`, or acting on a running
    // process (`-p`), where no command follows.
    "ionice -c3 tail -f build.log",
    "ionice -c3 tail -n 5 /dev/null",
    "ionice -p 1234",
    "chrt -i 0 tail -f build.log",
    "chrt -p 1234",
    "taskset 1 tail -f build.log",
    "taskset -p 1 1234",
    "unbuffer tail -f build.log",
    // A shortened `--follow` on a real file.
    "tail --f app.log",
    // No file operand, or `-` (standard input as a pipe).
    "tail -f",
    "tail -f -",
    // `-n5f`: `5f` is the value of `-n`, not a follow flag.
    "tail -n5f /dev/null",
    "tail -n 5f /dev/null",
    // `+5f` is a file operand, not an option cluster.
    "tail +5f /dev/null",
    "tail -f +5f",
    // After `--`, `-f` is a file name.
    "tail -- -f /dev/null",
    // Names that are not the devices.
    "tail -f /dev/nullx",
    "tail -f /dev/null.log",
    "tail -f dev/null",
    "tail -f /tmp/dev/null",
    "tail -f /dev/fd/1",
    // A different program, or tail only as data.
    "head -f /dev/null",
    "echo tail -f /dev/null",
    "echo 'tail -f /dev/null'",
    "grep -r 'tail -f /dev/null' docs",
    "git commit -m \"never use tail -f /dev/null as a sleep\"",
    "printf '%s\\n' 'timeout 600 tail -f /dev/null'",
    "bash -c 'echo tail -f /dev/null'",
    "function f { echo tail -f /dev/null; }",
    "function f",
    // Substitutions that run something else, or that the shell does not run.
    "echo \"$(date)\"",
    "echo `date`",
    "x=$(cat build.log)",
    "echo '$(tail -f /dev/null)'",
    "echo '`tail -f /dev/null`'",
    "echo $'$(tail -f /dev/null)'",
    "echo \\`tail -f /dev/null\\`",
    "true # $(tail -f /dev/null)",
    "git commit -m \"$(cat <<'EOF'\nfix: never use `tail -f /dev/null` or $(tail -f /dev/null)\nEOF\n)\"",
    "cat > notes.md <<'EOF'\nx=$(tail -f /dev/null)\nEOF",
    // Heredocs that are written, not executed.
    "cat > Dockerfile <<'EOF'\nCMD [\"tail\", \"-f\", \"/dev/null\"]\nEOF",
    "cat > entry.sh <<'EOF'\ntail -f /dev/null\nEOF",
    "cat > compose.yaml <<EOF\ncommand: tail -f /dev/null\nEOF",
    "bash -c \"cat > f <<'EOF'\ntail -f /dev/null\nEOF\"",
    // Container entry points: `tail` is not the program word (D11).
    "docker run -d --rm alpine tail -f /dev/null",
    "docker exec c tail -f /dev/null",
    "podman run -d alpine tail -f /dev/null",
    "kubectl exec pod -- tail -f /dev/null",
    // The supported ways to wait.
    "amplihack wait --seconds 600",
    "sleep 5",
    "cargo test & pid=$!; wait $pid",
    "timeout --foreground 600 cargo test",
    // Documented limits (D7, D11, F4): not matched.
    "tail -f $DEV",
    "tail -f \"$DEV\"",
    "tail -f $(echo /dev/null)",
    // A here-string or a pipe into a shell: the script is data until it
    // runs, and following it would need data-flow tracking.
    "bash <<< 'tail -f /dev/null'",
    "echo 'tail -f /dev/null' | bash",
    "tail -f /dev/nu*",
    "tail -f /dev/../dev/null",
    "echo /dev/null | xargs tail -f",
    "find /dev -name null -exec tail -f {} \\;",
    "eval 'tail -f /dev/null'",
    "env -S 'tail -f /dev/null'",
    "busybox tail -f /dev/null",
    "bash wait.sh",
    "sh ./wait.sh",
    // A heredoc fed to a script file is the script's input, not bash's.
    "bash run.sh <<'EOF'\ntail -f /dev/null\nEOF",
    "bash -euo pipefail run.sh <<'EOF'\ntail -f /dev/null\nEOF",
    // A shell with no `-c` operand and no heredoc.
    "bash",
    "bash -c",
    "bash -l",
];

#[test]
fn allows_data_real_files_containers_and_documented_limits() {
    for command in ALLOWED {
        assert!(!denied(command), "wrongly denied: {command:?}");
    }
}

#[test]
fn input_with_multibyte_characters_never_panics() {
    for command in [
        "tail -ñf /dev/null",
        "tail -fé /dev/null",
        "tail --follöw /dev/null",
        "tail -f /dev/nüll",
        "stdbuf -ö tail -f /dev/null",
        "setsid -é tail -f /dev/null",
        "bash -ç 'tail -f /dev/null'",
        "tail -n é -f /dev/null",
        "tail -",
        "tail --",
        "tail -n",
        "tail --lines",
        "stdbuf -o",
        "setsid",
        "ionice -c",
        "ionice -",
        "ionice --class=",
        "chrt -T",
        "chrt -é tail -f /dev/null",
        "taskset",
        "taskset -c",
        "unbuffer -p",
        "",
        "\u{0}",
    ] {
        let _ = check(command);
    }
    assert!(!denied("tail -f /dev/nüll"));
}

// ---------------------------------------------------------------------------
// Nesting and limits. Every limit fails open (returns None).
// ---------------------------------------------------------------------------

#[test]
fn nesting_up_to_three_shells_is_checked_and_a_fourth_is_allowed() {
    // `depth >= MAX_DEPTH` stops the scan, with the top level at depth 0.
    assert_eq!(MAX_DEPTH, 4);
    for levels in 1..MAX_DEPTH {
        let command = nest("tail -f /dev/null", levels);
        assert!(denied(&command), "{levels} nested shells: {command:?}");
    }
    for levels in [MAX_DEPTH, MAX_DEPTH + 1] {
        let command = nest("tail -f /dev/null", levels);
        assert!(!denied(&command), "{levels} nested shells: {command:?}");
    }
}

#[test]
fn a_heredoc_inside_a_shell_counts_towards_the_depth() {
    let inner = "bash <<'EOF'\ntail -f /dev/null\nEOF";
    assert!(denied(&nest(inner, 1)));
    assert!(!denied(&nest(inner, MAX_DEPTH)));
}

#[test]
fn string_input_over_the_size_limit_fails_open() {
    assert_eq!(MAX_INPUT, 64 * 1024);
    let head = "tail -f /dev/null; : ";
    let at_limit = format!("{head}{}", "x".repeat(MAX_INPUT - head.len()));
    assert_eq!(at_limit.len(), MAX_INPUT);
    assert!(denied(&at_limit), "input of exactly MAX_INPUT is checked");
    let over = format!("{at_limit}x");
    assert!(!denied(&over), "input over MAX_INPUT fails open");
    assert!(check_tool_input(&json!({"command": over})).is_none());
}

#[test]
fn joined_argv_over_the_size_limit_fails_open() {
    let pad = "x".repeat(MAX_INPUT);
    let input = json!({"command": ["bash", "-c", "tail -f /dev/null", pad]});
    assert!(check_tool_input(&input).is_none());
    let small = json!({"command": ["bash", "-c", "tail -f /dev/null", "x"]});
    assert!(check_tool_input(&small).is_some());
}

#[test]
fn an_incomplete_parse_fails_open() {
    for command in [
        "tail -f /dev/null 'unclosed",
        "tail -f /dev/null \"unclosed",
        "tail -f /dev/null $(unclosed",
        "tail -f /dev/null `unclosed",
        "bash <<EOF\ntail -f /dev/null",
        "tail -f /dev/null <<",
        "tail -f /dev/null >",
    ] {
        assert!(!denied(command), "incomplete parse must allow: {command:?}");
    }
}

#[test]
fn an_exhausted_reparse_budget_fails_open() {
    assert_eq!(MAX_REPARSES, 256);
    let mut command = "bash -c true; ".repeat(MAX_REPARSES + 8);
    command.push_str("bash -c 'tail -f /dev/null'");
    assert!(command.len() < MAX_INPUT);
    assert!(!denied(&command), "the budget is shared across the command");
    // Below the budget the same shape is still checked.
    let mut command = "bash -c true; ".repeat(8);
    command.push_str("bash -c 'tail -f /dev/null'");
    assert!(denied(&command));
}

#[test]
fn the_unwrap_limit_fails_open() {
    assert_eq!(MAX_UNWRAP, 8);
    assert!(denied("setsid setsid tail -f /dev/null"));
    assert!(denied(
        "stdbuf -oL setsid stdbuf -eL setsid -w tail -f /dev/null"
    ));
    let many = format!("{}tail -f /dev/null", "setsid ".repeat(MAX_UNWRAP + 4));
    assert!(!denied(&many), "past MAX_UNWRAP rounds the guard allows");
}

#[cfg(unix)]
#[test]
fn hostile_input_just_under_the_size_limit_finishes_quickly() {
    let mut command = String::new();
    let nested = nest("bash -c true", 3);
    let heredoc = "bash <<'EOF'\nbash -c 'bash -c true'\nEOF\n";
    while command.len() + nested.len() + heredoc.len() + 4 < MAX_INPUT {
        command.push_str(&nested);
        command.push_str("; ");
        command.push_str(heredoc);
    }
    assert!(command.len() <= MAX_INPUT);
    let deep = format!("{}tail -f /dev/null", "setsid stdbuf -oL ".repeat(3000));
    let deep = &deep[..deep.len().min(MAX_INPUT)];
    let start = thread_cpu_time();
    let _ = check(&command);
    let _ = check(deep);
    let spent = thread_cpu_time().saturating_sub(start);
    assert!(
        spent < std::time::Duration::from_secs(5),
        "took {spent:?} of CPU"
    );
}

// ---------------------------------------------------------------------------
// Tool input.
// ---------------------------------------------------------------------------

#[test]
fn tool_input_reads_command_or_cmd() {
    assert!(check_tool_input(&json!({"command": "tail -f /dev/null"})).is_some());
    assert!(check_tool_input(&json!({"cmd": "tail -f /dev/null"})).is_some());
    assert!(check_tool_input(&json!({"command": "ls", "cmd": "tail -f /dev/null"})).is_some());
    assert!(check_tool_input(&json!({"command": "tail -f build.log"})).is_none());
}

#[test]
fn tool_input_argv_array_is_quoted_and_joined() {
    for argv in [
        json!(["tail", "-f", "/dev/null"]),
        json!(["bash", "-lc", "tail -f /dev/null"]),
        json!(["setsid", "-w", "timeout", "600", "tail", "-f", "/dev/null"]),
    ] {
        assert!(
            check_tool_input(&json!({"command": argv})).is_some(),
            "{argv}"
        );
    }
    // Quoting keeps an argument one word: this is `echo` printing data.
    assert!(check_tool_input(&json!({"command": ["echo", "tail -f /dev/null"]})).is_none());
    assert!(check_tool_input(&json!({"command": ["tail", "-f", "build.log"]})).is_none());
}

#[test]
fn tool_input_array_with_a_non_string_element_is_allowed() {
    for input in [
        json!({"command": ["bash", 1]}),
        json!({"command": ["tail", "-f", null, "/dev/null"]}),
        json!({"command": ["bash", "-c", "tail -f /dev/null", 5]}),
        json!({"cmd": [{"x": 1}, "tail", "-f", "/dev/null"]}),
    ] {
        assert!(check_tool_input(&input).is_none(), "{input}");
    }
}

#[test]
fn tool_input_of_any_other_shape_is_a_no_op() {
    for input in [
        json!({}),
        json!(null),
        json!("tail -f /dev/null"),
        json!({"command": null}),
        json!({"command": 5}),
        json!({"command": {"x": "tail -f /dev/null"}}),
        json!({"command": []}),
        json!({"path": "tail -f /dev/null"}),
    ] {
        assert!(check_tool_input(&input).is_none(), "{input}");
    }
}

// ---------------------------------------------------------------------------
// The response.
// ---------------------------------------------------------------------------

/// `value` with every string equal to `message` replaced by a placeholder.
fn with_message_masked(value: &Value, message: &str) -> Value {
    match value {
        Value::String(s) if s == message => Value::String("<message>".into()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), with_message_masked(v, message)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[test]
fn deny_response_has_the_tmux_guard_shape_key_for_key() {
    let ours = check("tail -f /dev/null").expect("denied");
    let tmux = super::super::tmux_guard::check("tmux kill-server").expect("tmux denies");
    let tmux_message = tmux["message"].as_str().expect("tmux message").to_string();
    assert_eq!(
        with_message_masked(&ours, BLOCK_MESSAGE),
        with_message_masked(&tmux, &tmux_message),
    );
}

#[test]
fn deny_response_is_an_enforced_deny() {
    let response = check("tail -f /dev/null").expect("denied");
    assert_eq!(response["block"], true);
    assert_eq!(response["decision"], "block");
    assert_eq!(response["permissionDecision"], "deny");
    assert_eq!(
        response["hookSpecificOutput"]["hookEventName"],
        "PreToolUse"
    );
    assert_eq!(response["hookSpecificOutput"]["permissionDecision"], "deny");
    for reason in [
        &response["message"],
        &response["reason"],
        &response["permissionDecisionReason"],
        &response["hookSpecificOutput"]["permissionDecisionReason"],
    ] {
        assert_eq!(reason.as_str(), Some(BLOCK_MESSAGE));
    }
}

#[test]
fn block_message_names_the_supported_ways_to_wait() {
    for needle in [
        "amplihack wait --seconds",
        "wait $pid",
        "pid=$!",
        "tail -f /dev/null",
        "timeout N tail",
    ] {
        assert!(BLOCK_MESSAGE.contains(needle), "missing {needle:?}");
    }
}

#[test]
fn deny_response_never_echoes_the_command() {
    let marker = "zq-marker-1794-b6e1";
    for command in [
        format!("tail -f {marker}.log /dev/null"),
        format!("TOKEN={marker} tail -f /dev/null"),
        format!("bash -c 'cd /{marker} && tail -f /dev/null'"),
    ] {
        let response = check(&command).expect("denied");
        assert!(!response.to_string().contains(marker), "{response}");
        assert!(!response.to_string().contains("build.log"));
    }
}

/// No environment variable, flag or config turns the guard off, because the
/// module reads no environment at all.
#[test]
fn guard_source_reads_no_environment() {
    let source = include_str!("../tail_follow_guard.rs");
    for needle in [
        "std::env",
        "env::var",
        "env!(",
        "var_os",
        "getenv",
        "std::fs",
        "canonicalize",
        "SystemTime",
        "Instant",
    ] {
        assert!(
            !source.contains(needle),
            "tail_follow_guard.rs contains {needle:?}"
        );
    }
}

#[test]
fn guard_source_cites_the_issue_in_its_warning() {
    let source = include_str!("../tail_follow_guard.rs");
    assert!(source.contains("warn!("), "the deny logs one warn! line");
    assert!(source.contains("#1794"));
}
