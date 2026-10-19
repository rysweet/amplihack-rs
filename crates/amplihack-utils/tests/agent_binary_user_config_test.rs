//! Issue #1606 (TDD): the `user_config` layer of the agent-binary resolver.
//!
//! Contract (docs/reference/user-config.md#agentbinary), highest first:
//!
//! 1. `env`: an `AMPLIHACK_AGENT_BINARY` someone set: untagged, carrying a tag
//!    amplihack does not recognise, or a `user_config:<other>` tag (stale).
//! 2. `session_marker`: a live session marker, or a launcher's `session:*`
//!    tag, unless the tmux server's global environment holds it.
//! 3. `user_config`: this process's configured `agent.binary`, else an
//!    inherited value tagged `user_config:<same>` (or a bare `user_config`).
//! 4. what a tmux server supplied: a `tmux_server:*`-tagged value (label
//!    `env`), then a marker the server holds (`session_marker`).
//! 5. `launcher_context`, then `default` (`copilot`); a `default:*`-tagged
//!    value is ignored.
//!
//! The pure parts (`classify_inherited`, `resolve_layers`) are tested directly;
//! `resolve_detailed_configured` is tested with the process env under a lock.

#![allow(clippy::unwrap_used)]

use amplihack_utils::agent_binary::{
    self, BINARY_ENV, DEFAULT_BINARY, InheritedBinary, Layers, ResolutionSource, SESSION_MARKERS,
    SOURCE_ENV, USER_CONFIG_TAG, classify_inherited, export_tag, resolve_layers, user_config_tag,
};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

// ── ResolutionSource::UserConfig ────────────────────────────────────────────

#[test]
fn user_config_label_is_stable() {
    assert_eq!(ResolutionSource::UserConfig.label(), "user_config");
}

#[test]
fn user_config_is_not_inferred() {
    assert!(!ResolutionSource::UserConfig.is_inferred());
}

#[test]
fn user_config_tag_uses_kind_colon_binary_form() {
    assert_eq!(USER_CONFIG_TAG, "user_config");
    assert_eq!(user_config_tag("claude"), "user_config:claude");
}

#[test]
fn export_tag_for_user_config_binds_to_the_binary() {
    assert_eq!(
        export_tag("claude", ResolutionSource::UserConfig).as_deref(),
        Some("user_config:claude")
    );
    // Unchanged: an explicit env value is still handed on untagged.
    assert_eq!(export_tag("claude", ResolutionSource::Env), None);
}

// ── classify_inherited ──────────────────────────────────────────────────────

#[test]
fn classify_untagged_value_is_explicit() {
    assert_eq!(
        classify_inherited(Some("codex"), None),
        InheritedBinary::Explicit("codex".into())
    );
}

#[test]
fn classify_unknown_tag_is_explicit() {
    assert_eq!(
        classify_inherited(Some("codex"), Some("hand_written:whatever")),
        InheritedBinary::Explicit("codex".into())
    );
}

#[test]
fn classify_matching_user_config_tag_is_user_config() {
    assert_eq!(
        classify_inherited(Some("claude"), Some("user_config:claude")),
        InheritedBinary::UserConfig("claude".into())
    );
}

#[test]
fn classify_user_config_tag_matches_case_insensitively() {
    assert_eq!(
        classify_inherited(Some("Claude"), Some("user_config:CLAUDE")),
        InheritedBinary::UserConfig("claude".into())
    );
}

#[test]
fn classify_stale_user_config_tag_is_explicit() {
    // Someone set codex by hand next to a tag describing an earlier claude.
    assert_eq!(
        classify_inherited(Some("codex"), Some("user_config:claude")),
        InheritedBinary::Explicit("codex".into())
    );
}

#[test]
fn classify_bare_user_config_tag_binds_to_any_value() {
    assert_eq!(
        classify_inherited(Some("amplifier"), Some("user_config")),
        InheritedBinary::UserConfig("amplifier".into())
    );
}

#[test]
fn classify_tmux_server_tag_is_inherited_not_explicit() {
    assert_eq!(
        classify_inherited(Some("copilot"), Some("tmux_server:COPILOT_CLI")),
        InheritedBinary::Inherited("copilot".into())
    );
}

#[test]
fn classify_default_guess_is_ignored() {
    assert_eq!(
        classify_inherited(Some("copilot"), Some("default:copilot")),
        InheritedBinary::DefaultGuess
    );
}

#[test]
fn classify_session_tag_ranks_with_markers() {
    assert!(matches!(
        classify_inherited(Some("claude"), Some("session:claude")),
        InheritedBinary::SessionDescribed(_)
    ));
}

#[test]
fn classify_absent_value_is_absent_whatever_the_tag() {
    assert_eq!(classify_inherited(None, None), InheritedBinary::Absent);
    assert_eq!(
        classify_inherited(None, Some("user_config:claude")),
        InheritedBinary::Absent
    );
}

#[test]
fn classify_validates_before_reading_the_tag() {
    // S7: a tag cannot launder a value off the allowlist.
    for bad in ["/bin/sh", "claude;rm", "gpt", "", "claude\n", "../claude"] {
        assert_eq!(
            classify_inherited(Some(bad), Some("user_config")),
            InheritedBinary::Absent,
            "value {bad:?} must be rejected before its tag is read"
        );
        assert_eq!(
            classify_inherited(Some(bad), Some(&format!("user_config:{bad}"))),
            InheritedBinary::Absent,
            "value {bad:?} must be rejected before its tag is read"
        );
    }
}

#[test]
fn classify_is_total_on_garbage_tags() {
    for tag in [
        "",
        ":",
        "user_config:",
        "user_config::claude",
        "\u{202e}",
        "x".repeat(4096).as_str(),
    ] {
        // Must not panic; any answer that keeps the value is acceptable.
        let _ = classify_inherited(Some("claude"), Some(tag));
    }
}

// ── resolve_layers(Layers) ──────────────────────────────────────────────────

#[test]
fn layers_all_empty_is_the_default() {
    assert_eq!(
        resolve_layers(Layers::default()),
        (DEFAULT_BINARY.to_string(), ResolutionSource::Default)
    );
}

#[test]
fn explicit_env_beats_user_config() {
    let got = resolve_layers(Layers {
        env: s("codex"),
        user_config: s("claude"),
        inherited: s("copilot"),
        marker: s("copilot"),
        server_held_marker: s("copilot"),
        persisted: s("amplifier"),
    });
    assert_eq!(got, ("codex".into(), ResolutionSource::Env));
}

#[test]
fn user_config_beats_every_tmux_server_held_value() {
    let got = resolve_layers(Layers {
        user_config: s("claude"),
        inherited: s("copilot"),
        server_held_marker: s("copilot"),
        persisted: s("codex"),
        ..Layers::default()
    });
    assert_eq!(got, ("claude".into(), ResolutionSource::UserConfig));
}

/// Review of #1647: the CLI actually hosting this process (a live marker, or
/// a launcher's `session:<cli>` tag, which ranks with the markers) outranks
/// the configured value.
#[test]
fn a_live_session_marker_beats_user_config() {
    let got = resolve_layers(Layers {
        user_config: s("copilot"),
        marker: s("claude"),
        ..Layers::default()
    });
    assert_eq!(got, ("claude".into(), ResolutionSource::SessionMarker));
}

#[test]
fn user_config_beats_launcher_context() {
    let got = resolve_layers(Layers {
        user_config: s("codex"),
        persisted: s("claude"),
        ..Layers::default()
    });
    assert_eq!(got, ("codex".into(), ResolutionSource::UserConfig));
}

#[test]
fn without_config_inherited_tmux_value_keeps_env_label() {
    // It still beats a marker the tmux server holds, under its old label.
    let got = resolve_layers(Layers {
        inherited: s("copilot"),
        server_held_marker: s("claude"),
        ..Layers::default()
    });
    assert_eq!(got, ("copilot".into(), ResolutionSource::Env));
    // A marker the server does not hold is this session's own, and wins.
    let got = resolve_layers(Layers {
        inherited: s("copilot"),
        marker: s("claude"),
        ..Layers::default()
    });
    assert_eq!(got, ("claude".into(), ResolutionSource::SessionMarker));
}

#[test]
fn without_config_marker_then_persisted_order_is_unchanged() {
    assert_eq!(
        resolve_layers(Layers {
            marker: s("claude"),
            persisted: s("copilot"),
            ..Layers::default()
        }),
        ("claude".into(), ResolutionSource::SessionMarker)
    );
    assert_eq!(
        resolve_layers(Layers {
            persisted: s("codex"),
            ..Layers::default()
        }),
        ("codex".into(), ResolutionSource::LauncherContext)
    );
}

// ── resolve_detailed_with (process env) ─────────────────────────────────────

/// Clear every variable the resolver reads, run `f`, then restore.
fn with_env(vars: &[(&str, &str)], f: impl FnOnce()) {
    let _guard = env_lock();
    let mut keys: Vec<&str> = vec![BINARY_ENV, SOURCE_ENV, "TMUX"];
    keys.extend(SESSION_MARKERS.iter().map(|(k, _)| *k));
    let saved: Vec<(String, Option<std::ffi::OsString>)> = keys
        .iter()
        .map(|k| (k.to_string(), std::env::var_os(k)))
        .collect();
    // SAFETY: serialized by `env_lock`; this test binary has no other threads
    // that read the environment concurrently.
    unsafe {
        for k in &keys {
            std::env::remove_var(k);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
    }
    f();
    unsafe {
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
    }
}

#[test]
fn acceptance_config_beats_a_foreign_tmux_marker() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[
            (BINARY_ENV, "copilot"),
            (SOURCE_ENV, "tmux_server:COPILOT_CLI"),
            ("COPILOT_CLI", "1"),
        ],
        || {
            // The tmux server's global environment holds COPILOT_CLI=1.
            let held = |_| true;
            let r = agent_binary::resolve_detailed_configured(
                dir.path(),
                agent_binary::ConfiguredBinary::Set("claude"),
                &held,
            )
            .unwrap();
            assert_eq!(r.binary, "claude");
            assert_eq!(r.source, ResolutionSource::UserConfig);
        },
    );
}

#[test]
fn a_marker_the_tmux_server_does_not_hold_beats_config() {
    let dir = tempfile::tempdir().unwrap();
    with_env(&[("COPILOT_CLI", "1")], || {
        let asked = std::cell::Cell::new(false);
        let held = |_| {
            asked.set(true);
            false
        };
        let r = agent_binary::resolve_detailed_configured(
            dir.path(),
            agent_binary::ConfiguredBinary::Set("claude"),
            &held,
        )
        .unwrap();
        assert!(asked.get());
        assert_eq!(r.binary, "copilot");
        assert_eq!(r.source, ResolutionSource::SessionMarker);
    });
}

#[test]
fn a_launchers_session_tag_beats_config() {
    // What `amplihack claude` exports to its children.
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[(BINARY_ENV, "claude"), (SOURCE_ENV, "session:claude")],
        || {
            let r = agent_binary::resolve_detailed_configured(
                dir.path(),
                agent_binary::ConfiguredBinary::Set("copilot"),
                &|_| false,
            )
            .unwrap();
            assert_eq!(r.binary, "claude");
            assert_eq!(r.source, ResolutionSource::SessionMarker);
        },
    );
}

#[test]
fn the_tmux_server_is_not_asked_when_nothing_below_could_answer() {
    let dir = tempfile::tempdir().unwrap();
    with_env(&[("CLAUDECODE", "1")], || {
        let held = |_| panic!("asked without a contest");
        let r = agent_binary::resolve_detailed_configured(
            dir.path(),
            agent_binary::ConfiguredBinary::NotRead,
            &held,
        )
        .unwrap();
        assert_eq!(r.source, ResolutionSource::SessionMarker);
    });
}

#[test]
fn acceptance_untagged_env_beats_config() {
    let dir = tempfile::tempdir().unwrap();
    with_env(&[(BINARY_ENV, "codex")], || {
        let r = agent_binary::resolve_detailed_with(dir.path(), Some("claude")).unwrap();
        assert_eq!(r.binary, "codex");
        assert_eq!(r.source, ResolutionSource::Env);
    });
}

#[test]
fn this_process_config_beats_an_inherited_user_config_tag() {
    // Review of #1647: a `config set` takes effect even under a tmux server
    // whose panes captured the old value.
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[(BINARY_ENV, "claude"), (SOURCE_ENV, "user_config:claude")],
        || {
            let r = agent_binary::resolve_detailed_with(dir.path(), Some("codex")).unwrap();
            assert_eq!(r.binary, "codex");
            assert_eq!(r.source, ResolutionSource::UserConfig);
            // With no config read here, the inherited tag still answers.
            let r = agent_binary::resolve_detailed_with(dir.path(), None).unwrap();
            assert_eq!(r.binary, "claude");
            assert_eq!(r.source, ResolutionSource::UserConfig);
            // A config that was read and no longer sets agent.binary (after
            // `config unset`) makes the inherited tag stale.
            let r = agent_binary::resolve_detailed_configured(
                dir.path(),
                agent_binary::ConfiguredBinary::Unset,
                &|_| false,
            )
            .unwrap();
            assert_eq!(r.binary, DEFAULT_BINARY);
            assert_eq!(r.source, ResolutionSource::Default);
        },
    );
}

#[test]
fn stale_user_config_tag_makes_the_value_explicit() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[(BINARY_ENV, "codex"), (SOURCE_ENV, "user_config:claude")],
        || {
            let r = agent_binary::resolve_detailed_with(dir.path(), Some("claude")).unwrap();
            assert_eq!(r.binary, "codex");
            assert_eq!(r.source, ResolutionSource::Env);
        },
    );
}

#[test]
fn configured_value_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    with_env(&[], || {
        let r = agent_binary::resolve_detailed_with(dir.path(), Some("/bin/sh")).unwrap();
        assert_eq!(r.binary, DEFAULT_BINARY);
        assert_eq!(r.source, ResolutionSource::Default);
    });
}

#[test]
fn resolve_detailed_equals_resolve_detailed_with_none() {
    let dir = tempfile::tempdir().unwrap();
    with_env(&[("CLAUDECODE", "1")], || {
        let a = agent_binary::resolve_detailed(dir.path()).unwrap();
        let b = agent_binary::resolve_detailed_with(dir.path(), None).unwrap();
        assert_eq!((a.binary, a.source), (b.binary, b.source));
    });
}

#[test]
fn default_guess_tag_is_still_ignored_below_config() {
    let dir = tempfile::tempdir().unwrap();
    with_env(
        &[(BINARY_ENV, "copilot"), (SOURCE_ENV, "default:copilot")],
        || {
            let r = agent_binary::resolve_detailed_with(dir.path(), Some("amplifier")).unwrap();
            assert_eq!(r.binary, "amplifier");
            assert_eq!(r.source, ResolutionSource::UserConfig);
        },
    );
}
