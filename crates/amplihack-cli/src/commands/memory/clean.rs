//! `memory clean` command implementation.

use super::*;
use crate::command_error::exit_error;
use anyhow::Result;
use std::io::{self, Write};

pub fn run_clean(pattern: &str, backend: &str, dry_run: bool, confirm: bool) -> Result<()> {
    let backend = BackendChoice::parse(backend)?;
    let matched = match backend {
        BackendChoice::Sqlite => list_sqlite_sessions()?,
        BackendChoice::Kuzu => list_kuzu_sessions()?,
    }
    .into_iter()
    .filter(|session| wildcard_match(pattern, &session.session_id))
    .collect::<Vec<_>>();

    if matched.is_empty() {
        return Ok(());
    }

    print!(
        "\nFound {} session(s) matchin' pattern '{}':\n",
        matched.len(),
        pattern
    );
    for session in &matched {
        println!(
            "  - {} ({} memories)",
            session.session_id, session.memory_count
        );
    }

    if dry_run {
        println!("\nDry-run mode: No sessions were deleted.");
        println!("Use --no-dry-run to actually be deletin' these sessions.");
        return Ok(());
    }

    if !confirm {
        print!("\nAre ye sure ye want to delete these sessions? [y/N]: ");
        io::stdout().flush()?;
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        let normalized = response.trim().to_ascii_lowercase();
        if normalized != "y" && normalized != "yes" {
            println!("Cleanup be cancelled.");
            return Ok(());
        }
    }

    let mut deleted_count = 0usize;
    let mut error_count = 0usize;
    for session in &matched {
        let deleted = match backend {
            BackendChoice::Sqlite => delete_sqlite_session(&session.session_id),
            BackendChoice::Kuzu => delete_kuzu_session(&session.session_id),
        };
        match deleted {
            Ok(true) => {
                deleted_count += 1;
                println!("Deleted: {}", session.session_id);
            }
            Ok(false) => {
                error_count += 1;
                writeln!(
                    io::stderr(),
                    "Failed to be deletin': {}",
                    session.session_id
                )?;
            }
            Err(error) => {
                error_count += 1;
                writeln!(
                    io::stderr(),
                    "Error deletin' {}: {error}",
                    session.session_id
                )?;
            }
        }
    }

    print!(
        "\nCleanup complete: {} deleted, {} errors\n",
        deleted_count, error_count
    );
    if error_count > 0 {
        return Err(exit_error(1));
    }
    Ok(())
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern_chars = pattern.as_bytes();
    let value_chars = value.as_bytes();
    let mut dp = vec![vec![false; value_chars.len() + 1]; pattern_chars.len() + 1];
    dp[0][0] = true;
    for i in 1..=pattern_chars.len() {
        if pattern_chars[i - 1] == b'*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=pattern_chars.len() {
        for j in 1..=value_chars.len() {
            dp[i][j] = match pattern_chars[i - 1] {
                b'*' => dp[i - 1][j] || dp[i][j - 1],
                b'?' => dp[i - 1][j - 1],
                current => dp[i - 1][j - 1] && current == value_chars[j - 1],
            };
        }
    }
    dp[pattern_chars.len()][value_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching_supports_globs() {
        assert!(wildcard_match("test_*", "test_session"));
        assert!(wildcard_match("dev_?", "dev_a"));
        assert!(!wildcard_match("dev_?", "dev_ab"));
        assert!(!wildcard_match("demo_*", "test_session"));
    }
}
