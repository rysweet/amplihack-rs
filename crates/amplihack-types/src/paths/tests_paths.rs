use super::*;
use crate::test_support::env_lock;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_test_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = crate::test_support::scratch_base::scratch_base().join(format!(
        "amplihack-types-{name}-{}-{unique}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn paths_are_consistent() {
    let dirs = ProjectDirs::new("/project");
    assert_eq!(dirs.claude, PathBuf::from("/project/.claude"));
    assert_eq!(dirs.runtime, PathBuf::from("/project/.claude/runtime"));
    assert_eq!(dirs.locks, PathBuf::from("/project/.claude/runtime/locks"));
    assert_eq!(
        dirs.metrics,
        PathBuf::from("/project/.claude/runtime/metrics")
    );
    assert_eq!(
        dirs.lock_active_file(),
        PathBuf::from("/project/.claude/runtime/locks/.lock_active")
    );
    assert_eq!(
        dirs.lock_goal_file(),
        PathBuf::from("/project/.claude/runtime/locks/.lock_goal")
    );
    assert_eq!(
        dirs.lock_message_file(),
        PathBuf::from("/project/.claude/runtime/locks/.lock_message")
    );
    assert_eq!(
        dirs.continuation_prompt_file(),
        PathBuf::from("/project/.claude/runtime/locks/.continuation_prompt")
    );
}

#[test]
fn session_paths() {
    let dirs = ProjectDirs::new("/project");
    assert_eq!(
        dirs.session_locks("abc"),
        PathBuf::from("/project/.claude/runtime/locks/abc")
    );
    assert_eq!(
        dirs.session_logs("abc"),
        PathBuf::from("/project/.claude/runtime/logs/abc")
    );
}

#[test]
fn sanitize_normal_session_id() {
    assert_eq!(
        sanitize_session_id("normal-session-id-123"),
        "normal-session-id-123"
    );
}

#[test]
fn sanitize_strips_path_traversal() {
    // `..` and `/` are replaced with underscores
    assert_eq!(
        sanitize_session_id("../../../etc/passwd"),
        "_________etc_passwd"
    );
}

#[test]
fn sanitize_strips_forward_slashes() {
    assert_eq!(sanitize_session_id("foo/bar"), "foo_bar");
}

#[test]
fn sanitize_strips_backslashes() {
    assert_eq!(sanitize_session_id("foo\\bar"), "foo_bar");
}

#[test]
fn sanitize_strips_mixed_traversal() {
    assert_eq!(
        sanitize_session_id("..\\..\\windows\\system32"),
        "______windows_system32"
    );
}

#[test]
fn sanitize_replaces_dots_with_underscore() {
    assert_eq!(sanitize_session_id("session.123"), "session_123");
}

#[test]
fn sanitize_replaces_traversal_chars_with_underscores() {
    // ../../../ → all dots and slashes become underscores
    let result = sanitize_session_id("../../../");
    assert_eq!(result, "_________");
}

#[test]
fn session_locks_sanitizes_traversal() {
    let dirs = ProjectDirs::new("/project");
    let path = dirs.session_locks("../../../etc/passwd");
    assert_eq!(
        path,
        PathBuf::from("/project/.claude/runtime/locks/_________etc_passwd")
    );
}

#[test]
fn session_logs_sanitizes_traversal() {
    let dirs = ProjectDirs::new("/project");
    let path = dirs.session_logs("../../../etc/passwd");
    assert_eq!(
        path,
        PathBuf::from("/project/.claude/runtime/logs/_________etc_passwd")
    );
}

#[test]
fn session_power_steering_sanitizes_traversal() {
    let dirs = ProjectDirs::new("/project");
    let path = dirs.session_power_steering("../../../etc/passwd");
    assert_eq!(
        path,
        PathBuf::from("/project/.claude/runtime/power-steering/_________etc_passwd")
    );
}

#[test]
fn resolve_framework_file_prefers_src_amplihack_checkout() {
    let dir = temp_test_dir("src-amplihack");
    let project = dir.join("worktree").join("nested");
    let framework = dir.join("worktree").join("src").join("amplihack");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(framework.join(".claude/context")).unwrap();
    fs::write(
        framework.join(".claude/context/USER_PREFERENCES.md"),
        "verbosity = balanced",
    )
    .unwrap();

    let resolved = resolve_framework_file_from(&project, ".claude/context/USER_PREFERENCES.md");

    assert_eq!(
        resolved.as_deref(),
        Some(
            framework
                .join(".claude/context/USER_PREFERENCES.md")
                .as_path()
        )
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_framework_file_uses_amplihack_root_override() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = temp_test_dir("amplihack-root");
    let project = dir.join("project");
    let framework = dir.join("framework-root");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(framework.join(".claude/context")).unwrap();
    fs::write(
        framework.join(".claude/context/USER_PREFERENCES.md"),
        "verbosity = concise",
    )
    .unwrap();
    let previous = env::var_os("AMPLIHACK_ROOT");
    unsafe { env::set_var("AMPLIHACK_ROOT", &framework) };

    let resolved = resolve_framework_file_from(&project, ".claude/context/USER_PREFERENCES.md");

    match previous {
        Some(value) => unsafe { env::set_var("AMPLIHACK_ROOT", value) },
        None => unsafe { env::remove_var("AMPLIHACK_ROOT") },
    }

    assert_eq!(
        resolved.as_deref(),
        Some(
            framework
                .join(".claude/context/USER_PREFERENCES.md")
                .as_path()
        )
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_framework_file_rejects_path_traversal() {
    let dir = temp_test_dir("path-traversal");
    fs::create_dir_all(dir.join(".claude")).unwrap();

    assert!(resolve_framework_file_from(&dir, "../secret").is_none());
    assert!(resolve_framework_file_from(&dir, "/absolute/path").is_none());

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolve_workflow_file_supports_legacy_default_workflow_path_for_migration_only() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = temp_test_dir("legacy-workflow");
    let project = dir.join("project");
    let framework = dir.join("framework-root");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(framework.join(".claude/workflow")).unwrap();
    fs::write(
        framework.join(".claude/workflow/DEFAULT_WORKFLOW.md"),
        "# Legacy Default Workflow\n",
    )
    .unwrap();
    let previous = env::var_os("AMPLIHACK_ROOT");
    unsafe { env::set_var("AMPLIHACK_ROOT", &framework) };

    let resolved = ProjectDirs::new(&project).resolve_workflow_file();

    match previous {
        Some(value) => unsafe { env::set_var("AMPLIHACK_ROOT", value) },
        None => unsafe { env::remove_var("AMPLIHACK_ROOT") },
    }

    assert_eq!(
        resolved.as_deref(),
        Some(
            framework
                .join(".claude/workflow/DEFAULT_WORKFLOW.md")
                .as_path()
        )
    );

    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Issue #1963: the hook state root and per-session state directories.
//
// The resolver functions are pure, so most of these tests pass candidate
// values directly and touch neither the environment nor the file system.
// Only the `amplihack_state_root` tests read the environment, and they hold
// the crate's single `env_lock()` while they do.
// ---------------------------------------------------------------------------

mod state_root {
    use super::super::{
        UNKNOWN_SESSION_KEY, amplihack_state_root, resolve_state_root, session_state_dir,
        session_state_dir_under,
    };
    use crate::test_support::env_lock;
    use std::ffi::{OsStr, OsString};
    use std::path::{Component, Path, PathBuf};

    fn os(value: &str) -> Option<&OsStr> {
        Some(OsStr::new(value))
    }

    fn sessions_dir(root: &str) -> PathBuf {
        Path::new(root).join("runtime").join("sessions")
    }

    /// Restores one environment variable on drop. Callers hold `env_lock()`.
    struct Restore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl Restore {
        fn set(key: &'static str, value: Option<&OsStr>) -> Self {
            let previous = std::env::var_os(key);
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
            Self { key, previous }
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    // ---- resolve_state_root: AMPLIHACK_HOME ----

    #[test]
    fn absolute_amplihack_home_wins_over_home() {
        assert_eq!(
            resolve_state_root(os("/opt/amp"), os("/home/alice"), false),
            Some(PathBuf::from("/opt/amp"))
        );
    }

    #[test]
    fn absolute_amplihack_home_is_used_without_home() {
        assert_eq!(
            resolve_state_root(os("/opt/amp"), None, false),
            Some(PathBuf::from("/opt/amp"))
        );
    }

    #[test]
    fn absent_amplihack_home_falls_back_to_home_dot_amplihack() {
        assert_eq!(
            resolve_state_root(None, os("/home/alice"), false),
            Some(PathBuf::from("/home/alice/.amplihack"))
        );
    }

    #[test]
    fn empty_amplihack_home_falls_back_to_home() {
        assert_eq!(
            resolve_state_root(os(""), os("/home/alice"), false),
            Some(PathBuf::from("/home/alice/.amplihack"))
        );
    }

    #[test]
    fn whitespace_amplihack_home_falls_back_to_home() {
        assert_eq!(
            resolve_state_root(os("   "), os("/home/alice"), false),
            Some(PathBuf::from("/home/alice/.amplihack"))
        );
    }

    /// R1: values are never trimmed. A leading space makes the value relative.
    #[test]
    fn leading_space_amplihack_home_is_relative_and_skipped() {
        assert_eq!(
            resolve_state_root(os(" /opt/amp"), os("/home/alice"), false),
            Some(PathBuf::from("/home/alice/.amplihack"))
        );
    }

    /// R1: a trailing space is kept, not trimmed away.
    #[test]
    fn trailing_space_amplihack_home_is_used_as_written() {
        assert_eq!(
            resolve_state_root(os("/opt/amp "), os("/home/alice"), false),
            Some(PathBuf::from("/opt/amp "))
        );
    }

    #[test]
    fn relative_amplihack_home_falls_back_to_home() {
        for value in [".amplihack", "amp", "./amp", "../amplihack", "."] {
            assert_eq!(
                resolve_state_root(os(value), os("/home/alice"), false),
                Some(PathBuf::from("/home/alice/.amplihack")),
                "relative AMPLIHACK_HOME {value:?} must be ignored"
            );
        }
    }

    #[test]
    fn amplihack_home_with_parent_component_falls_back_to_home() {
        for value in ["/opt/amp/../etc", "/..", "/opt/..", "/a/b/../../c"] {
            assert_eq!(
                resolve_state_root(os(value), os("/home/alice"), false),
                Some(PathBuf::from("/home/alice/.amplihack")),
                "AMPLIHACK_HOME {value:?} has a `..` component and must be ignored"
            );
        }
    }

    #[test]
    fn amplihack_home_with_parent_component_and_no_home_is_none() {
        assert_eq!(resolve_state_root(os("/opt/amp/../etc"), None, false), None);
    }

    /// `..` inside a file name is not a parent component.
    #[test]
    fn dots_inside_a_component_are_not_parent_components() {
        assert_eq!(
            resolve_state_root(os("/opt/amp..x"), os("/home/alice"), false),
            Some(PathBuf::from("/opt/amp..x"))
        );
    }

    // ---- resolve_state_root: HOME ----

    #[test]
    fn unusable_home_without_amplihack_home_is_none() {
        for home in [
            None,
            os(""),
            os("   "),
            os("relative/home"),
            os("/home/../root"),
        ] {
            assert_eq!(
                resolve_state_root(None, home, false),
                None,
                "HOME {home:?} must not produce a state root"
            );
        }
    }

    #[test]
    fn relative_amplihack_home_and_relative_home_is_none() {
        assert_eq!(resolve_state_root(os("amp"), os("home"), false), None);
    }

    /// The working directory and `.` are never a state root.
    #[test]
    fn neither_root_never_resolves_to_cwd_or_dot() {
        let root = resolve_state_root(None, None, false);
        assert_eq!(root, None);
        let root = resolve_state_root(os("."), os("."), false);
        assert_eq!(root, None);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_absolute_amplihack_home_passes_unchanged() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"/opt/amp-\xff\xfe");
        assert_eq!(
            resolve_state_root(Some(raw), os("/home/alice"), false),
            Some(PathBuf::from(raw))
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_absolute_home_passes_unchanged() {
        use std::os::unix::ffi::OsStrExt;
        let raw = OsStr::from_bytes(b"/home/\xff");
        assert_eq!(
            resolve_state_root(None, Some(raw), false),
            Some(PathBuf::from(raw).join(".amplihack"))
        );
    }

    // ---- amplihack_state_root: reads the environment ----

    #[test]
    fn amplihack_state_root_reads_amplihack_home_first() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _amp = Restore::set("AMPLIHACK_HOME", os("/opt/amp-env"));
        let _home = Restore::set("HOME", os("/home/env-user"));
        assert_eq!(amplihack_state_root(), Some(PathBuf::from("/opt/amp-env")));
    }

    #[test]
    fn amplihack_state_root_falls_back_to_home() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _amp = Restore::set("AMPLIHACK_HOME", None);
        let _home = Restore::set("HOME", os("/home/env-user"));
        assert_eq!(
            amplihack_state_root(),
            Some(PathBuf::from("/home/env-user/.amplihack"))
        );
    }

    #[test]
    fn amplihack_state_root_ignores_relative_amplihack_home() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _amp = Restore::set("AMPLIHACK_HOME", os("relative/amp"));
        let _home = Restore::set("HOME", os("/home/env-user"));
        assert_eq!(
            amplihack_state_root(),
            Some(PathBuf::from("/home/env-user/.amplihack"))
        );
    }

    #[test]
    fn amplihack_state_root_is_none_without_usable_values() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _amp = Restore::set("AMPLIHACK_HOME", None);
        let _home = Restore::set("HOME", None);
        assert_eq!(amplihack_state_root(), None);
        assert_eq!(session_state_dir(Some("abc")), None);
        assert_eq!(session_state_dir(None), None);
    }

    #[test]
    fn session_state_dir_joins_the_resolved_root() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _amp = Restore::set("AMPLIHACK_HOME", None);
        let _home = Restore::set("HOME", os("/home/env-user"));
        assert_eq!(
            session_state_dir(Some("demo-1")),
            Some(PathBuf::from(
                "/home/env-user/.amplihack/runtime/sessions/demo-1"
            ))
        );
        assert_eq!(
            session_state_dir(None),
            Some(sessions_dir("/home/env-user/.amplihack").join(UNKNOWN_SESSION_KEY))
        );
    }

    // ---- session_state_dir_under: session keys ----

    #[test]
    fn unknown_session_key_is_documented_value() {
        assert_eq!(UNKNOWN_SESSION_KEY, "unknown-session");
    }

    #[test]
    fn plain_session_id_is_used_as_key() {
        assert_eq!(
            session_state_dir_under(Path::new("/r"), Some("demo-1")),
            PathBuf::from("/r/runtime/sessions/demo-1")
        );
    }

    #[test]
    fn session_keys_match_the_documented_table() {
        let cases: &[(&str, &str)] = &[
            ("demo-1", "demo-1"),
            ("lane 3", "lane_3"),
            (" a ", "_a_"),
            ("..", "__"),
            (".", "_"),
            ("../../escape", "______escape"),
            ("/abs", "_abs"),
            ("a/b", "a_b"),
            ("x\0y", "x_y"),
            ("a\\b", "a_b"),
        ];
        for (id, key) in cases {
            assert_eq!(
                session_state_dir_under(Path::new("/r"), Some(id)),
                sessions_dir("/r").join(key),
                "session id {id:?} must map to key {key:?}"
            );
        }
    }

    #[test]
    fn absent_empty_or_blank_session_id_uses_unknown_session() {
        for id in [None, Some(""), Some("  "), Some("\t\n")] {
            assert_eq!(
                session_state_dir_under(Path::new("/r"), id),
                sessions_dir("/r").join(UNKNOWN_SESSION_KEY),
                "session id {id:?} must use the fallback key"
            );
        }
    }

    /// The fallback key goes through the same function, so a caller that
    /// passes its own fallback (the workflow `current` key) gets one component.
    #[test]
    fn current_fallback_key_is_one_component() {
        assert_eq!(
            session_state_dir_under(Path::new("/r"), Some("current")),
            sessions_dir("/r").join("current")
        );
    }

    #[test]
    fn hostile_ids_stay_directly_under_sessions() {
        let long = "a".repeat(300);
        let hostile = [
            "../../escape",
            "/abs",
            "a/b",
            "..",
            ".",
            "x\0y",
            "/",
            "//",
            "\\..\\..\\x",
            "C:\\Windows",
            "~",
            "$HOME",
            "é/ü",
            long.as_str(),
        ];
        let root = Path::new("/r");
        for id in hostile {
            let dir = session_state_dir_under(root, Some(id));
            assert_eq!(
                dir.parent(),
                Some(sessions_dir("/r").as_path()),
                "id {id:?} escaped runtime/sessions: {}",
                dir.display()
            );
            let name = dir.file_name().expect("one key component");
            let name = name.to_str().expect("key is ASCII");
            assert!(!name.is_empty(), "id {id:?} produced an empty key");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "id {id:?} produced key {name:?} outside [A-Za-z0-9_-]"
            );
            assert!(
                dir.components().all(|c| !matches!(c, Component::ParentDir)),
                "id {id:?} produced a `..` component"
            );
        }
    }

    /// No length cap: an over-long id gives an over-long key, and the write
    /// fails later instead of the resolver panicking or truncating.
    #[test]
    fn long_id_is_not_truncated_and_does_not_panic() {
        let long = "b".repeat(300);
        let dir = session_state_dir_under(Path::new("/r"), Some(&long));
        assert_eq!(dir, sessions_dir("/r").join(&long));
    }

    /// Re-exports from the crate root, next to `sanitize_session_id`.
    #[test]
    fn resolver_api_is_reexported_from_crate_root() {
        assert_eq!(crate::UNKNOWN_SESSION_KEY, "unknown-session");
        assert_eq!(
            crate::resolve_state_root(None, os("/h"), false),
            Some(PathBuf::from("/h/.amplihack"))
        );
        assert_eq!(
            crate::session_state_dir_under(Path::new("/r"), None),
            sessions_dir("/r").join("unknown-session")
        );
        let _: fn() -> Option<PathBuf> = crate::amplihack_state_root;
        let _: fn(Option<&str>) -> Option<PathBuf> = crate::session_state_dir;
    }
}

// ---------------------------------------------------------------------------
// Issue #2528 (D3): a pinned recipe run sets AMPLIHACK_HOME to its bundle
// snapshot and passes the real state root in AMPLIHACK_STATE_HOME. State
// readers take AMPLIHACK_STATE_HOME first when it is usable (absolute, no
// `..`), and otherwise fall back silently to their usual rules.
// ---------------------------------------------------------------------------

mod state_home_2528 {
    use super::super::amplihack_state_root;
    use crate::test_support::env_lock;
    use std::ffi::OsString;
    use std::path::PathBuf;

    struct Restore(Vec<(&'static str, Option<OsString>)>);

    impl Restore {
        fn set(values: &[(&'static str, Option<&str>)]) -> Self {
            let previous = values
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect();
            for (key, value) in values {
                match value {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
            Self(previous)
        }
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    fn root_with(state_home: Option<&str>) -> Option<PathBuf> {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _env = Restore::set(&[
            ("AMPLIHACK_STATE_HOME", state_home),
            ("AMPLIHACK_HOME", Some("/runs/r/runtime/bundle")),
            ("HOME", Some("/home/alice")),
        ]);
        amplihack_state_root()
    }

    #[test]
    fn state_home_is_read_first() {
        assert_eq!(
            root_with(Some("/home/alice/.amplihack")),
            Some(PathBuf::from("/home/alice/.amplihack"))
        );
    }

    #[test]
    fn unusable_state_home_is_ignored_without_error() {
        for value in ["", "relative/state", "/home/alice/../bob/.amplihack"] {
            assert_eq!(
                root_with(Some(value)),
                Some(PathBuf::from("/runs/r/runtime/bundle")),
                "{value:?} is not usable; the usual rules apply"
            );
        }
    }

    #[test]
    fn unset_state_home_keeps_the_usual_rules() {
        assert_eq!(
            root_with(None),
            Some(PathBuf::from("/runs/r/runtime/bundle"))
        );
    }
}
