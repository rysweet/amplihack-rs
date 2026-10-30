//! Issue #1733: the hook denies a tmux command that would kill a whole server,
//! or every session on it, unless the invocation names a private socket with
//! `-S` or `-L`. Commands that only mention the forms as data pass.

use super::*;
use serde_json::json;

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
    // stdbuf is not a wrapper: layer 2 finds the tmux word after it.
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
    // Acceptance criteria literals (D5).
    "env FOO=1 tmux kill-server",
    "echo | tmux kill-server",
    "echo\ntmux kill-server",
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
    // Acceptance criteria literals (D5): scoped kill-session -a.
    "tmux -L x kill-session -a",
    "tmux -S /p kill-session -a",
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
        // A socket path read from tmux is the user's server (crusty round 1).
        "#{socket_path}",
        // The way out when the command only mentions the words as text.
        "--context-from-file",
        "--body-file",
        "its own tool call",
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

/// The guard always runs: nothing in the environment can turn it off or
/// change a verdict, because neither this module nor the tokeniser it uses
/// reads the environment. Running `check` twice in one process could not
/// show that, so this reads their source instead.
#[test]
fn guard_source_reads_no_environment() {
    for (name, source) in [
        ("tmux_guard.rs", include_str!("../tmux_guard.rs")),
        ("shell.rs", include_str!("../shell.rs")),
    ] {
        for needle in ["std::env", "env::var", "env!(", "var_os", "getenv"] {
            assert!(!source.contains(needle), "{name} contains {needle:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// F1 (SR-7): an unscoped tmux invocation whose subcommand word is an
// expansion is uncertain, because the expansion may name kill-server.
// ---------------------------------------------------------------------------

const EXPANDED_SUBCOMMAND_DENIED: &[&str] = &[
    "c=kill-server; tmux $c",
    "tmux $c",
    "tmux \"$c\"",
    "tmux ${C}",
    "tmux \"${C}\"",
    "tmux $(echo kill-server)",
    "tmux `echo kill-server`",
    "tmux kill-$x",
    "TMUX_TMPDIR=/tmp/sb tmux $c",
    "env -u TMUX tmux $c",
    "true && tmux $c",
    "bash -c 'tmux $c'",
    "bash -c 'tmux $(echo kill-server)'",
    "bash -c 'x=$(tmux $c)'",
    "bash <<'EOF'\ntmux $c\nEOF",
    "git rebase -x 'tmux $c' main",
    // Global options do not scope, so the subcommand is still an expansion.
    "tmux -2 $c",
    "tmux -f /dev/null \"$c\"",
    // After an unknown global option every later word is inspected.
    "tmux -x $c",
    "tmux -d ${C}",
];

const EXPANDED_SUBCOMMAND_ALLOWED: &[&str] = &[
    // Scoped invocations may use an expanded subcommand.
    "tmux -S /p $cmd",
    "tmux -L x \"$c\"",
    "env -u TMUX tmux -S \"$SB/tmux.sock\" $c",
    // Expansions in option values and arguments, after a literal subcommand.
    "tmux new -s \"$name\"",
    "tmux new -d -s \"$name\" 'sleep 1'",
    "tmux kill-session -t \"$s\"",
    "tmux send-keys -t \"$pane\" \"$keys\" Enter",
    "tmux ls",
    "tmux display -p '#{session_name}'",
    // An expansion that is not a tmux subcommand word.
    "echo $c",
    "x=$(tmux ls)",
    "bash -c 'tmux new -s \"$name\"'",
    "git commit -m \"tmux $c\"",
];

#[test]
fn denies_an_unscoped_tmux_whose_subcommand_is_an_expansion() {
    let missed: Vec<&&str> = EXPANDED_SUBCOMMAND_DENIED
        .iter()
        .filter(|c| !denied(c))
        .collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_expansions_in_scoped_invocations_and_option_values() {
    let wrong: Vec<&&str> = EXPANDED_SUBCOMMAND_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

// ---------------------------------------------------------------------------
// F4: PowerShell is a shell tool.
// ---------------------------------------------------------------------------

#[test]
fn powershell_is_a_shell_tool() {
    for name in ["powershell", "PowerShell", "POWERSHELL"] {
        assert!(is_shell_tool(name), "{name}");
    }
}

#[test]
fn shell_tool_names_are_matched_exactly() {
    for name in ["Read", "Write", "Edit", "pwsh-ish", "bashful", ""] {
        assert!(!is_shell_tool(name), "{name}");
    }
}

// ---------------------------------------------------------------------------
// S1: tmux commands that take another tmux command as an argument. Any later
// argument of an unscoped invocation that contains a kill command counts.
// ---------------------------------------------------------------------------

const NESTED_COMMAND_DENIED: &[&str] = &[
    "tmux if-shell true kill-server",
    "tmux if -F 1 kill-server",
    "tmux if-shell -b true \"kill-session -a\"",
    "tmux if-shell true kill-session -a",
    "tmux if-shell true \"if-shell true kill-server\"",
    "tmux run-shell -C kill-server",
    "tmux run -C 'kill-server'",
    "tmux confirm-before -y kill-server",
    "tmux bind k kill-server",
    "tmux bind-key -n F12 kill-server",
    "tmux set-hook -g client-attached kill-server",
    "tmux display-menu -T m quit q kill-server",
    "tmux command-prompt -I kill-server",
    "tmux set -g command-alias[100] zz=kill-server",
    "tmux new -d 'sleep 1' \\; if-shell true kill-server",
    "tmux -x if-shell true kill-server",
    // Accepted fail-closed deny: the keys are only typed, not run, but an
    // unscoped tmux with a kill command in any argument is not cleared.
    "tmux send-keys 'tmux kill-server'",
];

const NESTED_COMMAND_ALLOWED: &[&str] = &[
    "tmux -S /p if-shell true kill-server",
    "tmux -L qa run-shell -C kill-server",
    "tmux if-shell true 'display ok'",
    "tmux new -s kill-switch",
    "tmux kill-session -t foo",
    "tmux kill-session -t foo \\; kill-session -t bar",
    "tmux bind k kill-pane",
];

#[test]
fn denies_a_kill_command_nested_in_an_unscoped_tmux_argument() {
    let missed: Vec<&&str> = NESTED_COMMAND_DENIED
        .iter()
        .filter(|c| !denied(c))
        .collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_nested_tmux_commands_that_are_scoped_or_harmless() {
    let wrong: Vec<&&str> = NESTED_COMMAND_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

// ---------------------------------------------------------------------------
// S2: a substitution whose body mentions tmux and kill-s is cleared only when
// it holds a scoped tmux invocation, unless its output stays data.
// ---------------------------------------------------------------------------

const SUBSTITUTION_OUTPUT_DENIED: &[&str] = &[
    "bash -c \"$(echo tmux kill-server)\"",
    "bash -c \"$(echo 'tmux kill-server')\"",
    "sh -c \"`echo tmux kill-server`\"",
    "bash -c \"$(cat <<< 'tmux kill-server')\"",
    "$(echo tmux kill-server)",
    "`echo tmux kill-server`",
    "x=$(echo tmux kill-server); $x",
    "eval $(echo tmux kill-server)",
    "eval \"$(printf 'tmux kill-server')\"",
    "eval \"`echo tmux kill-server`\"",
    "env $(echo tmux kill-server)",
    "nohup $(echo tmux kill-server)",
    "$(bash -c 'echo \"tmux kill-server\"')",
    "echo \"$(echo tmux kill-server)\" | bash",
    // Accepted fail-closed deny: the output is only assigned, but nothing
    // here shows that it stays data.
    "x=$(echo \"notes on tmux kill-server\")",
];

const SUBSTITUTION_OUTPUT_ALLOWED: &[&str] = &[
    "bash -c \"$(echo hello)\"",
    "x=$(echo hi); $x",
    "echo \"tmux kill-server\"",
    "echo \"$(tmux -S /p kill-server)\"",
    "x=$(tmux ls)",
    "bash -c 'echo \"tmux kill-server\"'",
    // The output of a substitution in a data-only command stays data.
    "git commit -m \"$(echo 'deny tmux kill-server')\"",
    "git commit -m \"$(cat <<'EOF'\nfix: deny tmux kill-server\nEOF\n)\"",
    "gh pr create --title t --body \"$(cat <<'EOF'\nDenies tmux kill-session -a.\nEOF\n)\"",
];

#[test]
fn denies_a_substitution_whose_output_is_run() {
    let missed: Vec<&&str> = SUBSTITUTION_OUTPUT_DENIED
        .iter()
        .filter(|c| !denied(c))
        .collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_substitutions_that_are_scoped_or_stay_data() {
    let wrong: Vec<&&str> = SUBSTITUTION_OUTPUT_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 1, concern 1: inside a pane, `#{socket_path}` is the user's
// own socket, so an `-S`/`-L` value that substitutes tmux's output, or a
// variable assigned one, is not scope. The second command is the deny
// message's own `env -u TMUX` form with the socket taken from tmux.
// ---------------------------------------------------------------------------

const SOCKET_FROM_TMUX_DENIED: &[&str] = &[
    "tmux -S \"$(tmux display -p '#{socket_path}')\" kill-server",
    "sock=$(tmux display -p '#{socket_path}'); env -u TMUX tmux -S \"$sock\" kill-server",
    "tmux -S \"$(tmux display-message -p '#{socket_path}')\" kill-session -a",
    "env -u TMUX tmux -S \"$(tmux display -p '#{socket_path}')\" kill-server",
    "tmux -S \"`tmux display -p '#{socket_path}'`\" kill-server",
    "tmux -S\"$(tmux display -p '#{socket_path}')\" kill-server",
    "tmux -L \"$(tmux display -p '#{socket_path}')\" kill-server",
    "tmux -S \"$(/usr/bin/tmux display -p '#{socket_path}')\" kill-server",
    "tmux -S \"$(command tmux display -p '#{socket_path}')\" kill-server",
    "export sock=\"$(tmux display -p '#{socket_path}')\"; tmux -S \"$sock\" kill-server",
    "sock=$(tmux display -p '#{socket_path}'); s2=$sock; tmux -S \"$s2\" kill-server",
    "sock=`tmux display -p '#{socket_path}'`; tmux -S \"$sock\" kill-server",
    "bash -c 'sock=$(tmux display -p \"#{socket_path}\"); tmux -S \"$sock\" kill-server'",
];

const SOCKET_FROM_TMUX_ALLOWED: &[&str] = &[
    "tmux -S \"$SB/sock\" kill-server",
    "tmux -S \"$(mktemp -d)/s\" kill-server",
    "SB=$(mktemp -d); env -u TMUX tmux -S \"$SB/s\" kill-server",
    // A sandbox whose path mentions tmux is still a private socket.
    "SB=$(mktemp -d /tmp/tmux-qa.XXXXXX); env -u TMUX tmux -S \"$SB/s\" kill-server",
    "tmux -S \"$(mktemp -d -t tmux-qa.XXXX)/s\" kill-server",
    // Reading the socket path is fine; only a kill on it is denied.
    "tmux display -p '#{socket_path}'",
    "tmux -S \"$(tmux display -p '#{socket_path}')\" ls",
];

#[test]
fn denies_a_socket_path_taken_from_tmux() {
    let missed: Vec<&&str> = SOCKET_FROM_TMUX_DENIED
        .iter()
        .filter(|c| !denied(c))
        .collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_a_private_socket_made_by_a_substitution() {
    let wrong: Vec<&&str> = SOCKET_FROM_TMUX_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 1, concerns 3 and 4: amplihack's own tmux use stays allowed,
// including an inline task description that mentions the guard. The
// `-c`/`--context` values of `amplihack recipe run` are data, exactly as a
// `--context-from-file` file is.
// ---------------------------------------------------------------------------

const AMPLIHACK_OWN_TMUX_USE_ALLOWED: &[&str] = &[
    "amplihack recipe stop amp-default-workflow-repo-20261029-101500",
    "amplihack recipe stop 2f1c9a7e-run-id",
    "amplihack recipe run default-workflow --detach --context-from-file task_description=/home/u/.amplihack/work/task.md -c repo_path=.",
    "amplihack recipe run smart-orchestrator --detach -w /repo --context-from-file task_description=\"$TASK_FILE\" -c repo_path=.",
    // Inline task text that mentions the guard (the dev-orchestrator and
    // auto-drive-to-merge launch forms).
    "amplihack recipe run default-workflow --detach -c task_description=\"Follow-up to #1733: tmux kill-server guard false positives\"",
    "amplihack recipe run default-workflow -c task_description=\"make recipe stop use tmux kill-session -t =name\"",
    "amplihack recipe run default-workflow -c task_description='deny tmux kill-session -a without -S'",
    "cd /repo && AMPLIHACK_HOME=/x amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml -c task_description=\"deny tmux kill-server\" -c repo_path=\".\" --verbose",
    "amplihack recipe run auto-drive-to-merge -c task_description=\"tmux kill-server guard\" -c repo_path=/home/u/src/p",
    "amplihack recipe run default-workflow --context task_description='tmux kill-server guard'",
    "amplihack recipe run default-workflow --context=task_description='tmux kill-server guard'",
    "amplihack recipe run default-workflow -ctask_description='tmux kill-server guard'",
    // The hand-written detached form, with a task that does not mention the
    // guard.
    "tmux new-session -d -s run \"$(amplihack agent-binary --shell) amplihack recipe run default-workflow -c task_description='fix the parser'\"",
    "tmux new-session -d -s \"recipe-$(date +%s)\" \"cd /repo && $(amplihack agent-binary --shell -w /repo) amplihack recipe run smart-orchestrator -c task_description='TASK' -c repo_path='.'\"",
    // The exact-match target `recipe stop` and `--detach` use.
    "tmux kill-session -t '=amp-default-workflow-repo-20261029-101500'",
    "tmux kill-session -t =amp-x",
    "tmux kill-session -t \"=$session\"",
    "tmux kill-session -t=amp-x",
];

const AMPLIHACK_OWN_TMUX_USE_DENIED: &[&str] = &[
    // A substitution in a context value runs.
    "amplihack recipe run default-workflow -c task_description=\"$(tmux kill-server)\"",
    "amplihack recipe run default-workflow -c task_description=\"`tmux kill-server`\"",
    // Only context values are data: a positional argument or another
    // option's value is not.
    "amplihack recipe run default-workflow tmux kill-server",
    "amplihack recipe run default-workflow -c repo_path=. tmux kill-server",
    "amplihack recipe run default-workflow -w 'tmux kill-server'",
    // Only `recipe run` gets the exemption.
    "amplihack launch -- -p 'tmux kill-server'",
    // An evaluator anywhere on the line removes it, as for other data-only
    // commands: put the tmux command in its own tool call.
    "amplihack recipe run default-workflow -c task_description=\"tmux kill-server\"; tmux ls",
    "amplihack recipe run default-workflow -c task_description=\"tmux kill-server\" | bash",
    // A hand-written `tmux new-session` runs its argument in a shell, so
    // inline task text there is judged as a command. Use `--detach` or
    // `--context-from-file` for such a task.
    "tmux new-session -d -s run \"$(amplihack agent-binary --shell) amplihack recipe run default-workflow -c task_description='tmux kill-server guard'\"",
];

#[test]
fn allows_amplihack_own_tmux_use() {
    let wrong: Vec<&&str> = AMPLIHACK_OWN_TMUX_USE_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

#[test]
fn recipe_run_context_values_are_data_only_on_their_own() {
    let missed: Vec<&&str> = AMPLIHACK_OWN_TMUX_USE_DENIED
        .iter()
        .filter(|c| !denied(c))
        .collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 1, concern 6: an embedded script gets the same verdict as the
// command unwrapped. A targeted `kill-session -t` inside `bash -c` or
// `ssh host '…'` is allowed; any run of tokens that would kill still denies,
// so a tmux call the tokeniser cannot parse is not cleared.
// ---------------------------------------------------------------------------

const WRAPPED_TARGETED_KILL_ALLOWED: &[&str] = &[
    "bash -c \"tmux kill-session -t amp-x\"",
    "ssh vm1 \"tmux kill-session -t amp-x\"",
    "ssh vm1 'tmux kill-session -t =amp-x 2>/dev/null || true'",
    "bash -lc 'tmux kill-session -t a; tmux kill-session -t b'",
    "sh -c \"bash -c 'tmux kill-session -t amp-x'\"",
    "x=$(tmux kill-session -t foo)",
    "python3 -c \"import os; os.system('tmux kill-session -t amp-x')\"",
];

const WRAPPED_KILL_DENIED: &[&str] = &[
    "ssh vm1 \"tmux kill-session -a\"",
    "ssh vm1 'tmux kill-server'",
    "bash -c \"tmux kill-session -t x; tmux kill-server\"",
    "bash -c \"tmux kill-session -t x -a\"",
    // An unscoped tmux beside a kill token elsewhere in the script.
    "bash -c \"tmux kill-session -t x; echo kill-server\"",
    "bash -c \"tmux ls; printf 'kill-server'\"",
    "python -c \"import os; os.system('tmux kill-session -a')\"",
    "python3 -c 'import subprocess; subprocess.run([\"tmux\", \"kill-session\", \"-a\"])'",
    // The substitution's output may be run.
    "bash -c \"$(echo tmux kill-session -t x)\"",
];

#[test]
fn allows_a_wrapped_targeted_kill_session() {
    let wrong: Vec<&&str> = WRAPPED_TARGETED_KILL_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

#[test]
fn denies_a_wrapped_script_with_any_kill() {
    let missed: Vec<&&str> = WRAPPED_KILL_DENIED.iter().filter(|c| !denied(c)).collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 1, concern 7: a `tmux -C` client runs each line of its stdin,
// so a kill word anywhere in a piped word counts, after a space, a newline
// escape or a tab escape.
// ---------------------------------------------------------------------------

const CONTROL_MODE_DENIED: &[&str] = &[
    "printf 'list-sessions\\nkill-server\\n' | tmux -C attach",
    "echo ' kill-server' | tmux -C attach",
    "echo -e 'ls\\tkill-session -a' | tmux -C attach",
    "printf 'ls\\n' 'kill-server' | tmux -CC attach",
    "bash -c \"printf 'ls\\nkill-server' | tmux -C attach\"",
    // bash keeps the backslash of `\n` inside double quotes.
    "printf \"list-sessions\\nkill-server\\n\" | tmux -C attach",
    "bash -c 'printf \"ls\\nkill-server\" | tmux -C attach'",
];

const CONTROL_MODE_ALLOWED: &[&str] = &[
    "printf 'list-sessions\\n' | tmux -C attach",
    "echo skill-set | tmux -C attach",
    "printf 'ls\\n' | tmux -S /tmp/tx1/a.sock -C attach",
    "printf 'kill-server\\n' | tmux -S /tmp/tx1/a.sock -C attach",
];

#[test]
fn denies_a_kill_piped_to_an_unscoped_control_client() {
    let missed: Vec<&&str> = CONTROL_MODE_DENIED.iter().filter(|c| !denied(c)).collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_control_mode_without_a_kill_or_with_scope() {
    let wrong: Vec<&&str> = CONTROL_MODE_ALLOWED.iter().filter(|c| denied(c)).collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 1, concern 8: git is data-only only with a subcommand in
// GIT_DATA_SUBCOMMANDS. Any other subcommand's arguments are judged as
// scripts, which covers the `--upload-pack` family without listing it.
// ---------------------------------------------------------------------------

const GIT_NON_DATA_DENIED: &[&str] = &[
    "git fetch --upload-pack 'tmux kill-server #' /tmp/r",
    "git pull --upload-pack='tmux kill-server #' /tmp/r",
    "git ls-remote --upload-pack 'tmux kill-server #' /tmp/r",
    "git push --receive-pack='tmux kill-server #' /tmp/r",
    "git archive --remote=/tmp/r --exec='tmux kill-server #' HEAD",
    "git grep --open-files-in-pager='tmux kill-server #' needle",
    "git -C repo fetch --upload-pack 'tmux kill-session -a #' /tmp/r",
];

const GIT_DATA_ALLOWED: &[&str] = &[
    "git log --grep 'tmux kill-server'",
    "git --no-pager log --grep 'tmux kill-server'",
    "git -C repo commit -m \"deny tmux kill-server\"",
    "git tag -a v1 -m 'deny tmux kill-server'",
    "git notes add -m 'tmux kill-server guard'",
    "git stash push -m 'tmux kill-server notes'",
    "git diff -G 'tmux kill-server'",
    "git show --stat HEAD",
    "git add -A && git commit -m \"deny tmux kill-server\" && git push",
    "git push -u origin HEAD && gh pr create --title t --body \"Denies tmux kill-server.\"",
];

#[test]
fn denies_shell_text_in_a_non_data_git_subcommand() {
    let missed: Vec<&&str> = GIT_NON_DATA_DENIED.iter().filter(|c| !denied(c)).collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

#[test]
fn allows_mentions_in_data_git_subcommands() {
    let wrong: Vec<&&str> = GIT_DATA_ALLOWED.iter().filter(|c| denied(c)).collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 3, concern 2: stdin flows left to right through a pipeline,
// so only a `kill-s` word in an earlier stage is input to an unscoped tmux.
// Targeted kills fed by `tmux ls` stay allowed.
// ---------------------------------------------------------------------------

const PIPELINE_ORDER_ALLOWED: &[&str] = &[
    "tmux ls -F '#S' | grep '^amp-' | xargs -r -n1 tmux kill-session -t",
    "tmux list-sessions -F '#{session_name}' | grep '^amp-' | xargs -I{} tmux kill-session -t {}",
    "tmux ls 2>/dev/null | grep -c kill-session",
    "tmux -C attach | grep kill-server",
    "tmux ls -F '#S' | grep '^amp-' | while read -r s; do tmux kill-session -t \"$s\"; done",
    // The same targeted kills broken across lines after each `|`, and a
    // `kill-s` word in a pipeline that a newline has already ended.
    "tmux ls -F '#S' |\n  grep '^amp-' |\n  xargs -r -n1 tmux kill-session -t",
    "echo kill-server\ntmux -C attach </dev/null",
    "echo kill-server | cat\ntmux -C attach </dev/null",
];

const PIPELINE_ORDER_DENIED: &[&str] = &[
    "echo kill-server | tmux -C attach",
    "printf 'ls\\nkill-server\\n' | tmux -C attach",
    "printf 'kill-server' | xargs tmux",
    "echo kill-server | tmux -S /tmp/tx1/a.sock ls | tmux -C attach",
    "tmux ls | grep amp | xargs -n1 tmux kill-session -a -t",
    // A heredoc in the pipeline counts for every stage, whichever gets it.
    "cat <<END | tmux -C attach\nkill-server\nEND",
    "tmux -C attach <<END\nkill-server\nEND",
    "tmux -C attach <<< kill-server",
    // Crusty round 4: a pipeline broken across lines after `|` is still one
    // pipeline (POSIX `pipe_sequence '|' linebreak command`).
    "echo kill-server |\ntmux -C attach",
    "printf 'ls\\nkill-server\\n' |\n  tmux -C attach",
    "echo kill-server | # feed it\ntmux -C attach",
    "echo kill-server |&\ntmux -C attach",
    "echo kill-server |\n\n  # feed it\n\ntmux -C attach",
    "echo kill-server | \\\n\ntmux -C attach",
    "echo kill-server \\\n| tmux -C attach",
    "cat <<END |\nkill-server\nEND\ntmux -C attach",
];

#[test]
fn a_kill_word_after_an_unscoped_tmux_in_a_pipeline_is_not_its_input() {
    let wrong: Vec<&&str> = PIPELINE_ORDER_ALLOWED
        .iter()
        .filter(|c| denied(c))
        .collect();
    assert!(wrong.is_empty(), "wrongly denied: {wrong:#?}");
}

#[test]
fn a_kill_word_before_an_unscoped_tmux_in_a_pipeline_is_denied() {
    let missed: Vec<&&str> = PIPELINE_ORDER_DENIED
        .iter()
        .filter(|c| !denied(c))
        .collect();
    assert!(missed.is_empty(), "not denied: {missed:#?}");
}

// ---------------------------------------------------------------------------
// Crusty round 3, concern 3: each tmux invocation reads the rest of its
// command, so the number read per simple command is capped and the cost
// stays linear in the input.
// ---------------------------------------------------------------------------

#[test]
fn more_tmux_words_in_one_command_than_the_cap_deny() {
    let at_cap = "tmux ".repeat(MAX_TMUX_PER_COMMAND);
    assert!(
        !denied(at_cap.trim_end()),
        "{MAX_TMUX_PER_COMMAND} words denied"
    );
    let over = "tmux ".repeat(MAX_TMUX_PER_COMMAND + 1);
    assert!(denied(over.trim_end()), "over the cap allowed");
    // The cap is per simple command, not per line.
    let split = format!("{}; {}", at_cap.trim_end(), at_cap.trim_end());
    assert!(!denied(&split), "two commands at the cap denied");
}

#[test]
fn large_inputs_shaped_against_the_cost_bound_finish_quickly() {
    let started = std::time::Instant::now();
    let fill = MAX_INPUT - 1_000;
    let tmux64 = "tmux ".repeat(MAX_TMUX_PER_COMMAND);
    let cases = [
        // Was quadratic: 13,000 words took 29 s in a release build.
        "tmux ".repeat(fill / 5),
        format!("{tmux64}{}", "kill-session -t ".repeat(fill / 16)),
        format!("{tmux64}{}", "-2 ".repeat(fill / 3)),
        format!("{tmux64}{}", "if-shell x ".repeat(fill / 11)),
        format!("{tmux64}{}", "ls \\; ".repeat(fill / 6)),
        format!(
            "bash -c \"bash -c '{tmux64}{}kill-session -t a'\"",
            "-2 ".repeat(fill / 3)
        ),
        format!("{}tmux ;", "tmux ".repeat(MAX_TMUX_PER_COMMAND - 1)).repeat(fill / 322),
    ];
    for text in &cases {
        assert!(text.len() <= MAX_INPUT, "case is over MAX_INPUT");
        let _ = check(text);
    }
    // A release build takes about 0.2 s for the slowest of these; the bound
    // leaves room for a debug build on a loaded host.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "took {:?}",
        started.elapsed()
    );
}

/// The quadratic-free [`kills_in`] gives the same verdict as reading the
/// options after every `kill-session` token afresh.
#[test]
fn kills_in_matches_reading_every_kill_session_afresh() {
    fn reference(tokens: &[&str]) -> bool {
        tokens.iter().enumerate().any(|(j, token)| {
            kills_server(token)
                || (is_kill_session(token)
                    && kills_all_sessions(tokens.get(j + 1..).unwrap_or_default()))
        })
    }
    let atoms = [
        "kill-session",
        "kill-ses",
        "kill-server",
        "kill-s",
        "-t",
        "-a",
        "-C",
        "-at",
        "-ta",
        "-tx",
        "-z",
        "--",
        "-",
        "x",
    ];
    let mut state: u64 = 0x1733_0003;
    for _ in 0..20_000 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let len = (state >> 60) as usize + 1;
        let tokens: Vec<&str> = (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                atoms[(state >> 33) as usize % atoms.len()]
            })
            .collect();
        assert_eq!(kills_in(&tokens), reference(&tokens), "{tokens:?}");
    }
}
