use crate::Cli;
use clap::{CommandFactory, Parser};

fn long_help_for(path: &[&str]) -> String {
    let mut command = Cli::command();
    let mut current = &mut command;

    for name in path {
        current = current
            .find_subcommand_mut(name)
            .unwrap_or_else(|| panic!("expected `{}` subcommand in `{path:?}`", name));
    }

    let mut help = Vec::new();
    current.write_long_help(&mut help).unwrap();
    String::from_utf8(help).unwrap()
}

#[test]
fn remote_help_exposes_only_python_parity_subcommands() {
    let help = long_help_for(&["remote"]);

    for subcommand in ["exec", "list", "start", "output", "kill", "status"] {
        assert!(
            help.contains(subcommand),
            "remote help should list `{subcommand}`"
        );
    }

    assert!(
        !help.contains("prime"),
        "`remote prime` is stale documentation behavior and must not be exposed"
    );
}

#[test]
fn remote_exec_help_and_parser_match_python_flags() {
    let help = long_help_for(&["remote", "exec"]);
    for expected in [
        "--max-turns",
        "--vm-size",
        "--vm-name",
        "--keep-vm",
        "--no-reuse",
        "--timeout",
        "--region",
        "--port",
    ] {
        assert!(
            help.contains(expected),
            "exec help should include {expected}"
        );
    }

    Cli::try_parse_from([
        "amplihack",
        "remote",
        "exec",
        "ultrathink",
        "analyze issue #536",
        "--max-turns",
        "30",
        "--vm-size",
        "Standard_D4s_v5",
        "--vm-name",
        "amplihack-debug",
        "--keep-vm",
        "--no-reuse",
        "--timeout",
        "240",
        "--region",
        "eastus",
        "--port",
        "5000",
        "--",
        "--subscription",
        "Engineering",
    ])
    .expect("remote exec should parse Python-compatible flags and trailing azlin args");

    for invalid in [
        vec!["amplihack", "remote", "exec", "prime", "prompt"],
        vec![
            "amplihack",
            "remote",
            "exec",
            "auto",
            "prompt",
            "--max-turns",
            "0",
        ],
        vec![
            "amplihack",
            "remote",
            "exec",
            "auto",
            "prompt",
            "--max-turns",
            "51",
        ],
        vec![
            "amplihack",
            "remote",
            "exec",
            "auto",
            "prompt",
            "--timeout",
            "4",
        ],
        vec![
            "amplihack",
            "remote",
            "exec",
            "auto",
            "prompt",
            "--timeout",
            "481",
        ],
    ] {
        assert!(
            Cli::try_parse_from(invalid.clone()).is_err(),
            "{invalid:?} should be rejected by the CLI parser"
        );
    }
}

#[test]
fn remote_start_help_and_parser_match_python_flags() {
    let help = long_help_for(&["remote", "start"]);
    for expected in ["--command", "--max-turns", "--size", "--region", "--port"] {
        assert!(
            help.contains(expected),
            "start help should include {expected}"
        );
    }

    Cli::try_parse_from([
        "amplihack",
        "remote",
        "start",
        "--command",
        "fix",
        "--max-turns",
        "25",
        "--size",
        "xl",
        "--region",
        "westus3",
        "--port",
        "5001",
        "fix install smoke",
        "delete remote python files",
    ])
    .expect("remote start should parse multiple prompts and Python-compatible flags");

    assert!(
        Cli::try_parse_from(["amplihack", "remote", "start"]).is_err(),
        "remote start requires at least one prompt"
    );
    assert!(
        Cli::try_parse_from([
            "amplihack",
            "remote",
            "start",
            "--command",
            "prime",
            "prompt"
        ])
        .is_err(),
        "remote start command mode must match the Python choices"
    );
    assert!(
        Cli::try_parse_from(["amplihack", "remote", "start", "--size", "xxl", "prompt"]).is_err(),
        "remote start size must be one of s, m, l, xl"
    );
}

#[test]
fn remote_session_management_help_and_parser_match_python_flags() {
    let list_help = long_help_for(&["remote", "list"]);
    assert!(list_help.contains("--status"));
    assert!(list_help.contains("--json"));

    let output_help = long_help_for(&["remote", "output"]);
    assert!(output_help.contains("--lines"));
    assert!(output_help.contains("--follow"));

    let kill_help = long_help_for(&["remote", "kill"]);
    assert!(kill_help.contains("--force"));

    let status_help = long_help_for(&["remote", "status"]);
    assert!(status_help.contains("--json"));

    Cli::try_parse_from([
        "amplihack",
        "remote",
        "list",
        "--status",
        "running",
        "--json",
    ])
    .expect("remote list should parse status filter and JSON output");
    Cli::try_parse_from([
        "amplihack",
        "remote",
        "output",
        "sess-20260502-203014-4f2a",
        "--lines",
        "200",
        "--follow",
    ])
    .expect("remote output should parse lines and follow");
    Cli::try_parse_from([
        "amplihack",
        "remote",
        "kill",
        "sess-20260502-203014-4f2a",
        "--force",
    ])
    .expect("remote kill should parse force");
    Cli::try_parse_from(["amplihack", "remote", "status", "--json"])
        .expect("remote status should parse JSON output");

    assert!(
        Cli::try_parse_from(["amplihack", "remote", "list", "--status", "stale"]).is_err(),
        "remote list status must match the Python lifecycle statuses"
    );
}
