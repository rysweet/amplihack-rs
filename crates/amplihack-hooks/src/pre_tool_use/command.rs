//! Shell command parsing and segmentation.
//!
//! Splits commands on separators (;, &&, ||) and extracts tokens
//! using shell-words for proper quote handling.

/// Split a command string on shell separators (;, &&, ||).
/// Does NOT split on single pipe (|) — that's for piping, not chaining.
pub fn split_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = command.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < len {
        let c = chars[i];

        // Track quote state.
        if c == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(c);
            i += 1;
            continue;
        }
        if c == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(c);
            i += 1;
            continue;
        }

        // Only split outside of quotes.
        if !in_single_quote && !in_double_quote {
            // Check for && or ||
            if i + 1 < len {
                let next = chars[i + 1];
                if (c == '&' && next == '&') || (c == '|' && next == '|') {
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        segments.push(trimmed);
                    }
                    current.clear();
                    i += 2;
                    continue;
                }
            }
            // Check for ;
            if c == ';' {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    segments.push(trimmed);
                }
                current.clear();
                i += 1;
                continue;
            }
        }

        current.push(c);
        i += 1;
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        segments.push(trimmed);
    }

    segments
}

/// Extract path arguments from an rm or rmdir command segment.
///
/// Uses shell-words for proper quote handling.
/// Skips flags (tokens starting with -) and the command name itself.
pub fn extract_rm_paths(segment: &str) -> Vec<String> {
    let tokens = match shell_words::split(segment) {
        Ok(t) => t,
        Err(_) => segment.split_whitespace().map(String::from).collect(),
    };

    let mut paths = Vec::new();
    let mut found_command = false;

    for token in &tokens {
        if !found_command {
            if token == "rm"
                || token == "rmdir"
                || token.ends_with("/rm")
                || token.ends_with("/rmdir")
            {
                found_command = true;
            }
            continue;
        }

        // Skip flags.
        if token.starts_with('-') {
            continue;
        }

        paths.push(token.clone());
    }

    paths
}

/// Extract source paths from an mv command segment.
///
/// Supports both standard and -t/--target-directory forms:
/// - `mv src1 src2 dest/`  → sources: [src1, src2]
/// - `mv -t dest/ src1 src2`  → sources: [src1, src2]
pub fn extract_mv_source_paths(segment: &str) -> Vec<String> {
    let tokens = match shell_words::split(segment) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    // Find the mv command position.
    let mv_index = tokens.iter().position(|t| t == "mv" || t.ends_with("/mv"));
    let mv_index = match mv_index {
        Some(i) => i,
        None => return Vec::new(),
    };

    let args = &tokens[mv_index + 1..];
    let mut non_flag_args = Vec::new();
    let mut target_dir_mode = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        // End of options marker.
        if arg == "--" {
            non_flag_args.extend(args[i + 1..].iter().cloned());
            break;
        }

        // Option handling.
        if arg.starts_with('-') && arg != "-" {
            if arg == "-t" || arg == "--target-directory" {
                target_dir_mode = true;
                // Skip the directory argument.
                if i + 1 < args.len() {
                    i += 2;
                    continue;
                }
                return Vec::new(); // Malformed.
            }
            if arg.starts_with("--target-directory=") {
                target_dir_mode = true;
                i += 1;
                continue;
            }
            // Other flags (skip).
            i += 1;
            continue;
        }

        non_flag_args.push(arg.clone());
        i += 1;
    }

    if non_flag_args.is_empty() {
        return Vec::new();
    }

    // If target dir specified via -t, all remaining args are sources.
    if target_dir_mode {
        return non_flag_args;
    }

    // Standard form: mv src1 src2 ... dest — all but last are sources.
    if non_flag_args.len() >= 2 {
        non_flag_args.pop();
        return non_flag_args;
    }

    // Single arg — treat conservatively as potential source.
    non_flag_args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_simple_commands() {
        let segs = split_segments("ls && pwd");
        assert_eq!(segs, vec!["ls", "pwd"]);
    }

    #[test]
    fn split_semicolons() {
        let segs = split_segments("ls; pwd; echo hello");
        assert_eq!(segs, vec!["ls", "pwd", "echo hello"]);
    }

    #[test]
    fn split_or() {
        let segs = split_segments("test -f x || echo missing");
        assert_eq!(segs, vec!["test -f x", "echo missing"]);
    }

    #[test]
    fn split_preserves_pipe() {
        let segs = split_segments("ls | grep foo");
        assert_eq!(segs, vec!["ls | grep foo"]);
    }

    #[test]
    fn split_quoted_semicolons() {
        let segs = split_segments(r#"echo "hello; world" && pwd"#);
        assert_eq!(segs, vec![r#"echo "hello; world""#, "pwd"]);
    }

    #[test]
    fn extract_rm_basic() {
        let paths = extract_rm_paths("rm -rf /tmp/test /var/data");
        assert_eq!(paths, vec!["/tmp/test", "/var/data"]);
    }

    #[test]
    fn extract_rm_with_prefix() {
        let paths = extract_rm_paths("sudo /bin/rm -rf /tmp/test");
        assert_eq!(paths, vec!["/tmp/test"]);
    }

    #[test]
    fn extract_rm_flags_only() {
        let paths = extract_rm_paths("rm -rf");
        assert!(paths.is_empty());
    }

    #[test]
    fn extract_rmdir() {
        let paths = extract_rm_paths("rmdir /tmp/empty");
        assert_eq!(paths, vec!["/tmp/empty"]);
    }

    #[test]
    fn extract_mv_standard() {
        let paths = extract_mv_source_paths("mv src1 src2 dest/");
        assert_eq!(paths, vec!["src1", "src2"]);
    }

    #[test]
    fn extract_mv_target_dir() {
        let paths = extract_mv_source_paths("mv -t dest/ src1 src2");
        assert_eq!(paths, vec!["src1", "src2"]);
    }

    #[test]
    fn extract_mv_with_flags() {
        let paths = extract_mv_source_paths("mv -f src dest");
        assert_eq!(paths, vec!["src"]);
    }

    #[test]
    fn extract_mv_with_sudo() {
        let paths = extract_mv_source_paths("sudo /bin/mv src dest");
        assert_eq!(paths, vec!["src"]);
    }

    #[test]
    fn extract_mv_target_directory_equals() {
        let paths = extract_mv_source_paths("mv --target-directory=/dest src1 src2");
        assert_eq!(paths, vec!["src1", "src2"]);
    }
}
