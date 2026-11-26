//! Parse tests for `amplihack cleanup` (issue #1808), kept beside the command
//! rather than in `cli_tests.rs`.

use crate::Cli;

/// Issue #1808: `amplihack cleanup` parses its documented flags and the
/// `restore` subcommand, and rejects bad usage at parse time (exit 2).
#[test]
fn issue_1808_cleanup_command_parses() {
    for ok in [
        &["amplihack", "cleanup"][..],
        &[
            "amplihack",
            "cleanup",
            "--apply",
            "--format",
            "json",
            "--keep-hours",
            "0",
            "--log-days",
            "7",
            "--only",
            "run-tmp",
            "--only",
            "session-log",
        ],
        &["amplihack", "cleanup", "--skip-logs"],
        &[
            "amplihack",
            "cleanup",
            "restore",
            "/x/MANIFEST.jsonl",
            "--dest",
            "/y",
        ],
    ] {
        assert!(Cli::try_parse_from(ok).is_ok(), "{ok:?} should parse");
    }
    for bad in [
        &["amplihack", "cleanup", "--only", "bogus"][..],
        &["amplihack", "cleanup", "--only", "run-tmp", "--skip-logs"],
        &["amplihack", "cleanup", "--format", "xml"],
        &["amplihack", "cleanup", "--keep-hours", "87601"],
        &["amplihack", "cleanup", "--log-days", "36501"],
        &["amplihack", "cleanup", "--apply", "restore", "/x"],
        &["amplihack", "cleanup", "restore"],
    ] {
        let err = Cli::try_parse_from(bad).expect_err(&format!("{bad:?} must be rejected"));
        assert_eq!(err.exit_code(), 2, "{bad:?}: {err}");
    }
}

/// Issue #2039 (D6): the hidden `--worktree-target <CHECKOUT>` option parses
/// with `--apply` and `--format`, and conflicts with `--only`, `--skip-logs`,
/// `--keep-hours`, `--log-days` and the `restore` subcommand.
#[test]
fn issue_2039_worktree_target_parses_and_conflicts() {
    for ok in [
        &["amplihack", "cleanup", "--worktree-target", "/w/wt/feat"][..],
        &[
            "amplihack",
            "cleanup",
            "--apply",
            "--worktree-target",
            "/w/wt/feat",
        ],
        &[
            "amplihack",
            "cleanup",
            "--apply",
            "--format",
            "json",
            "--worktree-target",
            "/w/wt/feat",
        ],
        // Relative paths parse; the command refuses them with exit 1.
        &["amplihack", "cleanup", "--worktree-target", "rel/path"],
    ] {
        assert!(Cli::try_parse_from(ok).is_ok(), "{ok:?} should parse");
    }
    for bad in [
        &["amplihack", "cleanup", "--worktree-target"][..],
        &[
            "amplihack",
            "cleanup",
            "--worktree-target",
            "/x",
            "--only",
            "run-target",
        ],
        &[
            "amplihack",
            "cleanup",
            "--worktree-target",
            "/x",
            "--skip-logs",
        ],
        &[
            "amplihack",
            "cleanup",
            "--worktree-target",
            "/x",
            "--keep-hours",
            "1",
        ],
        &[
            "amplihack",
            "cleanup",
            "--worktree-target",
            "/x",
            "--log-days",
            "1",
        ],
        &[
            "amplihack",
            "cleanup",
            "--worktree-target",
            "/x",
            "restore",
            "/m",
        ],
    ] {
        let err = Cli::try_parse_from(bad).expect_err(&format!("{bad:?} must be rejected"));
        assert_eq!(err.exit_code(), 2, "{bad:?}: {err}");
    }
}

/// Issue #2039: `--worktree-target` is for the bundled helpers, not users;
/// `amplihack cleanup --help` does not list it.
#[test]
fn issue_2039_worktree_target_is_hidden_from_help() {
    let err = Cli::try_parse_from(["amplihack", "cleanup", "--help"]).unwrap_err();
    let help = err.to_string();
    assert!(help.contains("--apply"), "{help}");
    assert!(!help.contains("worktree-target"), "{help}");
}
