//! Issue #1733: the hook denies a tmux command that would kill a whole server,
//! or every session on it, unless the invocation names a private socket with
//! `-S` or `-L`. Commands that only mention the forms as data pass.

use super::*;
use serde_json::{Value, json};

fn denied(command: &str) -> bool {
    check(command).is_some()
}

/// The exact command from the incident (2026-10-28 15:18:39 PDT).
const INCIDENT: &str = "TMUX_TMPDIR=$SB/tmuxsock tmux kill-server";

const DENIED: &[&str] = &[
    INCIDENT,
    "tmux kill-server",
    "TMUX_TMPDIR=/tmp/sb tmux kill-server",
    // Setting TMUX is never scope.
    "TMUX=/tmp/x,1,0 tmux kill-server",
    "env TMUX=/tmp/x,1,0 tmux kill-server",
    "env -u TMUX tmux kill-server",
    // kill-server and its unambiguous prefixes.
    "tmux kill-s",
    "tmux kill-se",
    "tmux kill-ser",
    "tmux kill-serv",
    // kill-session -a and its prefixes, getopt-style flags `aCt:`.
    "tmux kill-session -a",
    "tmux kill-ses -a",
    "tmux kill-sess -a",
    "tmux kill-session -aC",
    "tmux kill-session -Ca",
    "tmux kill-session -at x",
    "tmux kill-session -a -t x",
    "tmux kill-session -C -a",
    "tmux kill-session -t x -a",
    // An unknown kill-session flag is uncertain, so it denies.
    "tmux kill-session -Z",
    "tmux kill-session -t x -Q",
    // Global options: unscoped forms.
    "tmux -2 kill-server",
    "tmux -uv kill-server",
    "tmux -f /dev/null kill-server",
    "tmux -c 'ls' kill-server",
    "tmux -T 256 kill-server",
    "tmux -- kill-server",
    // -d and -h are not tmux global options: they end the options unscoped.
    "tmux -d kill-server",
    "tmux -h kill-server",
    "tmux -x -S /tmp/tx1/a.sock kill-server",
    "tmux -d -S /tmp/tx1/a.sock kill-server",
    // -S/-L after the subcommand is not scope.
    "tmux kill-server -S /tmp/tx1/a.sock",
    "tmux kill-server -L qa",
    // Scope that names the user's own server.
    "tmux -L default kill-server",
    "tmux -Ldefault kill-server",
    "tmux -L '' kill-server",
    "tmux -S '' kill-server",
    "tmux -S \"$TMUX\" kill-server",
    "tmux -S $TMUX kill-server",
    "tmux -S ${TMUX} kill-server",
    "tmux -S \"${TMUX%%,*}\" kill-server",
    "tmux -S ${TMUX:-/tmp/x} kill-server",
    "tmux -S ${TMUX#x} kill-server",
    "tmux -S $TMUX/.. kill-server",
    "tmux -S /tmp/tmux-1000/default kill-server",
    "tmux -S/tmp/tmux-1000/default kill-server",
    "tmux -S /home/u/sock/default kill-server",
    "tmux -S /tmp/tmux-1000/default kill-session -a",
    // tmux command sequences.
    "tmux new -d 'sleep 1' \\; kill-server",
    "tmux new -d 'sleep 1' ';' kill-server",
    "tmux new -d sleep\\; kill-server",
    "tmux ls \\; kill-session -a",
    // Program word after wrappers, paths and case.
    "\\tmux kill-server",
    "command tmux kill-server",
    "exec tmux kill-server",
    "nice tmux kill-server",
    "nice -n 5 tmux kill-server",
    "nohup tmux kill-server",
    "timeout 5 tmux kill-server",
    "stdbuf -oL tmux kill-server",
    "time tmux kill-server",
    "sudo tmux kill-server",
    "/usr/bin/tmux kill-server",
    "Tmux kill-server",
    "TMUX kill-server",
    // Separators, grouping and continuations.
    "cd /tmp && tmux kill-server",
    "true; tmux kill-server",
    "false || tmux kill-server",
    "ls | tmux kill-server",
    "tmux kill-server &",
    "echo start\ntmux kill-server",
    "tmux \\\nkill-server",
    "tmux kill-\\\nserver",
    "{ tmux kill-server; }",
    "(tmux kill-server)",
    "if true; then tmux kill-server; fi",
    "tmux -S /tmp/tx1/a.sock ls; tmux kill-server",
    "tmux -S /tmp/tx1/a.sock kill-server; TMUX_TMPDIR=/x tmux kill-server",
    // Quoted subcommand words.
    "tmux 'kill-server'",
    "tmux \"kill-server\"",
    "tmux kill-\"server\"",
    "tmux $'kill-server'",
    // L2: a tmux word inside another command.
    "xargs tmux kill-server",
    "ssh host tmux kill-server",
    "setsid tmux kill-server",
    "watch tmux kill-server",
    "sudo -u ryan tmux kill-server",
    "find . -maxdepth 0 -exec tmux kill-server \\;",
    "parallel tmux kill-server ::: 1",
    // L3: tmux inside an embedded script.
    "bash -c 'tmux kill-server'",
    "bash -lc \"tmux kill-server\"",
    "sh -c \"TMUX_TMPDIR=/x tmux kill-server\"",
    "zsh -c 'cd /; tmux kill-session -a'",
    "bash -c \"bash -c 'tmux kill-server'\"",
    "python -c \"import os; os.system('tmux kill-server')\"",
    "python3 -c 'import subprocess; subprocess.run([\"tmux\", \"kill-server\"])'",
    "node -e \"require('child_process').execSync('tmux kill-server')\"",
    "echo \"$(tmux kill-server)\"",
    "echo $(tmux kill-server)",
    "echo `tmux kill-server`",
    "cat <(tmux kill-server)",
    "x=$(tmux kill-server)",
    "gh pr create --title t --body \"$(tmux kill-server)\"",
    "git commit -m \"$(tmux kill-server)\"",
    "git -c alias.x='!tmux kill-server' x",
    "git -c 'alias.x=!tmux kill-server' x",
    "eval \"tmux kill-server\"",
    "eval tmux kill-server",
    "echo tmux kill-server | sh",
    "echo tmux kill-server | bash",
    "printf 'tmux kill-server\\n' | bash -s",
    "echo 'tmux kill-server' | xargs -I{} sh -c {}",
    "env -S 'tmux kill-server'",
    "tmux new \"tmux kill-server\"",
    "tmux -S /tmp/tx1/a.sock new -d \"tmux kill-server\"",
    "docker exec c tmux kill-server",
    "kubectl exec pod -- tmux kill-server",
    // Heredocs fed to a shell.
    "bash <<EOF\ntmux kill-server\nEOF",
    "bash <<'EOF'\nTMUX_TMPDIR=$SB tmux kill-server\nEOF",
    "sh <<-EOF\n\ttmux kill-session -a\n\tEOF",
    "cat <<'EOF' | sh\ntmux kill-server\nEOF",
    "ssh host <<'EOF'\ntmux kill-server\nEOF",
    // Uncertain input fails closed.
    "$T kill-server",
    "${TMUX_BIN} kill-session -a",
    "\"$TMUX_BIN\" kill-server",
    "echo kill-server | tmux -C attach",
    "tmux kill-server 'unbalanced",
    "tmux kill-server \"unbalanced",
    "echo \"$(tmux kill-server\"",
    "echo $(tmux kill-server",
    "cat <<\ntmux kill-server",
    // Redirections are not tmux arguments (security review, finding 1).
    "tmux 2>/dev/null kill-server",
    "tmux >/dev/null kill-server",
    "tmux > /dev/null kill-server",
    "tmux </dev/null kill-server",
    "tmux 2>&1 kill-server",
    "tmux &>/dev/null kill-server",
    "tmux kill-server>/dev/null",
    "tmux kill-server</dev/null",
    "tmux >>log kill-server",
    "tmux 2>&- kill-server",
    "tmux kill-server >",
    // `$'…'` escapes are decoded (finding 2).
    "tmux $'\\x6bill-server'",
    "tmux $'kill-\\x73erver'",
    "tmux $'\\153ill-server'",
    "bash -c $'\\x74mux kill-server'",
    // git subcommands that run shell commands (finding 3).
    "git rebase -x 'tmux kill-server' HEAD~1",
    "git rebase --exec 'tmux kill-server' main",
    "git bisect run tmux kill-server",
    "git submodule foreach 'tmux kill-server'",
    "git -C repo submodule foreach tmux kill-server",
    "git difftool --extcmd='tmux kill-server'",
    "git difftool -x 'tmux kill-server'",
    // git and gh commands that store shell text and run it later on the line.
    "git config alias.k '!tmux kill-server' && git k",
    "git config core.pager 'tmux kill-server' && git log",
    "git config --global core.pager 'tmux kill-server'",
    "git clone --upload-pack='tmux kill-server #' host:repo",
    "gh alias set k --shell 'tmux kill-server' && gh k",
    // Environment assignments that git runs as commands.
    "GIT_EXTERNAL_DIFF='tmux kill-server' git diff",
    "GIT_SSH_COMMAND='tmux kill-server' git fetch",
    "GIT_EDITOR='tmux kill-server' git commit",
    "env GIT_EDITOR='tmux kill-server' git commit",
    // A heredoc written to a file and run elsewhere on the line (finding 4).
    "cat > k.sh <<'EOF'\ntmux kill-server\nEOF\nbash k.sh",
    "cat <<'EOF' > k.sh && sh k.sh\ntmux kill-session -a\nEOF",
    // Acceptance criteria and docs tables, verbatim (D2).
    "FOO=1 BAR=2 tmux kill-server",
    "tmux kill-session -a -t foo",
    "true && tmux kill-server",
    "echo x | tmux kill-server",
    "sh -c 'tmux kill-server'",
    "bash -c \"tmux kill-server\"",
    "python -c \"os.system('tmux kill-server')\"",
];

const ALLOWED: &[&str] = &[
    "",
    "   ",
    "ls -la",
    "cargo test -p amplihack-hooks tmux_guard -- kill_server",
    "./kill-server.sh",
    "echo kill-server",
    "pkill -f my-test-binary",
    // Scoped by -S or -L before the subcommand.
    "env -u TMUX tmux -S \"$SB/tmux.sock\" kill-server",
    "env -u TMUX tmux -S /tmp/tx1/a.sock kill-server",
    "tmux -S /tmp/tx1/a.sock kill-server",
    "tmux -S/tmp/tx1/a.sock kill-server",
    "tmux -L qa-1733 kill-server",
    "tmux -Lqa-1733 kill-server",
    "tmux -f /dev/null -S /tmp/tx1/a.sock kill-server",
    "tmux -2 -S /tmp/tx1/a.sock kill-server",
    "tmux -u -L qa kill-server",
    "tmux -S /tmp/tx1/a.sock kill-session -a",
    "tmux -L qa kill-ses -a",
    "tmux -S \"$TMUX_TMPDIR/a.sock\" kill-server",
    "tmux -S ${TMUX_TMPDIR}/a.sock kill-server",
    "TMUX_TMPDIR=/tmp/sb tmux -S /tmp/sb/s kill-server",
    "TMUX=/tmp/x,1,0 tmux -S /tmp/tx1/a.sock kill-server",
    "tmux -S /tmp/tx1/a.sock new -d 'sleep 1' \\; kill-server",
    "\\tmux -S /tmp/tx1/a.sock kill-server",
    "sudo tmux -L qa kill-server",
    // Subcommands that are not guarded, and kill-session without -a.
    "tmux ls",
    "tmux list-sessions",
    "tmux new -d -s qa 'sleep 1'",
    "tmux kill-session",
    "tmux kill-session -t qa-1733",
    "tmux kill-session -ta",
    "tmux kill-session -t a",
    "tmux kill-session -C -t x",
    "tmux kill-session -t x -- -a",
    "tmux kill-window -a",
    "tmux kill-pane -a",
    // Data-only commands mention the forms.
    "git commit -m \"add tmux kill-server guard\"",
    "git commit -m 'deny tmux kill-session -a without -S'",
    "gh pr create --title t --body \"Denies TMUX_TMPDIR=… tmux kill-server.\"",
    "gh issue create --body 'tmux kill-server killed the server'",
    "echo tmux kill-server",
    "echo \"tmux kill-server\" > notes.md",
    "printf '%s\\n' 'tmux kill-session -a'",
    "grep -rn 'tmux kill-server' .",
    "rg \"tmux kill-server\" docs",
    "cat notes.md | grep 'tmux kill-server' | wc -l",
    "jq -r '.cmd' log.json | grep 'tmux kill-server'",
    "test \"$x\" = 'tmux kill-server'",
    "cat <<'EOF' > notes.md\ntmux kill-server\nEOF",
    "cat <<EOF > notes.md\nTMUX_TMPDIR=$SB tmux kill-server\nEOF",
    // Embedded scripts whose tmux is scoped.
    "bash -c 'tmux -S /tmp/tx1/a.sock kill-server'",
    "sh -c \"env -u TMUX tmux -S /tmp/tx1/a.sock kill-server\"",
    "bash <<'EOF'\ntmux -S /tmp/tx1/a.sock kill-server\nEOF",
    "echo \"$(tmux -S /tmp/tx1/a.sock kill-server)\"",
    "ssh host tmux -S /tmp/tx1/a.sock kill-server",
    "xargs tmux -L qa kill-server",
    "tmux -S /tmp/tx1/a.sock new -d \"tmux -S /tmp/tx1/a.sock kill-server\"",
    // Redirections do not hide scope or turn data into code.
    "tmux ls >/dev/null 2>&1",
    "tmux -S /tmp/p kill-server 2>&1",
    "tmux -S /tmp/p kill-server &>/dev/null",
    "tmux 2>/dev/null -S /tmp/p kill-server",
    "git commit -m \"a > b; tmux kill-server\"",
    "echo x >> log; tmux ls",
    "diff <(tmux ls) <(echo)",
    "tmux $'-S' /tmp/p $'\\x6bill-server'",
    // git subcommands that run commands, with scoped or no tmux.
    "git rebase -x 'tmux -S /tmp/p kill-server' HEAD~1",
    "git rebase -i HEAD~3",
    "git log --grep 'rebase tmux kill-server'",
    "git clone host:repo",
    "gh alias list",
    // Acceptance criteria and docs tables, verbatim (D2).
    "tmux -L x kill-server",
    "tmux -S /p kill-server",
    "env -u TMUX tmux -S /tmp/x/sock kill-server",
    "tmux kill-session -t foo",
    "grep kill-server file",
];

#[test]
fn denies_every_unscoped_or_uncertain_form() {
    let missed: Vec<&&str> = DENIED.iter().filter(|c| !denied(c)).collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_scoped_and_data_only_forms() {
    let wrong: Vec<&&str> = ALLOWED.iter().filter(|c| denied(c)).collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

#[test]
fn denies_the_exact_incident_command() {
    assert!(denied(INCIDENT));
}

#[test]
fn a_tmux_prefix_assignment_is_not_scope() {
    assert!(denied("TMUX=/tmp/tx1/a.sock,1,0 tmux kill-server"));
}

/// V3: scope copied from `$TMUX` through a variable is the user's own
/// server. Fails closed: a `-S` value that is a plain variable expansion
/// whose source the guard cannot see is not concrete scope.
#[test]
fn scope_copied_from_tmux_through_a_variable_is_not_scope() {
    assert!(denied("S=$TMUX; tmux -S \"$S\" kill-server"));
    assert!(denied("s=${TMUX%%,*}; tmux -S \"$s\" kill-server"));
    assert!(denied("a=$TMUX; b=${a%%,*}; tmux -S \"$b\" kill-server"));
    assert!(denied("export S=\"$TMUX\"; tmux -S \"${S}\" kill-server"));
    assert!(denied("S=$TMUX; bash -c \"tmux -S $S kill-server\""));
    assert!(denied("bash -c 'S=$TMUX; tmux -L \"$S\" kill-server'"));
    // A variable not taken from $TMUX is still scope.
    assert!(!denied(
        "SB=$(mktemp -d); tmux -S \"$SB/tmux.sock\" kill-server"
    ));
    assert!(!denied("S=$TMUX_TMPDIR/x; tmux -S \"$S\" kill-server"));
}

#[test]
fn a_long_chain_of_assignments_is_checked_quickly() {
    let mut line = String::from("v0=$TMUX");
    for n in 1..4000 {
        line.push_str(&format!("; v{n}=$v{}", n - 1));
    }
    line.push_str("; tmux -S \"$v3999\" kill-server");
    assert!(line.len() < 64 * 1024);
    let started = std::time::Instant::now();
    assert!(denied(&line));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn naming_the_default_socket_by_name_is_not_scope() {
    assert!(denied("tmux -L default kill-server"));
}

#[test]
fn a_commit_message_about_the_guard_is_data() {
    assert!(!denied("git commit -m \"add tmux kill-server guard\""));
}

#[test]
fn deny_response_is_understood_by_every_host() {
    let response = check(INCIDENT).expect("incident command is denied");
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
fn block_message_carries_the_required_guidance() {
    for needle in [
        "-S <private socket>",
        "env -u TMUX",
        "TMUX_TMPDIR alone does not redirect tmux while $TMUX is set",
    ] {
        assert!(BLOCK_MESSAGE.contains(needle), "missing {needle:?}");
    }
}

#[test]
fn deny_response_never_echoes_the_command() {
    let marker = "zq-secret-7f3a";
    let response = check(&format!("GH_TOKEN={marker} tmux kill-server")).expect("denied");
    assert!(!response.to_string().contains(marker), "{response}");
}

#[test]
fn tool_input_reads_command_or_cmd() {
    assert!(check_tool_input(&json!({"command": INCIDENT})).is_some());
    assert!(check_tool_input(&json!({"cmd": INCIDENT})).is_some());
    assert!(check_tool_input(&json!({"command": "ls", "cmd": INCIDENT})).is_some());
    assert!(check_tool_input(&json!({"command": "tmux -L qa kill-server"})).is_none());
}

#[test]
fn tool_input_argv_array_is_quoted_and_joined() {
    assert!(check_tool_input(&json!({"command": ["bash", "-lc", INCIDENT]})).is_some());
    assert!(check_tool_input(&json!({"command": ["tmux", "kill-server"]})).is_some());
    assert!(check_tool_input(&json!({"cmd": ["tmux", "kill-session", "-a"]})).is_some());
    assert!(
        check_tool_input(
            &json!({"command": ["bash", "-lc", "tmux -S /tmp/tx1/a.sock kill-server"]})
        )
        .is_none()
    );
    // Quoting keeps an argument one word: this is `echo` printing data.
    assert!(check_tool_input(&json!({"command": ["echo", "tmux kill-server"]})).is_none());
}

#[test]
fn tool_input_array_with_a_non_string_element_gets_the_text_test() {
    assert!(check_tool_input(&json!({"command": ["tmux", 5, "kill-server"]})).is_some());
    assert!(check_tool_input(&json!({"command": ["bash", null, "-c", INCIDENT]})).is_some());
    assert!(check_tool_input(&json!({"command": ["ls", 5]})).is_none());
}

#[test]
fn tool_input_of_any_other_shape_is_a_no_op() {
    for input in [
        json!({}),
        json!(null),
        json!("tmux kill-server"),
        json!({"command": null}),
        json!({"command": 5}),
        json!({"command": {"x": "tmux kill-server"}}),
        json!({"command": []}),
        json!({"path": "tmux kill-server"}),
    ] {
        assert!(check_tool_input(&input).is_none(), "{input}");
    }
}

#[test]
fn input_over_the_size_limit_gets_only_the_text_test() {
    let pad = "x".repeat(70 * 1024);
    assert!(denied(&format!(
        "echo {pad}; tmux -S /tmp/tx1/a.sock kill-server"
    )));
    assert!(denied(&format!("echo {pad} 'tmux kill-server'")));
    assert!(!denied(&format!("echo {pad}; tmux ls")));
    assert!(!denied(&format!("echo {pad}; kill-server")));
}

#[test]
fn reaching_the_reparse_limit_denies() {
    let one = "$(tmux -S /tmp/tx1/a.sock kill-server)";
    let few = format!("echo {}", [one; 8].join(" "));
    assert!(!denied(&few), "a few scoped substitutions are allowed");
    let many = format!("echo {}", vec![one; 300].join(" "));
    assert!(many.len() < 64 * 1024);
    assert!(denied(&many), "300 re-parses must hit the cap and deny");
}

#[test]
fn reaching_the_depth_limit_denies() {
    let nest = |depth: usize| {
        format!(
            "echo {}tmux -S /tmp/tx1/a.sock kill-server{}",
            "$(".repeat(depth),
            ")".repeat(depth)
        )
    };
    assert!(
        !denied(&nest(2)),
        "shallow nesting of a scoped command is allowed"
    );
    assert!(denied(&nest(8)), "nesting past the depth limit denies");
}

/// The guard must never panic: the hook fails open on Claude Code and
/// Copilot, so a panic would allow the command.
#[test]
fn pathological_inputs_never_panic_and_finish_quickly() {
    let started = std::time::Instant::now();
    let mut inputs: Vec<String> = [
        "",
        "tmux",
        "tmux -S",
        "tmux -L",
        "tmux --",
        "tmux -",
        "tmux kill-session -t",
        "tmux \\;",
        "tmux ;",
        "<<",
        "<<-",
        "$",
        "$'",
        "$(",
        "`",
        "'",
        "\"",
        "\\",
        "${",
        "${TMUX",
        "é tmux ü",
        "$'\\x",
        "$'\\x' tmux kill-server",
        "tmux kill-server \u{0}",
        "\u{0}tmux\u{0}kill-server",
        "tmux 'kill-server",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    inputs.push(format!("{}tmux kill-server", "$(".repeat(1000)));
    inputs.push(format!("{}tmux kill-server", "\"$(".repeat(1000)));
    inputs.push(format!("{}tmux kill-server", "bash -c '".repeat(200)));
    inputs.push("tmux kill-server ".repeat(4000));
    inputs.push(format!("tmux {}kill-server", "-2".repeat(5000)));
    inputs.push("\\".repeat(10_000));
    inputs.push("<<A\n".repeat(2000));
    // Many commands and pipelines: per-pipeline facts must not be
    // recomputed per command (was quadratic, ~3 s in release).
    inputs.push("a;".repeat(32_000));
    inputs.push("tmux ls|".repeat(8_000));

    // Seeded generator over shell metacharacters and guard tokens.
    let atoms = [
        "tmux",
        "kill-server",
        "kill-s",
        "kill-session",
        "-a",
        "-S",
        "-L",
        "default",
        "$TMUX",
        "${TMUX",
        "TMUX=",
        "env",
        "-u",
        "bash",
        "-c",
        "'",
        "\"",
        "`",
        "$(",
        ")",
        "(",
        "{",
        "}",
        ";",
        "\\;",
        "&&",
        "||",
        "|",
        "&",
        "\n",
        "\\\n",
        "<<",
        "<<-",
        "EOF",
        "<<<",
        "$'",
        "\\",
        "#",
        " ",
        "é",
        "\u{0}",
        "git",
        "-m",
        "xargs",
        "<(",
    ];
    let mut state: u64 = 0x1733_2026_1028_1518;
    for _ in 0..3000 {
        let mut text = String::new();
        let len = (state >> 59) as usize + 1;
        for _ in 0..len * 2 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            text.push_str(atoms[(state >> 33) as usize % atoms.len()]);
            if state & 1 == 0 {
                text.push(' ');
            }
        }
        inputs.push(text);
    }

    for text in &inputs {
        let _ = check(text);
        let _ = check_tool_input(&json!({"command": text}));
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn deeply_nested_input_is_safe_on_a_small_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let text = format!(
                "echo {}tmux kill-server{}",
                "$(".repeat(1000),
                ")".repeat(1000)
            );
            check(&text).is_some()
        })
        .expect("spawn");
    assert_eq!(handle.join().ok(), Some(true), "panicked or allowed");
}

#[test]
fn guard_reads_no_environment() {
    // The response is a pure function of the input text.
    let a: Option<Value> = check(INCIDENT);
    let b: Option<Value> = check(INCIDENT);
    assert_eq!(a, b);
    assert!(a.is_some());
}
