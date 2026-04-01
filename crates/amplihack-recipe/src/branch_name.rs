//! Branch name sanitization for git worktrees.
//!
//! Matches the sanitization pipeline from Python's
//! `tests/test_branch_name_sanitization.py` and
//! `default-workflow.yaml` step-04-setup-worktree:
//!
//! 1. Replace newlines with spaces
//! 2. Strip leading/trailing whitespace
//! 3. Lowercase
//! 4. Replace invalid chars with hyphens
//! 5. Collapse consecutive hyphens
//! 6. Truncate to 60 chars
//! 7. Strip trailing hyphens/dots

/// Maximum length for a sanitized branch name.
const MAX_BRANCH_LEN: usize = 60;

/// Sanitize a task description into a valid git branch name.
///
/// Reproduces the exact pipeline from default-workflow.yaml step-04.
pub fn sanitize_branch_name(task_desc: &str) -> String {
    // 1. Replace newlines/carriage returns with spaces
    let s = task_desc.replace(['\n', '\r'], " ");

    // 2. Strip leading/trailing whitespace
    let s = s.trim();

    // 3. Lowercase
    let s = s.to_lowercase();

    // 4. Replace invalid git ref chars with hyphens
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    // 5. Collapse consecutive hyphens
    let mut result = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // 6. Truncate to MAX_BRANCH_LEN
    let result = if result.len() > MAX_BRANCH_LEN {
        result[..MAX_BRANCH_LEN].to_string()
    } else {
        result
    };

    // 7. Strip trailing hyphens and dots
    result.trim_end_matches(['-', '.']).to_string()
}

/// Create a full branch name with prefix.
pub fn make_branch_name(prefix: &str, task_desc: &str) -> String {
    let sanitized = sanitize_branch_name(task_desc);
    if sanitized.is_empty() {
        format!("{prefix}/unnamed")
    } else {
        format!("{prefix}/{sanitized}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_description() {
        assert_eq!(
            sanitize_branch_name("Fix the login bug"),
            "fix-the-login-bug"
        );
    }

    #[test]
    fn multiline_description() {
        assert_eq!(
            sanitize_branch_name("Fix the\nlogin bug\nplease"),
            "fix-the-login-bug-please"
        );
    }

    #[test]
    fn special_characters() {
        assert_eq!(
            sanitize_branch_name("feat: add @user/auth (v2)"),
            "feat-add-user-auth-v2"
        );
    }

    #[test]
    fn consecutive_hyphens_collapsed() {
        assert_eq!(
            sanitize_branch_name("a --- b --- c"),
            "a-b-c"
        );
    }

    #[test]
    fn long_description_truncated() {
        let long = "a".repeat(100);
        let result = sanitize_branch_name(&long);
        assert!(result.len() <= MAX_BRANCH_LEN);
    }

    #[test]
    fn trailing_hyphens_stripped() {
        assert_eq!(
            sanitize_branch_name("fix trailing---"),
            "fix-trailing"
        );
    }

    #[test]
    fn trailing_dots_stripped() {
        assert_eq!(
            sanitize_branch_name("version 1.0."),
            "version-1.0"
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(sanitize_branch_name(""), "");
    }

    #[test]
    fn whitespace_only() {
        assert_eq!(sanitize_branch_name("   "), "");
    }

    #[test]
    fn unicode_replaced() {
        assert_eq!(
            sanitize_branch_name("日本語テスト"),
            ""  // all non-ascii → hyphens → collapsed → stripped
        );
    }

    #[test]
    fn make_branch_name_with_prefix() {
        assert_eq!(
            make_branch_name("feat", "Add user auth"),
            "feat/add-user-auth"
        );
    }

    #[test]
    fn make_branch_name_empty_desc() {
        assert_eq!(
            make_branch_name("fix", ""),
            "fix/unnamed"
        );
    }

    #[test]
    fn preserves_dots_and_underscores() {
        assert_eq!(
            sanitize_branch_name("v1.0_release"),
            "v1.0_release"
        );
    }

    #[test]
    fn truncation_respects_trailing_strip() {
        // 60 chars ending in hyphens should be stripped
        let desc = format!("{}---", "x".repeat(58));
        let result = sanitize_branch_name(&desc);
        assert!(!result.ends_with('-'));
        assert!(result.len() <= MAX_BRANCH_LEN);
    }
}
