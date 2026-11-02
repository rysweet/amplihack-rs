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
