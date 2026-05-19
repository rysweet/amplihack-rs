//! crates/amplihack-cli/tests/cli_driver.rs
//!
//! Issue rysweet/Simard#1940 — Layer 1 outside-in `--help` harness.
//!
//! Goal: lift every dispatch subcommand in `crates/amplihack-cli/src/commands/*`
//! off 0% line coverage by instantiating the `clap` parser for each one and
//! asking it to render its help. Help-rendering walks the entire arg tree
//! (including type validators and default-value formatters), so it exercises
//! the per-subcommand `#[derive(Subcommand)]` arms in `cli_commands.rs` and
//! `cli_subcommands.rs` without any I/O, network, or process spawning.
//!
//! Architectural restriction: this file MUST stop at the `clap` layer.
//! It never reaches `tokio_runtime`, `ratatui`, SSH, or anything else that
//! would block on `--help`. All assertions run in-process via
//! `Cli::command().try_get_matches_from(...)`.

use amplihack_cli::Cli;
use clap::{CommandFactory, error::ErrorKind};

/// Top-level dispatch subcommands targeted by issue rysweet/Simard#1940.
///
/// Names match `Commands` variants in `src/cli_commands.rs` after `clap`'s
/// default kebab-case rename. The list intentionally mixes simple variants
/// (`builder`, `doctor`) with grouped variants (`remote`, `reflect`) and
/// renamed variants (`session-tree`) so the harness exercises both the
/// derived-name path and the explicit `#[command(name = "...")]` path.
const TOP_LEVEL_SUBCOMMANDS: &[&str] = &[
    "builder",
    "remote",
    "fleet",
    "reflect",
    "doctor",
    "session-tree",
    "multitask",
    "completions",
];

/// Asking `clap` for `--help` returns `Err(ErrorKind::DisplayHelp)` with
/// `exit_code() == 0`. Anything else (true parse error, missing subcommand,
/// non-zero exit code) is a regression.
fn assert_help_emitted(args: &[&str]) {
    let result = Cli::command().try_get_matches_from(args);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!(
            "`{}` was expected to short-circuit with help output, but parsed successfully",
            args.join(" ")
        ),
    };
    assert_eq!(
        err.kind(),
        ErrorKind::DisplayHelp,
        "`{}` should emit DisplayHelp; got {:?}\nRendered:\n{}",
        args.join(" "),
        err.kind(),
        err
    );
    assert_eq!(
        err.exit_code(),
        0,
        "`{}` --help must exit 0, not {}",
        args.join(" "),
        err.exit_code()
    );
}

#[test]
fn top_level_help_renders_without_panic() {
    let result = Cli::command().try_get_matches_from(["amplihack", "--help"]);
    let err = result.expect_err("--help must short-circuit with an Err carrying help output");
    assert_eq!(err.kind(), ErrorKind::DisplayHelp);
    assert_eq!(err.exit_code(), 0);
    let rendered = err.to_string();
    assert!(
        rendered.contains("amplihack"),
        "top-level help should mention the binary name; got:\n{rendered}"
    );
}

#[test]
fn every_target_subcommand_renders_help_in_process() {
    let mut failures: Vec<String> = Vec::new();
    for sub in TOP_LEVEL_SUBCOMMANDS {
        let args = ["amplihack", sub, "--help"];
        let result = Cli::command().try_get_matches_from(args);
        match result {
            Err(e) if e.kind() == ErrorKind::DisplayHelp && e.exit_code() == 0 => {}
            Err(e) => failures.push(format!(
                "subcommand `{sub}` --help returned non-help error: kind={:?}, code={}, msg={}",
                e.kind(),
                e.exit_code(),
                e
            )),
            Ok(_) => failures.push(format!(
                "subcommand `{sub}` --help parsed successfully instead of emitting help"
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "one or more target subcommands failed --help harness:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_target_subcommand_is_registered() {
    let mut command = Cli::command();
    let actual: Vec<String> = command
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    let mut missing: Vec<&str> = Vec::new();
    for sub in TOP_LEVEL_SUBCOMMANDS {
        if !actual.iter().any(|name| name == sub) {
            missing.push(sub);
        }
    }
    assert!(
        missing.is_empty(),
        "expected subcommands missing from `amplihack` Cli: {missing:?}\nactual subcommands: {actual:?}"
    );

    for sub in TOP_LEVEL_SUBCOMMANDS {
        let subcommand = command
            .find_subcommand_mut(sub)
            .unwrap_or_else(|| panic!("clap lost track of subcommand `{sub}` mid-test"));
        let mut buf = Vec::new();
        subcommand
            .write_long_help(&mut buf)
            .unwrap_or_else(|e| panic!("write_long_help for `{sub}` panicked: {e}"));
        let rendered = String::from_utf8(buf).expect("clap help must be valid UTF-8");
        assert!(!rendered.is_empty(), "rendered help for `{sub}` was empty");
    }
}

#[test]
fn builder_subcommand_help_lists_claude_and_codex_leaves() {
    assert_help_emitted(&["amplihack", "builder", "--help"]);

    let mut command = Cli::command();
    let builder = command
        .find_subcommand_mut("builder")
        .expect("`amplihack builder` must exist");
    let leaves: Vec<String> = builder
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    for expected in ["claude", "codex"] {
        assert!(
            leaves.iter().any(|name| name == expected),
            "`amplihack builder` must expose `{expected}` leaf; actual: {leaves:?}"
        );
    }
}

#[test]
fn remote_subcommand_help_lists_required_leaves() {
    assert_help_emitted(&["amplihack", "remote", "--help"]);

    let mut command = Cli::command();
    let remote = command
        .find_subcommand_mut("remote")
        .expect("`amplihack remote` must exist");
    let leaves: Vec<String> = remote
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    for expected in ["exec", "list", "start", "output", "kill", "status"] {
        assert!(
            leaves.iter().any(|name| name == expected),
            "`amplihack remote` must expose `{expected}` leaf; actual: {leaves:?}"
        );
    }
}

/// Deliberately negative test that locks down a behavior contract:
/// the harness must reject typo'd subcommand names rather than silently
/// dispatch to an unrelated leaf. If this regresses (clap allows a fuzzy
/// match that goes through), the failure surface is immediate.
#[test]
fn unknown_top_level_subcommand_is_rejected() {
    let result = Cli::command().try_get_matches_from(["amplihack", "buidler", "--help"]);
    let err = result.expect_err("typo subcommand `buidler` must not be accepted");
    assert_ne!(
        err.kind(),
        ErrorKind::DisplayHelp,
        "typo should not emit help; got: {err}"
    );
}
