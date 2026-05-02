//! Markdown-section parsing helpers for expert review outputs.
//!
//! These mirror Python's `_extract_section`, `_extract_list_items`, and
//! `_extract_scores` semantics without pulling in a regex dependency.

use std::collections::HashMap;

/// Extract the body of a `## <SectionName>` block, trimmed.
///
/// Header match is case-insensitive on the section name. The body extends
/// from the line after the header to the line before the next `## ` header
/// (or end of input).
pub fn extract_section(text: &str, section_name: &str) -> String {
    let header_lower = format!("## {}", section_name.to_lowercase());
    let mut found = false;
    let mut buf = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !found {
            let lc = trimmed.to_lowercase();
            if lc.starts_with(&header_lower) {
                let rest = &lc[header_lower.len()..];
                if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                    found = true;
                }
            }
        } else if trimmed.starts_with("## ") {
            break;
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    buf.trim().to_string()
}

/// Extract bullet-list items (`- item` or `* item`) from a section body.
pub fn extract_list_items(text: &str, section_name: &str) -> Vec<String> {
    let body = extract_section(text, section_name);
    if body.is_empty() {
        return Vec::new();
    }
    let mut items = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- ") {
            items.push(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("* ") {
            items.push(rest.trim().to_string());
        }
    }
    items
}

/// Extract `name: 0.8` style scores into a `HashMap<String, f32>`.
pub fn extract_scores(text: &str, section_name: &str) -> HashMap<String, f32> {
    let body = extract_section(text, section_name);
    let mut scores = HashMap::new();
    if body.is_empty() {
        return scores;
    }
    for line in body.lines() {
        let t = line
            .trim_start()
            .trim_start_matches(['-', '*'])
            .trim_start();
        if let Some((name, val)) = t.split_once(':') {
            let key = name.trim();
            let v = val.trim();
            if key.is_empty() {
                continue;
            }
            if !key
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == '_')
                .unwrap_or(false)
            {
                continue;
            }
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            if let Ok(parsed) = v.parse::<f32>() {
                scores.insert(key.to_string(), parsed);
            }
        }
    }
    scores
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn section_basic() {
        let t = "## Vote\nAPPROVE\n\n## Confidence\n0.85\n";
        assert_eq!(extract_section(t, "Vote"), "APPROVE");
        assert_eq!(extract_section(t, "Confidence"), "0.85");
    }

    #[test]
    fn section_missing_returns_empty() {
        assert_eq!(extract_section("## A\nx", "B"), "");
    }

    #[test]
    fn list_items_strips_leading_marker() {
        let t = "## S\n- a\n* b\n";
        assert_eq!(extract_list_items(t, "S"), vec!["a", "b"]);
    }

    #[test]
    fn scores_parse_floats() {
        let t = "## Domain Scores\n- accuracy: 0.8\n- latency: 0.6\n";
        let s = extract_scores(t, "Domain Scores");
        assert_eq!(s.get("accuracy"), Some(&0.8));
        assert_eq!(s.get("latency"), Some(&0.6));
    }

    #[test]
    fn scores_skip_invalid_floats() {
        let t = "## S\n- ok: 0.5\n- bad: NaNish\n";
        let s = extract_scores(t, "S");
        assert!(s.contains_key("ok"));
        assert!(!s.contains_key("bad"));
    }
}
