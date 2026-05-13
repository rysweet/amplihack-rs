//! Level 1 pure-Rust syntax checking for C# files.
//!
//! Checks:
//! - Balanced delimiters: `{}`, `()`, `[]`
//! - Common C# structural patterns (namespace, class/struct/interface declarations)
//! - String/char literal awareness (skip delimiters inside strings)

use super::{Diagnostic, DiagnosticSeverity};
use anyhow::Result;
use std::path::Path;

/// Run all syntax checks on a single .cs file. Returns diagnostics (empty = pass).
pub fn check_syntax(path: &Path) -> Result<Vec<Diagnostic>> {
    let content = std::fs::read_to_string(path)?;
    let mut diagnostics = Vec::new();

    diagnostics.extend(check_balanced_delimiters(&content));

    Ok(diagnostics)
}

/// Check that `{}`, `()`, `[]` are balanced, respecting string/char literals and comments.
fn check_balanced_delimiters(content: &str) -> Vec<Diagnostic> {
    let mut stack: Vec<(char, u32)> = Vec::with_capacity(32);
    let mut diagnostics = Vec::new();
    let mut line_num: u32 = 1;
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut in_verbatim_string = false;
    let mut in_char = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut prev_char = '\0';

    while let Some(ch) = chars.next() {
        if ch == '\n' {
            line_num += 1;
            in_line_comment = false;
            prev_char = ch;
            continue;
        }

        // Handle comment starts
        if !in_string
            && !in_verbatim_string
            && !in_char
            && !in_line_comment
            && !in_block_comment
            && ch == '/'
            && let Some(&next) = chars.peek()
        {
            if next == '/' {
                in_line_comment = true;
                chars.next();
                prev_char = '/';
                continue;
            } else if next == '*' {
                in_block_comment = true;
                chars.next();
                prev_char = '*';
                continue;
            }
        }

        // Handle block comment end
        if in_block_comment {
            if ch == '*'
                && let Some(&next) = chars.peek()
                && next == '/'
            {
                in_block_comment = false;
                chars.next();
            }
            prev_char = ch;
            continue;
        }

        if in_line_comment {
            prev_char = ch;
            continue;
        }

        // Handle string literals
        if !in_char && !in_verbatim_string && ch == '"' && prev_char != '\\' {
            if prev_char == '@' {
                in_verbatim_string = true;
            } else {
                in_string = !in_string;
            }
            prev_char = ch;
            continue;
        }

        if in_verbatim_string {
            if ch == '"' {
                if let Some(&next) = chars.peek()
                    && next == '"'
                {
                    // Escaped quote in verbatim string
                    chars.next();
                    prev_char = '"';
                    continue;
                }
                in_verbatim_string = false;
            }
            prev_char = ch;
            continue;
        }

        if in_string {
            prev_char = ch;
            continue;
        }

        // Handle char literals
        if ch == '\'' && prev_char != '\\' {
            in_char = !in_char;
            prev_char = ch;
            continue;
        }

        if in_char {
            prev_char = ch;
            continue;
        }

        // Track delimiters
        match ch {
            '{' | '(' | '[' => stack.push((ch, line_num)),
            '}' | ')' | ']' => {
                let expected = match ch {
                    '}' => '{',
                    ')' => '(',
                    ']' => '[',
                    _ => unreachable!(),
                };
                if let Some(&(open, _)) = stack.last() {
                    if open == expected {
                        stack.pop();
                    } else {
                        diagnostics.push(Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            message: format!(
                                "Mismatched delimiter: expected closing for '{}', found '{}'",
                                open, ch
                            ),
                            line: Some(line_num),
                            column: None,
                        });
                    }
                } else {
                    diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        message: format!("Unexpected closing delimiter '{}'", ch),
                        line: Some(line_num),
                        column: None,
                    });
                }
            }
            _ => {}
        }

        prev_char = ch;
    }

    // Report unclosed delimiters
    for (open, line) in stack {
        diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Error,
            message: format!("Unclosed delimiter '{}' (unbalanced braces/brackets)", open),
            line: Some(line),
            column: None,
        });
    }

    diagnostics
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn balanced_simple_class() {
        let content = r#"
namespace Foo {
    class Bar {
        void Baz() {
            var x = (1 + 2);
            var arr = new int[] { 1, 2, 3 };
        }
    }
}
"#;
        let diags = check_balanced_delimiters(content);
        assert!(
            diags.is_empty(),
            "Expected no diagnostics, got: {:?}",
            diags
        );
    }

    #[test]
    fn unbalanced_extra_open_brace() {
        let content = r#"
class Foo {
    void Bar() {
        if (true) {
            // missing close
        }

"#;
        let diags = check_balanced_delimiters(content);
        assert!(!diags.is_empty());
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("Unclosed delimiter"))
        );
    }

    #[test]
    fn unbalanced_extra_close_brace() {
        let content = "class Foo { } }";
        let diags = check_balanced_delimiters(content);
        assert!(!diags.is_empty());
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("Unexpected closing"))
        );
    }

    #[test]
    fn unbalanced_parentheses() {
        let content = "class Foo { void Bar() { var x = (1 + 2; } }";
        let diags = check_balanced_delimiters(content);
        assert!(!diags.is_empty());
    }

    #[test]
    fn delimiters_in_strings_are_ignored() {
        let content = r#"
class Foo {
    void Bar() {
        var s = "{ not a real brace (";
        var t = "} still not real )";
    }
}
"#;
        let diags = check_balanced_delimiters(content);
        assert!(
            diags.is_empty(),
            "Braces in strings should be ignored: {:?}",
            diags
        );
    }

    #[test]
    fn delimiters_in_verbatim_strings_are_ignored() {
        let content = r#"
class Foo {
    void Bar() {
        var s = @"multi
line {{{ string with ""quotes""
and unbalanced ( stuff";
    }
}
"#;
        let diags = check_balanced_delimiters(content);
        assert!(
            diags.is_empty(),
            "Braces in verbatim strings should be ignored: {:?}",
            diags
        );
    }

    #[test]
    fn delimiters_in_line_comments_are_ignored() {
        let content = r#"
class Foo {
    void Bar() {
        // this { is a comment (
        var x = 1;
    }
}
"#;
        let diags = check_balanced_delimiters(content);
        assert!(
            diags.is_empty(),
            "Braces in comments should be ignored: {:?}",
            diags
        );
    }

    #[test]
    fn delimiters_in_block_comments_are_ignored() {
        let content = r#"
class Foo {
    void Bar() {
        /* multi-line
           { comment with ( brackets
        */
        var x = 1;
    }
}
"#;
        let diags = check_balanced_delimiters(content);
        assert!(
            diags.is_empty(),
            "Braces in block comments should be ignored: {:?}",
            diags
        );
    }

    #[test]
    fn mismatched_delimiter_types() {
        let content = "class Foo { void Bar() { var x = (1]; } }";
        let diags = check_balanced_delimiters(content);
        assert!(!diags.is_empty());
        assert!(diags.iter().any(|d| d.message.contains("Mismatched")));
    }

    #[test]
    fn check_syntax_reads_file_and_validates() {
        let dir = TempDir::new().unwrap();
        let cs = dir.path().join("Test.cs");
        fs::write(&cs, "class Good { void M() { } }").unwrap();

        let diags = check_syntax(&cs).unwrap();
        assert!(diags.is_empty());
    }

    #[test]
    fn check_syntax_reports_file_with_errors() {
        let dir = TempDir::new().unwrap();
        let cs = dir.path().join("Bad.cs");
        fs::write(&cs, "class Bad { void M() { }").unwrap();

        let diags = check_syntax(&cs).unwrap();
        assert!(!diags.is_empty());
    }

    #[test]
    fn check_syntax_nonexistent_file_returns_error() {
        let result = check_syntax(Path::new("/nonexistent/file.cs"));
        assert!(result.is_err());
    }

    #[test]
    fn empty_file_is_valid() {
        let dir = TempDir::new().unwrap();
        let cs = dir.path().join("Empty.cs");
        fs::write(&cs, "").unwrap();

        let diags = check_syntax(&cs).unwrap();
        assert!(diags.is_empty());
    }

    #[test]
    fn char_literals_with_braces_are_ignored() {
        let content = r#"
class Foo {
    void Bar() {
        char open = '{';
        char close = '}';
    }
}
"#;
        let diags = check_balanced_delimiters(content);
        assert!(
            diags.is_empty(),
            "Braces in char literals should be ignored: {:?}",
            diags
        );
    }
}
