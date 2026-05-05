//! User preference loading, extraction, and formatting.

use amplihack_types::ProjectDirs;
use std::fs;

/// Load user preferences from USER_PREFERENCES.md.
/// Also detects `## Learned Patterns` section.
pub(crate) fn load_user_preferences_with_patterns(dirs: &ProjectDirs) -> (Option<String>, bool) {
    let mut candidates = Vec::new();
    if let Some(path) = dirs.resolve_preferences_file() {
        candidates.push(path);
    }
    candidates.push(dirs.root.join("USER_PREFERENCES.md"));

    for path in &candidates {
        if path.exists() {
            match fs::read_to_string(path) {
                Ok(content) => {
                    let has_learned = content.contains("## Learned Patterns")
                        && content
                            .split("## Learned Patterns")
                            .nth(1)
                            .map(|s| {
                                s.lines()
                                    .any(|l| !l.trim().is_empty() && !l.starts_with('#'))
                            })
                            .unwrap_or(false);
                    let prefs = extract_preferences(&content);
                    if !prefs.is_empty() {
                        return (Some(build_preference_context(&prefs)), has_learned);
                    }
                    return (None, has_learned);
                }
                Err(e) => {
                    tracing::warn!("Failed to read preferences: {}", e);
                }
            }
        }
    }

    (None, false)
}

/// Check if the user prompt is a /dev invocation.
pub(crate) fn is_dev_invocation(prompt: &str) -> bool {
    let lowered = prompt.trim().to_ascii_lowercase();
    lowered == "/dev"
        || lowered.starts_with("/dev ")
        || lowered.starts_with("/dev\n")
        || lowered.contains("\n/dev ")
        || lowered.contains("\n/dev\n")
        || lowered.contains("dev-orchestrator")
        || lowered.starts_with("/amplihack:dev")
        || lowered.starts_with("/.claude:amplihack:dev")
}

/// Extract preference key-value pairs from markdown content.
///
/// Supports both formats for Python parity:
/// - Table format: `| key | value |`
/// - Header format: `### Key\nvalue`
pub fn extract_preferences(content: &str) -> Vec<(String, String)> {
    let mut prefs = Vec::new();

    // Try table format first.
    let mut found_table = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let parts: Vec<&str> = trimmed.split('|').map(str::trim).collect();
            if parts.len() >= 3 {
                let key = parts[1].trim();
                let value = parts[2].trim();
                if !key.is_empty()
                    && !value.is_empty()
                    && key != "Setting"
                    && key != "---"
                    && !key.starts_with('-')
                {
                    prefs.push((key.to_string(), value.to_string()));
                    found_table = true;
                }
            }
        }
    }

    if found_table {
        return prefs;
    }

    // Fall back to header format: ### Key\nvalue
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(header) = trimmed.strip_prefix("### ") {
            let key = header.trim().to_string();
            let mut value_lines = Vec::new();
            i += 1;
            while i < lines.len() {
                let next = lines[i].trim();
                if next.starts_with("### ") || next.starts_with("## ") || next.starts_with("# ") {
                    break;
                }
                if !next.is_empty() {
                    value_lines.push(next);
                }
                i += 1;
            }
            if !key.is_empty() && !value_lines.is_empty() {
                prefs.push((key, value_lines.join(" ")));
            }
        } else {
            i += 1;
        }
    }

    prefs
}

/// Build a context string from preferences.
pub fn build_preference_context(prefs: &[(String, String)]) -> String {
    let mut parts = vec!["## User Preferences".to_string()];
    for (key, value) in prefs {
        parts.push(format!("- **{key}**: {value}"));
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // extract_preferences — table format
    // -----------------------------------------------------------------------

    #[test]
    fn extract_table_format() {
        let content = "\
| Setting | Value |
| --- | --- |
| Verbosity | balanced |
| Style | concise |
";
        let prefs = extract_preferences(content);
        assert_eq!(prefs.len(), 2);
        assert_eq!(prefs[0], ("Verbosity".into(), "balanced".into()));
        assert_eq!(prefs[1], ("Style".into(), "concise".into()));
    }

    #[test]
    fn extract_table_skips_header_and_separator() {
        let content = "\
| Setting | Value |
| --- | --- |
| Theme | dark |
";
        let prefs = extract_preferences(content);
        assert_eq!(prefs.len(), 1);
        assert_eq!(prefs[0].0, "Theme");
    }

    #[test]
    fn extract_table_empty_value_skipped() {
        let content = "| Key |  |\n";
        let prefs = extract_preferences(content);
        assert!(prefs.is_empty());
    }

    // -----------------------------------------------------------------------
    // extract_preferences — header format
    // -----------------------------------------------------------------------

    #[test]
    fn extract_header_format() {
        let content = "\
### Verbosity
balanced

### Style
concise and direct
";
        let prefs = extract_preferences(content);
        assert_eq!(prefs.len(), 2);
        assert_eq!(prefs[0], ("Verbosity".into(), "balanced".into()));
        assert_eq!(prefs[1], ("Style".into(), "concise and direct".into()));
    }

    #[test]
    fn extract_header_stops_at_next_header() {
        let content = "\
### Key1
value1
### Key2
value2
## Section
ignored
";
        let prefs = extract_preferences(content);
        assert_eq!(prefs.len(), 2);
    }

    #[test]
    fn extract_empty_content() {
        assert!(extract_preferences("").is_empty());
    }

    // -----------------------------------------------------------------------
    // build_preference_context
    // -----------------------------------------------------------------------

    #[test]
    fn build_context_formats_correctly() {
        let prefs = vec![
            ("Verbosity".into(), "balanced".into()),
            ("Style".into(), "concise".into()),
        ];
        let ctx = build_preference_context(&prefs);
        assert!(ctx.starts_with("## User Preferences"));
        assert!(ctx.contains("- **Verbosity**: balanced"));
        assert!(ctx.contains("- **Style**: concise"));
    }

    #[test]
    fn build_context_empty_prefs() {
        let ctx = build_preference_context(&[]);
        assert_eq!(ctx, "## User Preferences");
    }

    // -----------------------------------------------------------------------
    // is_dev_invocation
    // -----------------------------------------------------------------------

    #[test]
    fn dev_invocation_exact() {
        assert!(is_dev_invocation("/dev"));
    }

    #[test]
    fn dev_invocation_with_args() {
        assert!(is_dev_invocation("/dev build the feature"));
    }

    #[test]
    fn dev_invocation_with_newline() {
        assert!(is_dev_invocation("/dev\nsome task"));
    }

    #[test]
    fn dev_invocation_embedded() {
        assert!(is_dev_invocation("first line\n/dev build it"));
    }

    #[test]
    fn dev_invocation_orchestrator() {
        assert!(is_dev_invocation("use dev-orchestrator"));
    }

    #[test]
    fn dev_invocation_amplihack() {
        assert!(is_dev_invocation("/amplihack:dev task"));
    }

    #[test]
    fn not_dev_invocation() {
        assert!(!is_dev_invocation("develop something"));
        assert!(!is_dev_invocation("hello world"));
    }
}
