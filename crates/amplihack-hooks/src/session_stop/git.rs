//! Git status checking and uncommitted work warnings.

use std::process::Command;
use std::time::{Duration, Instant};

const GIT_TIMEOUT: Duration = Duration::from_secs(5);

struct GitStatus {
    staged: Vec<String>,
    unstaged: Vec<String>,
    untracked: Vec<String>,
}

/// Run a git command with a timeout, returning its stdout on success.
fn run_git_with_timeout(args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + GIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let output = child.wait_with_output().ok()?;
                return Some(String::from_utf8_lossy(&output.stdout).into_owned());
            }
            Ok(Some(_)) => return None, // non-zero exit
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

fn parse_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn get_git_status() -> Option<GitStatus> {
    let staged = parse_lines(&run_git_with_timeout(&["diff", "--cached", "--name-only"])?);

    let unstaged = run_git_with_timeout(&["diff", "--name-only"])
        .map(|o| parse_lines(&o))
        .unwrap_or_default();

    let untracked = run_git_with_timeout(&["ls-files", "--others", "--exclude-standard"])
        .map(|o| parse_lines(&o))
        .unwrap_or_default();

    Some(GitStatus {
        staged,
        unstaged,
        untracked,
    })
}

/// Check git status and print warnings about uncommitted changes.
///
/// Best-effort: never blocks session exit.
pub(super) fn warn_uncommitted_work() {
    let status = match get_git_status() {
        Some(s) => s,
        None => return,
    };

    let GitStatus {
        staged,
        unstaged,
        untracked,
    } = status;

    if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() {
        return;
    }

    eprintln!("\n⚠️  Uncommitted work detected:");

    if !staged.is_empty() {
        eprintln!(
            "\n  Staged ({} file{}):",
            staged.len(),
            if staged.len() == 1 { "" } else { "s" }
        );
        for f in staged.iter().take(10) {
            eprintln!("    ✅ {f}");
        }
        if staged.len() > 10 {
            eprintln!("    ... and {} more", staged.len() - 10);
        }
    }

    if !unstaged.is_empty() {
        eprintln!(
            "\n  Modified ({} file{}):",
            unstaged.len(),
            if unstaged.len() == 1 { "" } else { "s" }
        );
        for f in unstaged.iter().take(10) {
            eprintln!("    📝 {f}");
        }
        if unstaged.len() > 10 {
            eprintln!("    ... and {} more", unstaged.len() - 10);
        }
    }

    if !untracked.is_empty() {
        eprintln!(
            "\n  Untracked ({} file{}):",
            untracked.len(),
            if untracked.len() == 1 { "" } else { "s" }
        );
        for f in untracked.iter().take(10) {
            eprintln!("    ❓ {f}");
        }
        if untracked.len() > 10 {
            eprintln!("    ... and {} more", untracked.len() - 10);
        }
    }

    let total = staged.len() + unstaged.len() + untracked.len();
    eprintln!("  💡 To commit: git add -A && git commit -m \"save work\"");
    eprintln!("  💡 To stash:  git stash push -m \"session work\"");
    eprintln!(
        "  📊 Total: {total} file{} with uncommitted changes\n",
        if total == 1 { "" } else { "s" }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_lines ---

    #[test]
    fn parse_lines_basic() {
        let output = "file1.rs\nfile2.rs\nfile3.rs\n";
        let result = parse_lines(output);
        assert_eq!(result, vec!["file1.rs", "file2.rs", "file3.rs"]);
    }

    #[test]
    fn parse_lines_filters_empty() {
        let output = "a\n\n\nb\n";
        assert_eq!(parse_lines(output), vec!["a", "b"]);
    }

    #[test]
    fn parse_lines_empty_input() {
        assert!(parse_lines("").is_empty());
        assert!(parse_lines("\n\n").is_empty());
    }

    #[test]
    fn parse_lines_no_trailing_newline() {
        assert_eq!(parse_lines("only-one"), vec!["only-one"]);
    }

    // --- GitStatus formatting ---

    #[test]
    fn git_status_struct_fields() {
        let status = GitStatus {
            staged: vec!["a.rs".into()],
            unstaged: vec!["b.rs".into(), "c.rs".into()],
            untracked: vec![],
        };
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.unstaged.len(), 2);
        assert!(status.untracked.is_empty());
    }

    // --- GIT_TIMEOUT constant ---

    #[test]
    fn git_timeout_is_5_seconds() {
        assert_eq!(GIT_TIMEOUT, Duration::from_secs(5));
    }

    // --- run_git_with_timeout ---

    #[test]
    fn run_git_version_succeeds() {
        // git --version should always work in CI
        let result = run_git_with_timeout(&["--version"]);
        assert!(result.is_some());
        assert!(result.unwrap().contains("git version"));
    }

    #[test]
    fn run_git_bad_command_returns_none() {
        // A git command that fails with non-zero exit
        let result = run_git_with_timeout(&["log", "--not-a-real-flag"]);
        assert!(result.is_none());
    }
}
