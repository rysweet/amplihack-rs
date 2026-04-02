use super::*;
use std::fs;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn temp_test_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
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
    assert_eq!(sanitize_session_id("../../../etc/passwd"), "etcpasswd");
}

#[test]
fn sanitize_strips_forward_slashes() {
    assert_eq!(sanitize_session_id("foo/bar"), "foobar");
}

#[test]
fn sanitize_strips_backslashes() {
    assert_eq!(sanitize_session_id("foo\\bar"), "foobar");
}

#[test]
fn sanitize_strips_mixed_traversal() {
    assert_eq!(
        sanitize_session_id("..\\..\\windows\\system32"),
        "windowssystem32"
    );
}

#[test]
#[should_panic(expected = "session_id is empty after sanitization")]
fn sanitize_rejects_empty_result() {
    sanitize_session_id("../../../");
}

#[test]
fn session_locks_sanitizes_traversal() {
    let dirs = ProjectDirs::new("/project");
    let path = dirs.session_locks("../../../etc/passwd");
    assert_eq!(
        path,
        PathBuf::from("/project/.claude/runtime/locks/etcpasswd")
    );
}

#[test]
fn session_logs_sanitizes_traversal() {
    let dirs = ProjectDirs::new("/project");
    let path = dirs.session_logs("../../../etc/passwd");
    assert_eq!(
        path,
        PathBuf::from("/project/.claude/runtime/logs/etcpasswd")
    );
}

#[test]
fn session_power_steering_sanitizes_traversal() {
    let dirs = ProjectDirs::new("/project");
    let path = dirs.session_power_steering("../../../etc/passwd");
    assert_eq!(
        path,
        PathBuf::from("/project/.claude/runtime/power-steering/etcpasswd")
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
