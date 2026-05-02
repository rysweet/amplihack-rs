//! crates/amplihack-cli/tests/remote_cli_contract.rs
//!
//! Issue #536 CLI contract for wiring `amplihack remote <subcommand>` to the
//! native Rust remote port.

use amplihack_cli::Cli;
use clap::{ArgMatches, CommandFactory};

fn parse_remote_leaf(args: &[&str]) -> (String, ArgMatches) {
    let matches = Cli::command()
        .try_get_matches_from(args)
        .unwrap_or_else(|e| panic!("failed to parse {args:?}: {e}"));
    let ("remote", remote) = matches.subcommand().expect("top-level subcommand") else {
        panic!("expected top-level `remote` subcommand for {args:?}");
    };
    let (name, leaf) = remote
        .subcommand()
        .unwrap_or_else(|| panic!("expected remote leaf subcommand for {args:?}"));
    (name.to_string(), leaf.clone())
}

fn first_raw(matches: &ArgMatches, id: &str) -> Option<String> {
    matches.get_raw(id).and_then(|mut values| {
        values
            .next()
            .map(|value| value.to_string_lossy().to_string())
    })
}

fn all_raw(matches: &ArgMatches, id: &str) -> Vec<String> {
    matches
        .get_raw(id)
        .map(|values| {
            values
                .map(|value| value.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn remote_help_exposes_python_equivalent_command_set() {
    let mut command = Cli::command();
    let remote = command
        .find_subcommand_mut("remote")
        .expect("top-level `amplihack remote` command must exist");

    let subcommands: Vec<_> = remote
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_string())
        .collect();
    for expected in ["exec", "list", "start", "output", "kill", "status"] {
        assert!(
            subcommands.iter().any(|actual| actual == expected),
            "`amplihack remote` must expose `{expected}`; actual subcommands: {subcommands:?}"
        );
    }

    let mut help = Vec::new();
    remote.write_long_help(&mut help).unwrap();
    let rendered = String::from_utf8(help).unwrap();
    for expected in [
        "exec",
        "list",
        "start",
        "output",
        "kill",
        "status",
        "ANTHROPIC_API_KEY",
    ] {
        assert!(
            rendered.contains(expected),
            "`amplihack remote --help` should document `{expected}`.\nHelp:\n{rendered}"
        );
    }
}

#[test]
fn remote_exec_flags_defaults_and_trailing_azlin_args_match_python_cli() {
    let (name, defaults) = parse_remote_leaf(&["amplihack", "remote", "exec", "auto", "ship it"]);
    assert_eq!(name, "exec");
    assert_eq!(first_raw(&defaults, "command").as_deref(), Some("auto"));
    assert_eq!(first_raw(&defaults, "prompt").as_deref(), Some("ship it"));
    assert_eq!(first_raw(&defaults, "max_turns").as_deref(), Some("10"));
    assert_eq!(
        first_raw(&defaults, "vm_size").as_deref(),
        Some("Standard_D2s_v3")
    );
    assert_eq!(first_raw(&defaults, "timeout").as_deref(), Some("120"));
    assert!(!defaults.get_flag("keep_vm"));
    assert!(!defaults.get_flag("no_reuse"));

    let (_, full) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "exec",
        "fix",
        "repair the failing test",
        "--max-turns",
        "12",
        "--vm-size",
        "Standard_E16s_v5",
        "--vm-name",
        "amplihack-existing",
        "--keep-vm",
        "--no-reuse",
        "--timeout",
        "90",
        "--region",
        "westus2",
        "--port",
        "2222",
        "--",
        "--spot",
        "--tag",
        "owner=amplihack",
    ]);
    assert_eq!(first_raw(&full, "command").as_deref(), Some("fix"));
    assert_eq!(first_raw(&full, "max_turns").as_deref(), Some("12"));
    assert_eq!(
        first_raw(&full, "vm_size").as_deref(),
        Some("Standard_E16s_v5")
    );
    assert_eq!(
        first_raw(&full, "vm_name").as_deref(),
        Some("amplihack-existing")
    );
    assert!(full.get_flag("keep_vm"));
    assert!(full.get_flag("no_reuse"));
    assert_eq!(first_raw(&full, "timeout").as_deref(), Some("90"));
    assert_eq!(first_raw(&full, "region").as_deref(), Some("westus2"));
    assert_eq!(first_raw(&full, "port").as_deref(), Some("2222"));
    let debug = format!("{full:?}");
    assert!(
        debug.contains("--spot") && debug.contains("owner=amplihack"),
        "exec must preserve trailing azlin args after `--`; matches: {debug}"
    );
}

#[test]
fn remote_start_flags_defaults_and_prompt_list_match_python_cli() {
    let (name, defaults) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "start",
        "first prompt",
        "second prompt",
    ]);
    assert_eq!(name, "start");
    assert_eq!(
        all_raw(&defaults, "prompts"),
        ["first prompt", "second prompt"]
    );
    assert_eq!(first_raw(&defaults, "command").as_deref(), Some("auto"));
    assert_eq!(first_raw(&defaults, "max_turns").as_deref(), Some("10"));
    assert_eq!(first_raw(&defaults, "size").as_deref(), Some("l"));
    assert_eq!(first_raw(&defaults, "region").as_deref(), Some("eastus"));

    let (_, full) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "start",
        "--command",
        "analyze",
        "--max-turns",
        "4",
        "--size",
        "xl",
        "--region",
        "centralus",
        "--port",
        "2022",
        "investigate issue #536",
    ]);
    assert_eq!(all_raw(&full, "prompts"), ["investigate issue #536"]);
    assert_eq!(first_raw(&full, "command").as_deref(), Some("analyze"));
    assert_eq!(first_raw(&full, "max_turns").as_deref(), Some("4"));
    assert_eq!(first_raw(&full, "size").as_deref(), Some("xl"));
    assert_eq!(first_raw(&full, "region").as_deref(), Some("centralus"));
    assert_eq!(first_raw(&full, "port").as_deref(), Some("2022"));
}

#[test]
fn remote_list_output_kill_and_status_flags_match_python_cli() {
    let (name, list) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "list",
        "--status",
        "running",
        "--json",
    ]);
    assert_eq!(name, "list");
    assert_eq!(first_raw(&list, "status").as_deref(), Some("running"));
    assert!(list.get_flag("json"));

    let (name, output) = parse_remote_leaf(&["amplihack", "remote", "output", "sess-1"]);
    assert_eq!(name, "output");
    assert_eq!(first_raw(&output, "session_id").as_deref(), Some("sess-1"));
    assert_eq!(first_raw(&output, "lines").as_deref(), Some("100"));
    assert!(!output.get_flag("follow"));

    let (_, output_follow) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "output",
        "sess-1",
        "--lines",
        "250",
        "--follow",
    ]);
    assert_eq!(first_raw(&output_follow, "lines").as_deref(), Some("250"));
    assert!(output_follow.get_flag("follow"));

    let (name, kill) = parse_remote_leaf(&["amplihack", "remote", "kill", "sess-1", "--force"]);
    assert_eq!(name, "kill");
    assert_eq!(first_raw(&kill, "session_id").as_deref(), Some("sess-1"));
    assert!(kill.get_flag("force"));

    let (name, status) = parse_remote_leaf(&["amplihack", "remote", "status", "--json"]);
    assert_eq!(name, "status");
    assert!(status.get_flag("json"));
}

#[test]
fn remote_cli_rejects_invalid_python_contract_values() {
    for args in [
        &["amplihack", "remote", "exec", "deploy", "prompt"][..],
        &["amplihack", "remote", "list", "--status", "paused"][..],
        &[
            "amplihack",
            "remote",
            "start",
            "--command",
            "deploy",
            "prompt",
        ][..],
        &["amplihack", "remote", "start", "--size", "xxl", "prompt"][..],
        &["amplihack", "remote", "output", "sess-1", "--lines", "0"][..],
    ] {
        let parsed = Cli::command().try_get_matches_from(args);
        assert!(
            parsed.is_err(),
            "invalid remote CLI invocation should be rejected: {args:?}"
        );
    }
}
