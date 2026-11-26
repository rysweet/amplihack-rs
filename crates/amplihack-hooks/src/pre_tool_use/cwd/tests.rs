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
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = std::env::var("HOME").unwrap();
    let resolved = resolve_path("~/Documents").unwrap();
    let expected = Path::new(&home).join("Documents");
    assert!(
        resolved == expected.canonicalize().unwrap_or_else(|_| expected.clone()),
        "~/Documents should resolve under $HOME, got: {resolved:?}"
    );
}

#[test]
fn tilde_other_user_not_expanded() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let resolved = resolve_path("~other_user");
    if let Some(r) = &resolved {
        let home = std::env::var("HOME").unwrap();
        assert!(
            !r.starts_with(&home) || r.starts_with(std::env::current_dir().unwrap()),
            "~other_user should not expand to $HOME, got: {r:?}"
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

// Issue #3261: each removal is decided on its own simple command.

/// Runs `check_cwd_deletion` on `command` with the working directory set to
/// a fresh scratch directory, which `{cwd}` in `command` is replaced with.
fn decide_in_scratch(command: &str) -> Option<Value> {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let dir = crate::test_support::isolated_tempdir();
    let _cwd = crate::test_support::CwdGuard::set(dir.path()).unwrap();
    let cwd = current_cwd().unwrap();
    let command = command.replace("{cwd}", &cwd.display().to_string());
    check_cwd_deletion(&command).unwrap()
}

#[test]
fn heredoc_body_mentioning_removal_is_allowed() {
    let command = "cat > notes.md <<'EOF'\nTo clean up, run rm -rf . and then `ls`.\nEOF\n\
                   gh issue create --body-file notes.md --title \"$HOME\"";
    assert!(decide_in_scratch(command).is_none());
}

#[test]
fn unquoted_heredoc_body_mentioning_removal_is_allowed() {
    let command = "cat > notes.md <<EOF\nrm -rf {cwd}\nhome is $HOME\nEOF";
    assert!(decide_in_scratch(command).is_none());
}

#[test]
fn heredoc_in_substitution_mentioning_removal_is_allowed() {
    let command = "gh issue create --body \"$(cat <<'EOF'\nThe hook rejects rm -rf . today.\nEOF\n)\" \
                   --repo \"$HOME/x\"";
    assert!(decide_in_scratch(command).is_none());
}

#[test]
fn quoted_heredoc_substitution_text_is_allowed() {
    assert!(decide_in_scratch("cat <<'EOF'\n$(rm -rf .)\nEOF").is_none());
}

#[test]
fn quoted_string_mentioning_removal_is_allowed() {
    assert!(decide_in_scratch("echo 'rm -rf .'; ls $HOME").is_none());
    assert!(
        decide_in_scratch("gh issue create --body \"Avoid rm -r . here\" --title `cat t`")
            .is_none()
    );
}

#[test]
fn comment_mentioning_removal_is_allowed() {
    assert!(decide_in_scratch("ls $HOME # rm -rf .").is_none());
}

#[test]
fn literal_removal_after_command_with_variable_is_decided_on_its_own_path() {
    let outside = "cd \"$DIR\" && git status; rm -r /nonexistent/issue-3261/build";
    assert!(decide_in_scratch(outside).is_none());
    let cwd = "cd \"$DIR\" && git status; rm -r {cwd}";
    assert!(decide_in_scratch(cwd).is_some());
}

#[test]
fn removal_with_expansion_in_its_own_arguments_is_rejected() {
    for command in [
        "rm -rf $VAR/x",
        "rm -rf ${VAR}",
        "rm -rf $(pwd)",
        "rm -rf `pwd`",
        "ls; rm -rf \"$VAR\"",
        "rmdir $D",
    ] {
        assert!(decide_in_scratch(command).is_some(), "{command}");
    }
}

#[test]
fn nested_removals_of_cwd_are_rejected() {
    for command in [
        "echo $(rm -rf .)",
        "bash -c 'rm -rf .'",
        "eval \"rm -rf .\"",
        "find /nonexistent -exec rm -rf {cwd} \\;",
        "xargs -0 rm -rf . < list",
        "sudo ionice -c2 rm -rf .",
        "bash <<'EOF'\nrm -rf .\nEOF",
        "cat <<EOF\n$(rm -rf .)\nEOF",
        "bash -c 'rm -rf \"$X\"'",
    ] {
        assert!(decide_in_scratch(command).is_some(), "{command}");
    }
}

#[test]
fn recursive_flag_after_operand_is_read() {
    assert!(decide_in_scratch("rm . -rf").is_some());
    assert!(decide_in_scratch("rm -- -r").is_none());
}

#[test]
fn removal_outside_cwd_is_allowed() {
    assert!(decide_in_scratch("rm -rf /nonexistent/issue-3261/a; echo $HOME").is_none());
    assert!(decide_in_scratch("find /nonexistent/issue-3261 -exec rm -rf {} +").is_none());
}

#[test]
fn unreadable_input_is_decided_on_the_whole_text() {
    assert!(decide_in_scratch("echo \"unclosed; rm -rf $X").is_some());
    assert!(decide_in_scratch("cat <<EOF\nrm -rf $X").is_some());
}
