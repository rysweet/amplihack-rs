//! Code review domain tools.
//!
//! Pure functions for analyzing code quality, security, and style.
//! Ports `domain_agents/code_review/tools.py`.

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

/// Parse and analyze code structure.
pub fn analyze_code(code: &str, language: &str) -> Value {
    if code.trim().is_empty() {
        return serde_json::json!({
            "line_count": 0, "function_count": 0, "class_count": 0,
            "import_count": 0, "comment_ratio": 0.0,
            "complexity_indicators": {},
        });
    }

    let lines: Vec<&str> = code.lines().collect();
    let total = lines.len();

    let (func_re, class_re, import_re, comment_re) = if language == "python" {
        (
            r"(?m)^\s*def\s+\w+",
            r"(?m)^\s*class\s+\w+",
            r"(?m)^\s*(import|from)\s+",
            r"(?m)^\s*#",
        )
    } else {
        (
            r"(def |function |fn |func )",
            r"class\s+\w+",
            r"(import|require|use)",
            r"(?m)^\s*(#|//)",
        )
    };

    let function_count = Regex::new(func_re)
        .map(|r| r.find_iter(code).count())
        .unwrap_or(0);
    let class_count = Regex::new(class_re)
        .map(|r| r.find_iter(code).count())
        .unwrap_or(0);
    let import_count = Regex::new(import_re)
        .map(|r| r.find_iter(code).count())
        .unwrap_or(0);
    let comment_re = Regex::new(comment_re).ok();
    let comment_lines = lines
        .iter()
        .filter(|l| comment_re.as_ref().map(|r| r.is_match(l)).unwrap_or(false))
        .count();
    let comment_ratio = if total > 0 {
        comment_lines as f64 / total as f64
    } else {
        0.0
    };

    let max_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .max()
        .unwrap_or(0);

    let branch_re = Regex::new(r"(?m)^\s*(if|elif|else|for|while|try|except|switch|case)\b").ok();
    let branch_count = branch_re.map(|r| r.find_iter(code).count()).unwrap_or(0);
    let avg_fn_len = total / function_count.max(1);

    serde_json::json!({
        "line_count": total,
        "function_count": function_count,
        "class_count": class_count,
        "import_count": import_count,
        "comment_ratio": (comment_ratio * 1000.0).round() / 1000.0,
        "complexity_indicators": {
            "max_nesting_depth": max_indent / 4,
            "branch_count": branch_count,
            "avg_function_length": avg_fn_len,
        },
    })
}

/// Check code for style violations.
pub fn check_style(code: &str, language: &str) -> Vec<Value> {
    let mut issues = Vec::new();
    if code.trim().is_empty() {
        return issues;
    }

    for (i, line) in code.lines().enumerate() {
        let line_num = i + 1;
        if line.len() > 120 {
            issues.push(serde_json::json!({
                "line": line_num, "type": "line_too_long", "severity": "warning",
                "message": format!("Line is {} characters (max 120)", line.len()),
            }));
        }
        if line != line.trim_end_matches([' ', '\t']) {
            issues.push(serde_json::json!({
                "line": line_num, "type": "trailing_whitespace",
                "severity": "info", "message": "Trailing whitespace",
            }));
        }
    }

    if language == "python" {
        if let Ok(re) = Regex::new(r"def\s+([a-z]+[A-Z]\w*)") {
            for cap in re.captures_iter(code) {
                if let Some(name) = cap.get(1) {
                    issues.push(serde_json::json!({
                        "line": 0, "type": "naming_convention", "severity": "warning",
                        "message": format!("Function '{}' uses camelCase instead of snake_case", name.as_str()),
                    }));
                }
            }
        }
        if code.contains("except:") || code.contains("except :") {
            issues.push(serde_json::json!({
                "line": 0, "type": "bare_except", "severity": "error",
                "message": "Found bare except clause(s)",
            }));
        }
    }

    issues
}

/// Scan code for security vulnerabilities.
pub fn detect_security_issues(code: &str, _language: &str) -> Vec<Value> {
    let mut issues = Vec::new();
    if code.trim().is_empty() {
        return issues;
    }

    let sql_patterns: &[(&str, &str)] = &[
        (
            r#"execute\s*\(\s*["'].*%s"#,
            "SQL injection via string formatting",
        ),
        (r#"execute\s*\(\s*f["']"#, "SQL injection via f-string"),
        (
            r#"execute\s*\(\s*["'].*\+"#,
            "SQL injection via concatenation",
        ),
        (
            r#"cursor\.execute\s*\(\s*["'].*\.format\("#,
            "SQL injection via .format()",
        ),
    ];
    for (pattern, msg) in sql_patterns {
        if Regex::new(pattern)
            .map(|r| r.is_match(code))
            .unwrap_or(false)
        {
            issues.push(serde_json::json!({
                "type": "sql_injection", "severity": "critical",
                "message": msg, "recommendation": "Use parameterized queries",
            }));
        }
    }

    let secret_patterns: &[(&str, &str)] = &[
        (
            r#"(?i)(password|secret|api_key|token)\s*=\s*["'][^"']+["']"#,
            "Hardcoded secret",
        ),
        (
            r#"(?i)(AWS_SECRET|PRIVATE_KEY)\s*=\s*["']"#,
            "Hardcoded cloud credential",
        ),
    ];
    for (pattern, msg) in secret_patterns {
        if Regex::new(pattern)
            .map(|r| r.is_match(code))
            .unwrap_or(false)
        {
            issues.push(serde_json::json!({
                "type": "hardcoded_secret", "severity": "critical",
                "message": msg, "recommendation": "Use environment variables",
            }));
        }
    }

    if Regex::new(r"\beval\s*\(")
        .map(|r| r.is_match(code))
        .unwrap_or(false)
        || Regex::new(r"\bexec\s*\(")
            .map(|r| r.is_match(code))
            .unwrap_or(false)
    {
        issues.push(serde_json::json!({
            "type": "dangerous_function", "severity": "high",
            "message": "Use of eval() or exec()",
            "recommendation": "Avoid eval/exec",
        }));
    }

    if Regex::new(r"os\.system\s*\(")
        .map(|r| r.is_match(code))
        .unwrap_or(false)
        || Regex::new(r"subprocess\.\w+\(.*shell\s*=\s*True")
            .map(|r| r.is_match(code))
            .unwrap_or(false)
    {
        issues.push(serde_json::json!({
            "type": "command_injection", "severity": "high",
            "message": "Potential OS command injection",
            "recommendation": "Use subprocess with shell=False",
        }));
    }

    issues
}

/// Suggest code improvements.
pub fn suggest_improvements(code: &str, language: &str) -> Vec<Value> {
    let mut suggestions = Vec::new();
    if code.trim().is_empty() {
        return suggestions;
    }

    let lines: Vec<&str> = code.lines().collect();

    if language == "python" {
        let func_re = Regex::new(r"(?m)^\s*def\s+(\w+)");
        let name_re = Regex::new(r"def\s+(\w+)");
        if let (Ok(func_re), Ok(name_re)) = (func_re, name_re) {
            for (i, line) in lines.iter().enumerate() {
                if !func_re.is_match(line) {
                    continue;
                }
                let next = match lines.get(i + 1) {
                    Some(n) => n.trim(),
                    None => continue,
                };
                if next.starts_with("\"\"\"") || next.starts_with("'''") {
                    continue;
                }
                if let Some(name) = name_re.captures(line).and_then(|c| c.get(1)) {
                    suggestions.push(serde_json::json!({
                        "type": "missing_docstring", "severity": "info",
                        "message": format!("Function '{}' lacks a docstring", name.as_str()),
                        "line": i + 1,
                    }));
                }
            }
        }
    }

    let analysis = analyze_code(code, language);
    let indicators = analysis
        .get("complexity_indicators")
        .cloned()
        .unwrap_or_default();

    if indicators
        .get("avg_function_length")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        > 30
    {
        suggestions.push(serde_json::json!({
            "type": "large_functions", "severity": "warning",
            "message": "Average function length exceeds 30 lines",
        }));
    }

    if indicators
        .get("max_nesting_depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        > 4
    {
        suggestions.push(serde_json::json!({
            "type": "deep_nesting", "severity": "warning",
            "message": "Maximum nesting depth exceeds 4",
        }));
    }

    suggestions
}

/// Helper to extract a string field from a task map.
pub(crate) fn get_str<'a>(task: &'a HashMap<String, serde_json::Value>, key: &str) -> &'a str {
    task.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_empty_code() {
        let r = analyze_code("", "python");
        assert_eq!(r["line_count"], 0);
    }

    #[test]
    fn analyze_python_code() {
        let code = "import os\n\ndef hello():\n    print('hi')\n\nclass Foo:\n    pass\n";
        let r = analyze_code(code, "python");
        assert_eq!(r["function_count"], 1);
        assert_eq!(r["class_count"], 1);
        assert_eq!(r["import_count"], 1);
    }

    #[test]
    fn check_style_camel_case() {
        let code = "def calculateTotal(items):\n    pass\n";
        let issues = check_style(code, "python");
        assert!(issues.iter().any(|i| i["type"] == "naming_convention"));
    }

    #[test]
    fn check_style_bare_except() {
        let code = "try:\n    pass\nexcept:\n    pass\n";
        let issues = check_style(code, "python");
        assert!(issues.iter().any(|i| i["type"] == "bare_except"));
    }

    #[test]
    fn detect_sql_injection() {
        let code = "cursor.execute(f\"SELECT * FROM users WHERE name = '{name}'\")\n";
        let issues = detect_security_issues(code, "python");
        assert!(issues.iter().any(|i| i["type"] == "sql_injection"));
    }

    #[test]
    fn detect_hardcoded_secret() {
        let code = "API_KEY = \"sk-1234567890\"\n";
        let issues = detect_security_issues(code, "python");
        assert!(issues.iter().any(|i| i["type"] == "hardcoded_secret"));
    }

    #[test]
    fn detect_eval_usage() {
        let code = "result = eval(expr)\n";
        let issues = detect_security_issues(code, "python");
        assert!(issues.iter().any(|i| i["type"] == "dangerous_function"));
    }

    #[test]
    fn suggest_missing_docstring() {
        let code = "def hello():\n    pass\n";
        let suggestions = suggest_improvements(code, "python");
        assert!(suggestions.iter().any(|s| s["type"] == "missing_docstring"));
    }

    #[test]
    fn suggest_no_false_positive_docstring() {
        let code = "def hello():\n    \"\"\"A greeting.\"\"\"\n    pass\n";
        let suggestions = suggest_improvements(code, "python");
        assert!(!suggestions.iter().any(|s| s["type"] == "missing_docstring"));
    }
}
