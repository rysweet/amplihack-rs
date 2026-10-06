//! Issue #1525 and the crusty review of #1490: the migrate skill's
//! `detect_cli` asks `amplihack agent-binary` for the launcher-context and
//! default layers instead of keeping a bash copy of the resolver.
//!
//! `tests/issue_1525_migrate_detect_cli_parity.sh` checks the shell side
//! against a stand-in `amplihack`. These run `detect_cli`, lifted out of
//! `migrate.sh` the same way, against this build of the real binary: what the
//! shell parses is what the resolver prints, and what the resolver checks --
//! freshness, RFC 3339, an unusable file named with its reason -- reaches a
//! migrate run without a second copy of any of it.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MIGRATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../amplifier-bundle/skills/migrate/scripts/migrate.sh"
);

/// Defines `log_warn` and the `detect_cli` functions from migrate.sh, with
/// the parent-process chain reporting no agent CLI, then runs `detect_cli`.
const DETECT_CLI: &str = r#"
eval "$(awk '/^log_warn\(\)/' "$MIGRATE")"
eval "$(awk '/^_?detect_cli[a-z_]*\(\) \{/,/^\}/' "$MIGRATE")"
declare -F detect_cli >/dev/null || { echo "detect_cli not defined" >&2; exit 90; }
ps() { :; }
detect_cli
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        // A `.git` boundary stops the resolver's walk-up at the work dir.
        fs::create_dir_all(dir.path().join("work/.git")).expect("create work dir");
        fs::create_dir_all(dir.path().join("home")).expect("create home");
        Self { dir }
    }

    fn work(&self) -> PathBuf {
        self.dir
            .path()
            .join("work")
            .canonicalize()
            .expect("canonicalize work dir")
    }

    fn context_path(&self) -> PathBuf {
        self.work().join(".claude/runtime/launcher_context.json")
    }

    fn write_context(&self, body: &str) -> PathBuf {
        let path = self.context_path();
        fs::create_dir_all(path.parent().unwrap()).expect("create runtime dir");
        fs::write(&path, body).expect("write launcher context");
        path
    }

    /// `detect_cli` from the work dir: no session marker, no agent binary
    /// variable unless given in `env`, this build of amplihack first on PATH.
    fn detect_cli(&self, env: &[(&str, &str)]) -> (String, String) {
        let bin_dir = Path::new(env!("CARGO_BIN_EXE_amplihack"))
            .parent()
            .expect("amplihack has a directory")
            .to_path_buf();
        let path = std::env::join_paths(std::iter::once(bin_dir).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .expect("join PATH");
        let mut command = Command::new("bash");
        command
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.dir.path().join("home"))
            .env("TMPDIR", std::env::temp_dir())
            .env("MIGRATE", MIGRATE)
            .current_dir(self.work())
            .args(["-c", DETECT_CLI]);
        for (key, value) in env {
            command.env(key, value);
        }
        let output: Output = command.output().expect("run bash");
        assert!(output.status.success(), "detect_cli failed: {output:?}");
        (
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_string(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

/// A timestamp in the writer's own format, so a format change there is
/// tested here too.
fn now_rfc3339() -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    amplihack_cli::launcher_context::write_launcher_context(
        dir.path(),
        amplihack_cli::launcher_context::LauncherKind::Claude,
        "amplihack claude",
        Default::default(),
    )
    .expect("write launcher context");
    amplihack_cli::launcher_context::read_launcher_context(dir.path())
        .expect("read it back")
        .timestamp
}

/// Nothing to go on: the resolver's default, with its notice on stderr. The
/// shell's own fallback warning would mean amplihack was not reached.
#[test]
fn with_nothing_to_go_on_the_resolver_gives_its_default() {
    let fx = Fixture::new();
    let (out, err) = fx.detect_cli(&[]);
    assert_eq!(out, "copilot");
    assert!(
        err.contains(
            "amplihack: resolved the agent binary to 'copilot' (no AMPLIHACK_AGENT_BINARY \
             or agent session marker was found)"
        ),
        "{err}"
    );
    assert!(!err.contains("no launcher context was read"), "{err}");
}

/// A fresh context written by amplihack itself answers through the resolver.
#[test]
fn a_fresh_launcher_context_answers_through_the_resolver() {
    let fx = Fixture::new();
    amplihack_cli::launcher_context::write_launcher_context(
        &fx.work(),
        amplihack_cli::launcher_context::LauncherKind::Codex,
        "amplihack codex",
        Default::default(),
    )
    .expect("write launcher context");
    let (out, err) = fx.detect_cli(&[]);
    assert_eq!(out, "codex", "stderr: {err}");
    assert!(err.contains("read from"), "the resolver says where: {err}");
}

/// An empty file is named by the resolver, on the user's terminal.
#[test]
fn an_empty_launcher_context_is_named_and_the_default_answers() {
    let fx = Fixture::new();
    let path = fx.write_context("");
    let (out, err) = fx.detect_cli(&[]);
    assert_eq!(out, "copilot");
    assert!(
        err.contains(&format!(
            "amplihack: ignored {}: it is empty.",
            path.display()
        )),
        "{err}"
    );
    assert!(
        !err.contains("did not name an agent CLI"),
        "the resolver answered; no fallback: {err}"
    );
}

/// The case the bash RFC 3339 parser existed for: a timestamp with no offset
/// is refused and named, by the one parser there now is.
#[test]
fn a_timestamp_without_an_offset_is_named_by_the_resolver() {
    let fx = Fixture::new();
    let path = fx.write_context(r#"{"launcher":"claude","timestamp":"2026-10-04 13:13:46"}"#);
    let (out, err) = fx.detect_cli(&[]);
    assert_eq!(out, "copilot");
    assert!(
        err.contains(&format!(
            "amplihack: ignored {}: it has a timestamp that is not RFC 3339",
            path.display()
        )),
        "{err}"
    );
}

/// A stale file is passed over silently, by the resolver's 24-hour bound.
#[test]
fn a_stale_launcher_context_is_passed_over() {
    let fx = Fixture::new();
    fx.write_context(r#"{"launcher":"claude","timestamp":"2020-01-01T00:00:00Z"}"#);
    let (out, err) = fx.detect_cli(&[]);
    assert_eq!(out, "copilot");
    assert!(!err.contains("ignored"), "{err}");
}

/// A tagged default guess is skipped by the shell and by the resolver alike,
/// so a fresh context below it answers.
#[test]
fn a_tagged_guess_is_skipped_on_both_sides() {
    let fx = Fixture::new();
    fx.write_context(&format!(
        r#"{{"launcher":"codex","timestamp":"{}"}}"#,
        now_rfc3339()
    ));
    let (out, err) = fx.detect_cli(&[
        ("AMPLIHACK_AGENT_BINARY", "copilot"),
        ("AMPLIHACK_AGENT_BINARY_SOURCE", "default:copilot"),
    ]);
    assert_eq!(out, "codex", "stderr: {err}");
}
