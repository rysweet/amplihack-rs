//! Tests for CWD protection.

use super::*;
use crate::test_support::env_lock;
use std::path::PathBuf;

#[test]
fn safe_rm_not_blocked() {
    let result = check_cwd_deletion("rm file.txt").unwrap();
    assert!(result.is_none());
}

#[test]
fn rm_rf_nonexistent_not_blocked() {
    let result = check_cwd_deletion("rm -rf /nonexistent/path/xyz").unwrap();
    assert!(result.is_none());
}

#[test]
fn rm_rf_cwd_blocked() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let cwd = std::env::current_dir().unwrap();
    let cmd = format!("rm -rf {}", cwd.display());
    let result = check_cwd_deletion(&cmd).unwrap();
    assert!(result.is_some());
    let block = result.unwrap();
    assert_eq!(block["block"], true);
    assert!(
        block["message"]
            .as_str()
            .unwrap()
            .contains("Working Directory Deletion Prevented")
    );
}

#[test]
fn mv_safe_not_blocked() {
    let result = check_cwd_rename("mv /tmp/nonexistent_a /tmp/nonexistent_b").unwrap();
    assert!(result.is_none());
}

#[test]
fn mv_cwd_blocked() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let cwd = std::env::current_dir().unwrap();
    let cmd = format!("mv {} /tmp/new_name", cwd.display());
    let result = check_cwd_rename(&cmd).unwrap();
    assert!(result.is_some());
    let block = result.unwrap();
    assert_eq!(block["block"], true);
}

#[test]
fn is_path_under_works() {
    assert!(is_path_under(Path::new("/a/b/c"), Path::new("/a/b")));
    assert!(is_path_under(Path::new("/a/b"), Path::new("/a/b")));
    assert!(!is_path_under(Path::new("/a/b"), Path::new("/a/b/c")));
    assert!(!is_path_under(Path::new("/x/y"), Path::new("/a/b")));
}

#[test]
fn rmdir_cwd_blocked() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let cwd = std::env::current_dir().unwrap();
    let cmd = format!("rmdir {}", cwd.display());
    let result = check_cwd_deletion(&cmd).unwrap();
    assert!(result.is_some());
}

#[test]
fn rm_rf_parent_of_cwd_blocked() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let cwd = std::env::current_dir().unwrap();
    if let Some(parent) = cwd.parent()
        && parent != Path::new("/")
    {
        let cmd = format!("rm -rf {}", parent.display());
        let result = check_cwd_deletion(&cmd).unwrap();
        assert!(result.is_some());
    }
}

#[test]
fn rm_rf_unrelated_dir_not_blocked() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::env::current_dir().unwrap();
    if !cwd.starts_with(dir.path()) {
        let cmd = format!("rm -rf {}", dir.path().display());
        let result = check_cwd_deletion(&cmd).unwrap();
        assert!(result.is_none());
    }
}

#[test]
fn blocks_command_substitution() {
    let result = check_cwd_deletion(r#"rm -rf "$(pwd)""#).unwrap();
    assert!(result.is_some());
    assert!(
        result.unwrap()["message"]
            .as_str()
            .unwrap()
            .contains("Shell Expansion")
    );
}

#[test]
fn blocks_backtick_substitution() {
    let result = check_cwd_deletion("rm -rf `pwd`").unwrap();
    assert!(result.is_some());
}

#[test]
fn blocks_variable_expansion() {
    let result = check_cwd_deletion("rm -rf $HOME/dir").unwrap();
    assert!(result.is_some());
}

#[test]
fn blocks_brace_variable_expansion() {
    let result = check_cwd_deletion("rm -rf ${PWD}").unwrap();
    assert!(result.is_some());
}

#[test]
fn allows_dollar_status_codes() {
    let result = check_cwd_deletion("rm -rf /tmp/test_dir").unwrap();
    assert!(result.is_none());
}

#[test]
fn dangerous_expansion_detection() {
    assert!(has_dangerous_expansion("$(pwd)"));
    assert!(has_dangerous_expansion("`pwd`"));
    assert!(has_dangerous_expansion("$HOME"));
    assert!(has_dangerous_expansion("${PWD}"));
    assert!(has_dangerous_expansion("$D"));
    assert!(!has_dangerous_expansion("/tmp/test"));
    assert!(!has_dangerous_expansion("$?"));
    assert!(!has_dangerous_expansion("$!"));
}

#[test]
fn tilde_resolves_to_home() {
    let home = std::env::var("HOME").unwrap();
    let resolved = resolve_path("~").unwrap();
    assert_eq!(
        resolved,
        Path::new(&home)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&home))
    );
}

#[test]
fn tilde_slash_resolves_to_home_subpath() {
    let home = std::env::var("HOME").unwrap();
    let resolved = resolve_path("~/Documents").unwrap();
    let expected = Path::new(&home).join("Documents");
    assert!(
        resolved == expected.canonicalize().unwrap_or_else(|_| expected.clone()),
        "~/Documents should resolve under $HOME, got: {:?}",
        resolved
    );
}

#[test]
fn tilde_other_user_not_expanded() {
    let resolved = resolve_path("~other_user");
    if let Some(r) = &resolved {
        let home = std::env::var("HOME").unwrap();
        assert!(
            !r.starts_with(&home) || r.starts_with(std::env::current_dir().unwrap()),
            "~other_user should not expand to $HOME, got: {:?}",
            r
        );
    }
}

#[test]
fn rm_rf_tilde_blocked() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::var("HOME").unwrap();
    let cwd = std::env::current_dir().unwrap();
    if cwd.starts_with(&home) {
        let result = check_cwd_deletion("rm -rf ~").unwrap();
        assert!(
            result.is_some(),
            "rm -rf ~ should be blocked when CWD is under $HOME"
        );
    }
}

#[test]
fn rm_rf_tilde_slash_blocked() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::var("HOME").unwrap();
    let cwd = std::env::current_dir().unwrap();
    if cwd.starts_with(&home) {
        let result = check_cwd_deletion("rm -rf ~/").unwrap();
        assert!(
            result.is_some(),
            "rm -rf ~/ should be blocked when CWD is under $HOME"
        );
    }
}

#[test]
fn mv_tilde_blocked() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::var("HOME").unwrap();
    let cwd = std::env::current_dir().unwrap();
    if cwd.starts_with(&home) {
        let result = check_cwd_rename("mv ~ /tmp/x").unwrap();
        assert!(
            result.is_some(),
            "mv ~ /tmp/x should be blocked when CWD is under $HOME"
        );
    }
}
