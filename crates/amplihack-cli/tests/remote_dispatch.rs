//! crates/amplihack-cli/tests/remote_dispatch.rs
//!
//! Issue rysweet/Simard#1940 — Layer 3 parser-level dispatch tests for
//! `commands::remote`.
//!
//! Architectural restriction: we exercise the `clap` parser for every
//! `RemoteCommands` variant via `Cli::command().try_get_matches_from(...)`.
//! We DO NOT call `commands::remote::run(...)`, because that function calls
//! `tokio_runtime()?` and dispatches into SSH / Azure / process spawning.
//! Parser-level coverage is the correct outside-in surface here — it walks
//! every per-variant `#[arg(...)]` attribute (default values, value parsers,
//! range validators) and therefore lifts `src/commands/remote.rs`'s
//! enum-variant matching surface off zero while staying hermetic.
//!
//! Complementary to `tests/remote_cli_contract.rs`: that file freezes the
//! Python-CLI compatibility contract; this file is structured for coverage
//! breadth — every variant plus its negative-input rejection paths.

use amplihack_cli::Cli;
use clap::{ArgMatches, CommandFactory, error::ErrorKind};

fn parse(args: &[&str]) -> clap::error::Result<ArgMatches> {
    Cli::command().try_get_matches_from(args)
}

fn parse_remote_leaf(args: &[&str]) -> (String, ArgMatches) {
    let matches = parse(args).unwrap_or_else(|e| panic!("expected parse OK for {args:?}: {e}"));
    let ("remote", remote) = matches.subcommand().expect("top-level subcommand") else {
        panic!("expected top-level `remote` subcommand for {args:?}");
    };
    let (name, leaf) = remote
        .subcommand()
        .unwrap_or_else(|| panic!("expected remote leaf subcommand for {args:?}"));
    (name.to_string(), leaf.clone())
}

fn first(matches: &ArgMatches, id: &str) -> Option<String> {
    matches
        .get_raw(id)
        .and_then(|mut vals| vals.next().map(|v| v.to_string_lossy().to_string()))
}

fn all(matches: &ArgMatches, id: &str) -> Vec<String> {
    matches
        .get_raw(id)
        .map(|vals| vals.map(|v| v.to_string_lossy().to_string()).collect())
        .unwrap_or_default()
}

// ----- Exec -----------------------------------------------------------------

#[test]
fn remote_exec_minimal_parses_with_defaults() {
    let (name, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "exec",
        "auto",
        "fixture-prompt-exec-minimal",
    ]);
    assert_eq!(name, "exec");
    assert_eq!(first(&m, "command").as_deref(), Some("auto"));
    assert_eq!(
        first(&m, "prompt").as_deref(),
        Some("fixture-prompt-exec-minimal")
    );
    assert_eq!(first(&m, "max_turns").as_deref(), Some("10"));
    assert_eq!(first(&m, "vm_size").as_deref(), Some("Standard_D2s_v3"));
    assert_eq!(first(&m, "timeout").as_deref(), Some("120"));
    assert!(!m.get_flag("keep_vm"));
    assert!(!m.get_flag("no_reuse"));
}

#[test]
fn remote_exec_full_flag_surface_round_trips() {
    let (_, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "exec",
        "ultrathink",
        "fixture-prompt-exec-full",
        "--max-turns",
        "25",
        "--vm-size",
        "Standard_E16s_v5",
        "--vm-name",
        "fixture-vm-existing",
        "--keep-vm",
        "--no-reuse",
        "--timeout",
        "60",
        "--region",
        "westus2",
        "--port",
        "2222",
        "--",
        "--spot",
        "--tag",
        "owner=fixture",
    ]);
    assert_eq!(first(&m, "command").as_deref(), Some("ultrathink"));
    assert_eq!(first(&m, "max_turns").as_deref(), Some("25"));
    assert_eq!(first(&m, "vm_size").as_deref(), Some("Standard_E16s_v5"));
    assert_eq!(first(&m, "vm_name").as_deref(), Some("fixture-vm-existing"));
    assert!(m.get_flag("keep_vm"));
    assert!(m.get_flag("no_reuse"));
    assert_eq!(first(&m, "timeout").as_deref(), Some("60"));
    assert_eq!(first(&m, "region").as_deref(), Some("westus2"));
    assert_eq!(first(&m, "port").as_deref(), Some("2222"));
    let trailing = all(&m, "azlin_args");
    assert!(
        trailing.iter().any(|s| s == "--spot") && trailing.iter().any(|s| s == "owner=fixture"),
        "trailing azlin args should be preserved; got: {trailing:?}"
    );
}

#[test]
fn remote_exec_rejects_invalid_command_mode() {
    let err = parse(&["amplihack", "remote", "exec", "deploy", "prompt"])
        .expect_err("invalid command mode `deploy` must be rejected");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

#[test]
fn remote_exec_rejects_out_of_range_max_turns() {
    let err = parse(&[
        "amplihack",
        "remote",
        "exec",
        "auto",
        "prompt",
        "--max-turns",
        "0",
    ])
    .expect_err("max-turns=0 must violate range(1..=50)");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);

    let err = parse(&[
        "amplihack",
        "remote",
        "exec",
        "auto",
        "prompt",
        "--max-turns",
        "51",
    ])
    .expect_err("max-turns=51 must violate range(1..=50)");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

#[test]
fn remote_exec_rejects_out_of_range_timeout() {
    let err = parse(&[
        "amplihack",
        "remote",
        "exec",
        "auto",
        "prompt",
        "--timeout",
        "4",
    ])
    .expect_err("timeout=4 must violate range(5..=480)");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);

    let err = parse(&[
        "amplihack",
        "remote",
        "exec",
        "auto",
        "prompt",
        "--timeout",
        "481",
    ])
    .expect_err("timeout=481 must violate range(5..=480)");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

// ----- List -----------------------------------------------------------------

#[test]
fn remote_list_defaults_parses_with_no_filter() {
    let (name, m) = parse_remote_leaf(&["amplihack", "remote", "list"]);
    assert_eq!(name, "list");
    assert!(
        first(&m, "status").is_none(),
        "status filter defaults to None"
    );
    assert!(!m.get_flag("json"));
}

#[test]
fn remote_list_with_status_and_json_flag_parses() {
    let (_, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "list",
        "--status",
        "completed",
        "--json",
    ]);
    assert_eq!(first(&m, "status").as_deref(), Some("completed"));
    assert!(m.get_flag("json"));
}

#[test]
fn remote_list_rejects_unknown_status_value() {
    let err = parse(&["amplihack", "remote", "list", "--status", "paused"])
        .expect_err("status=paused must be rejected by value_parser");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

// ----- Start ----------------------------------------------------------------

#[test]
fn remote_start_with_multiple_prompts_parses() {
    let (name, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "start",
        "fixture-prompt-one",
        "fixture-prompt-two",
    ]);
    assert_eq!(name, "start");
    assert_eq!(
        all(&m, "prompts"),
        vec!["fixture-prompt-one", "fixture-prompt-two"]
    );
    assert_eq!(first(&m, "command").as_deref(), Some("auto"));
    assert_eq!(first(&m, "size").as_deref(), Some("l"));
    assert_eq!(first(&m, "region").as_deref(), Some("eastus"));
}

#[test]
fn remote_start_full_flags_parse() {
    let (_, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "start",
        "--command",
        "fix",
        "--max-turns",
        "8",
        "--size",
        "xl",
        "--region",
        "centralus",
        "--port",
        "9000",
        "fixture-prompt-start-full",
    ]);
    assert_eq!(first(&m, "command").as_deref(), Some("fix"));
    assert_eq!(first(&m, "max_turns").as_deref(), Some("8"));
    assert_eq!(first(&m, "size").as_deref(), Some("xl"));
    assert_eq!(first(&m, "region").as_deref(), Some("centralus"));
    assert_eq!(first(&m, "port").as_deref(), Some("9000"));
}

#[test]
fn remote_start_rejects_invalid_size_tier() {
    let err = parse(&[
        "amplihack",
        "remote",
        "start",
        "--size",
        "xxl",
        "fixture-prompt",
    ])
    .expect_err("size=xxl must be rejected (only s/m/l/xl allowed)");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

#[test]
fn remote_start_requires_at_least_one_prompt() {
    let err = parse(&["amplihack", "remote", "start"])
        .expect_err("start requires at least one prompt arg");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

// ----- Output ---------------------------------------------------------------

#[test]
fn remote_output_minimal_parses_with_default_lines() {
    let (name, m) = parse_remote_leaf(&["amplihack", "remote", "output", "fixture-sess-1"]);
    assert_eq!(name, "output");
    assert_eq!(first(&m, "session_id").as_deref(), Some("fixture-sess-1"));
    assert_eq!(first(&m, "lines").as_deref(), Some("100"));
    assert!(!m.get_flag("follow"));
}

#[test]
fn remote_output_with_lines_and_follow_parses() {
    let (_, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "output",
        "fixture-sess-2",
        "--lines",
        "500",
        "--follow",
    ]);
    assert_eq!(first(&m, "lines").as_deref(), Some("500"));
    assert!(m.get_flag("follow"));
}

#[test]
fn remote_output_rejects_zero_lines() {
    let err = parse(&[
        "amplihack",
        "remote",
        "output",
        "fixture-sess-3",
        "--lines",
        "0",
    ])
    .expect_err("lines=0 must violate range(1..)");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

// ----- Kill -----------------------------------------------------------------

#[test]
fn remote_kill_minimal_parses() {
    let (name, m) = parse_remote_leaf(&["amplihack", "remote", "kill", "fixture-sess-kill"]);
    assert_eq!(name, "kill");
    assert_eq!(
        first(&m, "session_id").as_deref(),
        Some("fixture-sess-kill")
    );
    assert!(!m.get_flag("force"));
}

#[test]
fn remote_kill_with_force_flag_parses() {
    let (_, m) = parse_remote_leaf(&[
        "amplihack",
        "remote",
        "kill",
        "fixture-sess-kill-force",
        "--force",
    ]);
    assert!(m.get_flag("force"));
}

#[test]
fn remote_kill_requires_session_id() {
    let err = parse(&["amplihack", "remote", "kill"]).expect_err("kill requires session_id");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}

// ----- Status ---------------------------------------------------------------

#[test]
fn remote_status_defaults_parse() {
    let (name, m) = parse_remote_leaf(&["amplihack", "remote", "status"]);
    assert_eq!(name, "status");
    assert!(!m.get_flag("json"));
}

#[test]
fn remote_status_with_json_flag_parses() {
    let (_, m) = parse_remote_leaf(&["amplihack", "remote", "status", "--json"]);
    assert!(m.get_flag("json"));
}

// ----- Cross-variant coverage -----------------------------------------------

#[test]
fn remote_help_documents_all_six_required_leaves() {
    let mut command = Cli::command();
    let remote = command
        .find_subcommand_mut("remote")
        .expect("`amplihack remote` must exist");

    let actual: Vec<String> = remote
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    for expected in ["exec", "list", "start", "output", "kill", "status"] {
        assert!(
            actual.iter().any(|name| name == expected),
            "`amplihack remote` must expose `{expected}`; got: {actual:?}"
        );
    }

    let mut buf = Vec::new();
    remote.write_long_help(&mut buf).expect("render help");
    let rendered = String::from_utf8(buf).unwrap();
    for marker in ["exec", "list", "start", "output", "kill", "status"] {
        assert!(
            rendered.contains(marker),
            "rendered `remote --help` should mention `{marker}`; got:\n{rendered}"
        );
    }
}

#[test]
fn remote_top_level_help_short_circuits_with_exit_zero() {
    let err = parse(&["amplihack", "remote", "--help"])
        .expect_err("--help must short-circuit with help Err");
    assert_eq!(err.kind(), ErrorKind::DisplayHelp);
    assert_eq!(err.exit_code(), 0);
}

/// Deliberately failing red-phase test (TDD step 7).
///
/// Locks down a contract: `remote exec` should reject empty-string prompts
/// at parse time, because empty prompts are operationally meaningless on
/// the remote side. This currently FAILS — clap accepts any string,
/// including "". Implementation in step 8 will add a non-empty value
/// parser or revise the test to match the agreed behavior.
#[test]
#[ignore = "red-phase: locks down non-empty-prompt contract; un-ignore once decision recorded"]
fn remote_exec_rejects_empty_prompt() {
    let err = parse(&["amplihack", "remote", "exec", "auto", ""])
        .expect_err("empty prompt must be rejected — see issue rysweet/Simard#1940");
    assert_ne!(err.kind(), ErrorKind::DisplayHelp);
}
