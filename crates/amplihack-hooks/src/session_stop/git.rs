//! Git status checking and uncommitted work warnings.

use std::process::Command;

struct GitStatus {
    staged: Vec<String>,
    unstaged: Vec<String>,
    untracked: Vec<String>,
}

fn get_git_status() -> Option<GitStatus> {
    let staged = match Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => return None,
    };

    let unstaged = match Command::new("git").args(["diff", "--name-only"]).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let untracked = match Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

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
