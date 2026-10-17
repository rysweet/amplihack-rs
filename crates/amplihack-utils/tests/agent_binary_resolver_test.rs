//! TDD: Failing tests for `amplihack_utils::agent_binary` resolver.
//!
//! These tests define the contract for the unified agent-binary resolver:
//! 1. Precedence: `AMPLIHACK_AGENT_BINARY` env > `launcher_context.json` > "copilot" default.
//! 2. Strict allowlist: only {claude, copilot, codex, amplifier}.
//! 3. Walk-up boundary, file size cap, validation hardening.
//!
//! Run with: `TMPDIR=/tmp cargo test -p amplihack-utils --test agent_binary_resolver_test`
//!
//! NOTE: tests mutate process env so they share `serial_test::serial` (or a
//! single-threaded runner). Edition-2024 requires `unsafe` for env mutation.

#![allow(clippy::unwrap_used)]

use amplihack_utils::launcher_context::LauncherKind;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

use amplihack_utils::agent_binary::{
    self, ALLOWED_BINARIES, DEFAULT_BINARY, ResolveError, resolve,
};

/// Serialize tests that mutate `AMPLIHACK_AGENT_BINARY`. `cargo test` runs
/// integration tests in parallel by default, which races env mutation across
/// the entire test binary. CI uses the default thread count, so the lock is
/// required for portability.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Write a launcher context using the real writer.
///
/// The previous fixture hand-wrote JSON and drifted: it emitted `created_at`
/// where the writer emits `timestamp`, plus a `pid` field the struct has never
/// had. That drift is exactly why the missing staleness bound went unnoticed,
/// and a second hand-written stand-in would only reset the clock on the same
/// failure. Call the producer instead (Fowler, "Contract Test").
fn write_launcher_context(repo: &Path, launcher: &str) {
    let kind = match launcher {
        "claude" => LauncherKind::Claude,
        "copilot" => LauncherKind::Copilot,
        "codex" => LauncherKind::Codex,
        "amplifier" => LauncherKind::Amplifier,
        other => panic!("unsupported launcher in fixture: {other}"),
    };
    amplihack_utils::launcher_context::write_launcher_context(
        repo,
        kind,
        format!("amplihack {launcher}"),
        std::collections::BTreeMap::new(),
    )
    .unwrap();
}

/// Write a context body the real writer cannot produce.
///
/// `LauncherKind` is a closed enum, so `write_launcher_context` physically
/// cannot emit a launcher outside the allowlist. That is the point of the
/// type -- and it means the reader's allowlist can only be tested by going
/// around the writer. Use this for hostile or malformed input, and the real
/// writer for everything legitimate.
fn write_raw_launcher_context(repo: &Path, body: &str) {
    let runtime = repo.join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("launcher_context.json"), body).unwrap();
}

/// Same, but stamped into the past so the staleness bound can be exercised.
///
/// Built from the real `LauncherContext` struct, so a field rename breaks this
/// at compile time rather than silently at runtime.
fn write_launcher_context_aged(repo: &Path, launcher: &str, age_hours: i64) {
    write_launcher_context(repo, launcher);
    let path = amplihack_utils::launcher_context::launcher_context_path(repo);
    let mut ctx: amplihack_utils::launcher_context::LauncherContext =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    ctx.timestamp = (chrono::Utc::now() - chrono::Duration::hours(age_hours)).to_rfc3339();
    fs::write(&path, serde_json::to_string_pretty(&ctx).unwrap()).unwrap();
}

fn clear_env() {
    // SAFETY: tests are serialized; env mutation is unsafe in edition 2024.
    unsafe {
        // The resolver now consults live session markers, which this test
        // binary inherits from whatever CLI is running it. Leave them set and
        // every case below silently resolves through layer 2. Sourced from
        // SESSION_MARKERS: a hand-copied list here fell behind the moment a
        // marker was added (issue #1481).
        for (key, _) in amplihack_utils::agent_binary::SESSION_MARKERS {
            std::env::remove_var(key);
        }
        std::env::remove_var("AMPLIHACK_AGENT_BINARY");
        std::env::remove_var(amplihack_utils::agent_binary::SOURCE_ENV);
    }
}

fn set_env(value: &str) {
    // SAFETY: see clear_env.
    unsafe {
        std::env::set_var("AMPLIHACK_AGENT_BINARY", value);
        // A value set here is a choice. Without this, a `default:<binary>` tag
        // inherited from a recipe step running the suite would make the
        // resolver skip it (issue #1481).
        std::env::remove_var(amplihack_utils::agent_binary::SOURCE_ENV);
    }
}

/// Some test inputs (NUL bytes, etc.) are rejected by the OS before reaching
/// our resolver. Skip those — the resolver's `validate_binary_name` unit tests
/// already cover the `\0` case directly. The security test below also exercises
/// these via `try_set_env`.
fn try_set_env(value: &str) -> bool {
    if value.bytes().any(|b| b == 0) {
        return false;
    }
    set_env(value);
    true
}

#[test]
fn default_is_copilot_when_nothing_set() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
    assert_eq!(DEFAULT_BINARY, "copilot");
}

#[test]
fn env_var_takes_precedence_over_file() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    set_env("copilot");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
    clear_env();
}

#[test]
fn launcher_context_used_when_env_unset() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "claude");
}

#[test]
fn allowlist_contains_exactly_four_binaries() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let mut sorted: Vec<&str> = ALLOWED_BINARIES.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, vec!["amplifier", "claude", "codex", "copilot"]);
}

#[test]
fn env_value_outside_allowlist_is_rejected() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env("malicious");
    let result = resolve(tmp.path()).unwrap();
    // Rejected → falls through to default.
    assert_eq!(result, "copilot");
    clear_env();
}

#[test]
fn env_value_with_path_separator_is_rejected() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    for bad in &["/bin/sh", "..\\evil", "claude/../sh", "cla\0ude"] {
        if !try_set_env(bad) {
            // OS rejects NUL bytes in env values; the validator unit tests
            // cover that branch directly (see `validate_rejects_dangerous_inputs`).
            continue;
        }
        let result = resolve(tmp.path()).unwrap();
        assert_eq!(result, "copilot", "value {bad:?} should be rejected");
    }
    clear_env();
}

#[test]
fn env_value_is_trimmed_and_lowercased() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    set_env("  CLAUDE  ");
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "claude");
    clear_env();
}

#[test]
fn launcher_context_outside_allowlist_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    write_raw_launcher_context(
        tmp.path(),
        &format!(r#"{{"launcher":"rm-rf-slash","timestamp":"{now}"}}"#),
    );
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
}

#[test]
fn oversized_launcher_context_is_rejected() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    // > 64 KiB cap.
    let huge = "x".repeat(70 * 1024);
    let body = format!(r#"{{"launcher":"claude","junk":"{huge}"}}"#);
    fs::write(runtime.join("launcher_context.json"), body).unwrap();
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot", "oversized file must be rejected");
}

#[test]
fn malformed_launcher_context_falls_back_to_default() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("launcher_context.json"), "{not valid json").unwrap();
    let result = resolve(tmp.path()).unwrap();
    assert_eq!(result, "copilot");
}

#[test]
fn walk_up_finds_context_in_ancestor() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    let nested = tmp.path().join("a").join("b").join("c");
    fs::create_dir_all(&nested).unwrap();
    let result = resolve(&nested).unwrap();
    assert_eq!(result, "claude");
}

#[test]
fn walk_up_stops_at_git_boundary() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    // Outer repo with launcher_context = claude.
    write_launcher_context(tmp.path(), "claude");
    // Inner "repo" with .git boundary, no launcher_context.
    let inner = tmp.path().join("inner");
    fs::create_dir_all(inner.join(".git")).unwrap();
    let nested = inner.join("src");
    fs::create_dir_all(&nested).unwrap();
    let result = resolve(&nested).unwrap();
    // Should NOT find outer claude — boundary respected.
    assert_eq!(result, "copilot");
}

#[test]
fn resolve_function_signature_returns_result() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Compile-time contract: resolve(&Path) -> Result<String, ResolveError>.
    clear_env();
    let tmp = TempDir::new().unwrap();
    let _: Result<String, ResolveError> = resolve(tmp.path());
}

#[test]
fn validate_binary_name_helper_exposed() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    assert!(agent_binary::validate_binary_name("claude").is_some());
    assert!(agent_binary::validate_binary_name("COPILOT").is_some());
    assert!(agent_binary::validate_binary_name("evil/bin").is_none());
    assert!(agent_binary::validate_binary_name("").is_none());
    assert!(agent_binary::validate_binary_name(&"x".repeat(64)).is_none());
}

/// Issue #1342: a launcher context describes a session, and sessions end.
/// The file that hijacked every workflow under /tmp was five days old --
/// 5x past a 24-hour bound the codebase already defined and this resolver
/// alone ignored.
#[test]
fn stale_launcher_context_is_ignored() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context_aged(tmp.path(), "claude", 5 * 24);
    assert_eq!(
        resolve(tmp.path()).unwrap(),
        "copilot",
        "a context older than the staleness bound must not decide the binary"
    );
}

/// The bound must not be so tight that a context written minutes ago,
/// by the session actually running, gets thrown away.
#[test]
fn fresh_launcher_context_is_honoured() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    write_launcher_context_aged(tmp.path(), "claude", 1);
    assert_eq!(resolve(tmp.path()).unwrap(), "claude");
}

/// A context with no timestamp has no knowable age. Fail closed rather than
/// trusting it (#1342), and name the file, because however recent it is it will
/// never be used (#1525). The writer has always emitted a timestamp, so such a
/// file was written by hand or by something else.
#[test]
fn launcher_context_without_timestamp_is_ignored_and_named() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(
        runtime.join("launcher_context.json"),
        r#"{"launcher":"claude","pid":1234}"#,
    )
    .unwrap();
    let resolution = agent_binary::resolve_detailed(tmp.path()).unwrap();
    assert_eq!(resolution.binary, "copilot");
    assert_eq!(resolution.context_file, None);
    let reasons: Vec<&str> = resolution
        .unusable_contexts
        .iter()
        .map(|skipped| skipped.reason.as_str())
        .collect();
    assert_eq!(reasons, ["has no timestamp, so its age is unknown"]);
}

/// Issue #1525 review: a timestamp that is present but not RFC 3339 used to
/// be passed over as stale, unnamed, while migrate.sh's `date -d` read it as
/// fresh. Rust and shell now both name it and fall through.
#[test]
fn launcher_context_with_a_non_rfc3339_timestamp_is_ignored_and_named() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let runtime = tmp.path().join(".claude").join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let local = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");
    fs::write(
        runtime.join("launcher_context.json"),
        format!(r#"{{"launcher":"claude","timestamp":"{local}"}}"#),
    )
    .unwrap();
    let resolution = agent_binary::resolve_detailed(tmp.path()).unwrap();
    assert_eq!(resolution.binary, "copilot");
    let reasons: Vec<&str> = resolution
        .unusable_contexts
        .iter()
        .map(|skipped| skipped.reason.as_str())
        .collect();
    assert_eq!(reasons, ["has a timestamp that is not RFC 3339"]);
}

/// Crusty round 3: an explicit value still outranks a live session marker,
/// and the marker it overruled is kept, so a caller can say which session
/// was overridden. With no marker visible there is nothing to keep.
#[test]
fn an_explicit_value_keeps_the_session_marker_it_overrode() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    set_env("copilot");
    let without_marker = agent_binary::resolve_detailed(tmp.path()).unwrap();
    // SAFETY: serialized by env_lock; cleared again below.
    unsafe { std::env::set_var("CLAUDE_CODE_ENTRYPOINT", "cli") };
    let with_marker = agent_binary::resolve_detailed(tmp.path()).unwrap();
    clear_env();

    assert_eq!(without_marker.session_marker, None);
    assert_eq!(with_marker.binary, "copilot");
    assert_eq!(with_marker.source, agent_binary::ResolutionSource::Env);
    assert_eq!(
        with_marker.session_marker,
        Some(agent_binary::SessionMarker {
            variable: "CLAUDE_CODE_ENTRYPOINT",
            binary: "claude",
        })
    );
}

/// Issue #1481: a value tagged as a parent's default guess for the same binary
/// is skipped, and the layers below it answer. A tag naming another binary
/// does not veto the value.
#[test]
fn a_tagged_default_guess_is_skipped_by_the_resolver() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_env();
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".git")).unwrap();
    write_launcher_context(tmp.path(), "codex");

    set_env("copilot");
    // SAFETY: see clear_env.
    unsafe { std::env::set_var(agent_binary::SOURCE_ENV, "default:copilot") };
    let skipped = agent_binary::resolve_with_source(tmp.path()).unwrap();

    unsafe { std::env::set_var(agent_binary::SOURCE_ENV, "default:claude") };
    let honoured = agent_binary::resolve_with_source(tmp.path()).unwrap();
    clear_env();

    assert_eq!(
        skipped,
        (
            "codex".to_string(),
            agent_binary::ResolutionSource::LauncherContext
        )
    );
    assert_eq!(
        honoured,
        ("copilot".to_string(), agent_binary::ResolutionSource::Env)
    );
}

// ---------------------------------------------------------------------------
// Crusty review of #1490 at baaafb18: a value amplihack exported to describe a
// session, tagged `session:<binary>`, is not an instruction. It ranks with the
// session markers of the CLI it names, through the real environment and the
// real `resolve_detailed`.
// ---------------------------------------------------------------------------

/// Set `AMPLIHACK_AGENT_BINARY` with the tag a launcher exports beside it, and
/// each of `markers` to `1`, after clearing everything else.
fn set_described(binary: &str, markers: &[&str]) {
    clear_env();
    // SAFETY: see clear_env.
    unsafe {
        std::env::set_var("AMPLIHACK_AGENT_BINARY", binary);
        std::env::set_var(agent_binary::SOURCE_ENV, agent_binary::session_tag(binary));
        for marker in markers {
            std::env::set_var(marker, "1");
        }
    }
}

/// Crusty's reproduction: a Claude Code session in a pane of a tmux server an
/// `amplihack copilot` agent started holds `CLAUDECODE` and the server's
/// `COPILOT_CLI=1` and `AMPLIHACK_AGENT_BINARY=copilot`. It resolves claude,
/// from its own marker. Untagged, the copilot value answered from layer 1.
#[test]
fn a_launchers_description_does_not_outrank_a_live_claude_marker() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "copilot");
    set_described("copilot", &["CLAUDECODE", "COPILOT_CLI"]);
    let resolution = agent_binary::resolve_detailed(tmp.path()).unwrap();
    clear_env();
    assert_eq!(resolution.binary, "claude");
    assert_eq!(
        resolution.source,
        agent_binary::ResolutionSource::SessionMarker
    );
    assert_eq!(
        resolution.session_marker.map(|m| m.variable),
        Some("CLAUDECODE")
    );
}

/// With no live marker, the description answers, from layer 2, above any
/// launcher context, and the marker it reports is the tag's variable.
#[test]
fn a_launchers_description_alone_answers_from_layer_two() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    write_launcher_context(tmp.path(), "claude");
    set_described("codex", &[]);
    let resolution = agent_binary::resolve_detailed(tmp.path()).unwrap();
    clear_env();
    assert_eq!(resolution.binary, "codex");
    assert_eq!(
        resolution.source,
        agent_binary::ResolutionSource::SessionMarker
    );
    assert_eq!(
        resolution.session_marker.map(|m| (m.variable, m.binary)),
        Some((agent_binary::SOURCE_ENV, "codex"))
    );
}

/// `recipe run` hands its steps `session:claude` and unsets `CLAUDECODE`. A
/// Copilot marker those steps inherited from a tmux pane does not take them
/// over.
#[test]
fn a_claude_description_outranks_a_copilot_marker() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    set_described("claude", &["COPILOT_CLI"]);
    let resolved = resolve(tmp.path()).unwrap();
    clear_env();
    assert_eq!(resolved, "claude");
}

/// An untagged value is still an instruction, above every marker; so is one
/// set after a tag that named another binary.
#[test]
fn an_untagged_or_retagged_value_is_still_an_instruction() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    clear_env();
    // SAFETY: see clear_env.
    unsafe {
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot");
    }
    let untagged = agent_binary::resolve_with_source(tmp.path()).unwrap();
    unsafe {
        std::env::set_var(agent_binary::SOURCE_ENV, "session:claude");
    }
    let retagged = agent_binary::resolve_with_source(tmp.path()).unwrap();
    clear_env();
    for resolved in [untagged, retagged] {
        assert_eq!(
            resolved,
            ("copilot".to_string(), agent_binary::ResolutionSource::Env)
        );
    }
}
