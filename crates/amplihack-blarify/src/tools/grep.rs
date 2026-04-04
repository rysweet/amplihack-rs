//! Grep code tool — pattern search over code snippets.
//!
//! Mirrors the Python `tools/grep_code.py`.

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::db::manager::{DbManager, QueryParams};
use crate::db::queries;

/// Input parameters for grep_code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepCodeInput {
    pub pattern: String,
    #[serde(default = "default_case_sensitive")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub file_pattern: Option<String>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_case_sensitive() -> bool {
    true
}
fn default_max_results() -> usize {
    20
}

/// A single grep match result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepCodeMatch {
    pub file_path: String,
    pub line_number: i64,
    pub symbol_name: String,
    pub symbol_type: Vec<String>,
    pub code_snippet: String,
    pub id: String,
}

/// Extract matching lines with context from code.
pub fn extract_matching_lines(
    code: &str,
    pattern: &Regex,
    context_lines: usize,
) -> Vec<(i64, String)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut matches = Vec::new();
    let mut matched_indices = std::collections::HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        if pattern.is_match(line) {
            let start = i.saturating_sub(context_lines);
            let end = (i + context_lines + 1).min(lines.len());
            for j in start..end {
                matched_indices.insert(j);
            }
        }
    }

    let mut sorted_indices: Vec<usize> = matched_indices.into_iter().collect();
    sorted_indices.sort_unstable();

    for idx in sorted_indices {
        matches.push(((idx + 1) as i64, lines[idx].to_string()));
    }

    matches
}

/// Convert a glob pattern to a regex pattern.
pub fn convert_glob_to_regex(glob: &str) -> String {
    let mut regex = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '.' => regex.push_str("\\."),
            c => regex.push(c),
        }
    }
    regex.push('$');
    regex
}

/// Grep code in the graph database.
pub fn grep_code(db_manager: &dyn DbManager, input: &GrepCodeInput) -> Result<serde_json::Value> {
    let regex_pattern = if input.case_sensitive {
        Regex::new(&input.pattern)?
    } else {
        Regex::new(&format!("(?i){}", &input.pattern))?
    };

    let file_regex = input
        .file_pattern
        .as_ref()
        .map(|fp| Regex::new(&convert_glob_to_regex(fp)))
        .transpose()?;

    let mut params = QueryParams::new();
    params.insert(
        "entity_id".into(),
        serde_json::Value::String(db_manager.entity_id().into()),
    );
    params.insert(
        "repo_id".into(),
        serde_json::Value::String(db_manager.repo_id().into()),
    );

    let results = db_manager.query(queries::GREP_CODE_QUERY, Some(&params), false)?;

    let mut matches = Vec::new();
    for row in &results {
        if matches.len() >= input.max_results {
            break;
        }

        let path = row.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let text = row.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let id = row.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let labels: Vec<String> = row
            .get("labels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let start_line = row.get("start_line").and_then(|v| v.as_i64()).unwrap_or(1);

        // Apply file filter
        if let Some(ref fr) = file_regex
            && !fr.is_match(path)
        {
            continue;
        }

        let matched_lines = extract_matching_lines(text, &regex_pattern, 2);
        for (relative_line, snippet) in matched_lines {
            if matches.len() >= input.max_results {
                break;
            }
            matches.push(GrepCodeMatch {
                file_path: path.into(),
                line_number: start_line + relative_line - 1,
                symbol_name: name.into(),
                symbol_type: labels.clone(),
                code_snippet: snippet,
                id: id.into(),
            });
        }
    }

    if matches.is_empty() {
        Ok(serde_json::json!({"message": "No matches found"}))
    } else {
        Ok(serde_json::json!({"matches": matches}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_matching_lines_basic() {
        let code = "line1\nline2 match\nline3\nline4";
        let re = Regex::new("match").unwrap();
        let results = extract_matching_lines(code, &re, 0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 2);
        assert_eq!(results[0].1, "line2 match");
    }

    #[test]
    fn extract_with_context() {
        let code = "a\nb\nc match\nd\ne";
        let re = Regex::new("match").unwrap();
        let results = extract_matching_lines(code, &re, 1);
        assert_eq!(results.len(), 3); // b, c match, d
    }

    #[test]
    fn glob_to_regex_conversion() {
        assert_eq!(convert_glob_to_regex("*.rs"), "^.*\\.rs$");
        assert_eq!(convert_glob_to_regex("src/*.py"), "^src/.*\\.py$");
    }

    #[test]
    fn grep_code_match_serialization() {
        let m = GrepCodeMatch {
            file_path: "src/main.rs".into(),
            line_number: 42,
            symbol_name: "main".into(),
            symbol_type: vec!["FUNCTION".into()],
            code_snippet: "fn main() {}".into(),
            id: "abc".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"line_number\":42"));
    }
}
