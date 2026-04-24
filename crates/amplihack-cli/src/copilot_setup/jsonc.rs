//! JSONC (JSON-with-comments) tolerance utilities for Copilot config files.
//!
//! GitHub Copilot CLI writes `~/.copilot/config.json` as JSONC: it begins with
//! one or more `//` line comments before the actual JSON object. Bare
//! `serde_json::from_str` rejects those comments, which broke amplihack's
//! `register_plugin` and `write_user_level_hooks` paths.
//!
//! This module provides two pure string utilities used at the read/write
//! boundary:
//!
//! * [`strip_jsonc_comments`] — remove `//…\n` and `/* … */` comments while
//!   preserving the byte contents of double-quoted string literals.
//! * [`leading_comment_prefix`] — return the substring up to (exclusive) the
//!   first `{` or `[` found outside of comments and strings, so the caller
//!   can write it back verbatim and not lose user/Copilot-managed comments.

/// Remove JSONC `//` line comments and `/* */` block comments from `input`
/// while preserving the contents of double-quoted string literals byte-for-byte.
///
/// Notes:
/// * Block comments do not nest.
/// * `//` and `/*` inside a `"…"` string are preserved verbatim.
/// * Unterminated strings or block comments at EOF are tolerated (no panic);
///   the partial content is preserved up to EOF.
/// * Line/column information from `serde_json` errors will refer to offsets
///   within the returned (stripped) buffer, NOT the on-disk file.
pub fn strip_jsonc_comments(input: &str) -> String {
    // Byte-level scan with slice copies. All structural markers ('"', '/',
    // '*', '\\', '\n') are ASCII, so they only ever appear at UTF-8 codepoint
    // boundaries — slicing `&input[chunk_start..i]` is always safe.
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;
    let mut chunk_start = 0;

    enum State {
        Normal,
        InString,
        InLineComment,
        InBlockComment,
    }
    let mut state = State::Normal;
    let mut escape = false;

    while i < len {
        let b = bytes[i];
        match state {
            State::Normal => {
                if b == b'"' {
                    state = State::InString;
                    escape = false;
                    i += 1;
                } else if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
                    out.push_str(&input[chunk_start..i]);
                    i += 2;
                    state = State::InLineComment;
                } else if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    out.push_str(&input[chunk_start..i]);
                    i += 2;
                    state = State::InBlockComment;
                } else {
                    i += 1;
                }
            }
            State::InString => {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::InLineComment => {
                if b == b'\n' {
                    state = State::Normal;
                    chunk_start = i; // newline becomes the start of next chunk
                }
                i += 1;
            }
            State::InBlockComment => {
                if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                    state = State::Normal;
                    i += 2;
                    chunk_start = i;
                } else {
                    i += 1;
                }
            }
        }
    }

    // Flush the trailing chunk for any state where bytes beyond chunk_start
    // are content to keep (Normal, InString — including unterminated strings).
    if matches!(state, State::Normal | State::InString) {
        out.push_str(&input[chunk_start..len]);
    }
    out
}

/// Return the substring of `input` that appears before the first `{` or `[`
/// found outside of comments and string literals.
///
/// The returned slice is intended to be written back verbatim to preserve any
/// leading `//` comment block (Copilot CLI emits two such lines). If no JSON
/// container start is found, the entire input is returned.
pub fn leading_comment_prefix(input: &str) -> &str {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    enum State {
        Normal,
        InString,
        InLineComment,
        InBlockComment,
    }
    let mut state = State::Normal;
    let mut escape = false;

    while i < len {
        let b = bytes[i];
        match state {
            State::Normal => {
                if b == b'{' || b == b'[' {
                    return &input[..i];
                }
                if b == b'"' {
                    state = State::InString;
                    escape = false;
                    i += 1;
                } else if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
                    state = State::InLineComment;
                    i += 2;
                } else if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    state = State::InBlockComment;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            State::InString => {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::InLineComment => {
                if b == b'\n' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::InBlockComment => {
                if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                    state = State::Normal;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
    }

    input
}

/// Concatenate a preserved JSONC `prefix` (typically a leading `//` comment
/// block) with a freshly-serialized JSON `body`, inserting a single newline
/// separator when the prefix is non-empty and does not already end in one.
pub fn apply_prefix(prefix: &str, body: String) -> String {
    if prefix.is_empty() {
        return body;
    }
    let need_sep = !prefix.ends_with('\n');
    let mut out = String::with_capacity(prefix.len() + body.len() + usize::from(need_sep));
    out.push_str(prefix);
    if need_sep {
        out.push('\n');
    }
    out.push_str(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_line_comments() {
        let src = "// User settings belong in settings.json.\n\
                   // This file is managed automatically.\n\
                   {\"a\": 1}\n";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse stripped");
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn strips_inline_line_comment_after_value() {
        let src = "{\"a\": 1 // trailing\n, \"b\": 2}\n";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse stripped");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strips_block_comment() {
        let src = "{/* hi */\"a\": 1}";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse stripped");
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn block_comment_does_not_nest() {
        // The first `*/` closes the block; the trailing `*/` is plain JSON
        // content (illegal JSON, but proves non-nesting). We verify the
        // stripper consumed only up to the first `*/`.
        let src = "/* outer /* inner */AFTER";
        let out = strip_jsonc_comments(src);
        assert_eq!(out, "AFTER");
    }

    #[test]
    fn preserves_double_slash_inside_string() {
        let src = "{\"url\": \"https://example.com//path\"}";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse stripped");
        assert_eq!(v["url"], "https://example.com//path");
    }

    #[test]
    fn preserves_block_comment_markers_inside_string() {
        let src = "{\"s\": \"/* not a comment */\"}";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse stripped");
        assert_eq!(v["s"], "/* not a comment */");
    }

    #[test]
    fn handles_escaped_quote_inside_string() {
        let src = "{\"s\": \"a\\\"// still in string\"}";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(&out).expect("parse stripped");
        assert_eq!(v["s"], "a\"// still in string");
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(strip_jsonc_comments(""), "");
    }

    #[test]
    fn unterminated_string_at_eof_does_not_panic() {
        let src = "{\"s\": \"unterminated";
        let _ = strip_jsonc_comments(src); // Must not panic.
    }

    #[test]
    fn unterminated_block_comment_at_eof_does_not_panic() {
        let src = "{/* never closes";
        let _ = strip_jsonc_comments(src); // Must not panic.
    }

    #[test]
    fn line_comment_at_eof_without_newline() {
        let src = "{\"a\": 1}\n// trailing";
        let out = strip_jsonc_comments(src);
        let v: serde_json::Value = serde_json::from_str(out.trim()).expect("parse stripped");
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn leading_prefix_returns_comment_block_before_object() {
        let src = "// one\n// two\n{\"a\": 1}\n";
        let prefix = leading_comment_prefix(src);
        assert_eq!(prefix, "// one\n// two\n");
    }

    #[test]
    fn leading_prefix_empty_when_object_starts_at_zero() {
        let src = "{\"a\": 1}";
        assert_eq!(leading_comment_prefix(src), "");
    }

    #[test]
    fn leading_prefix_for_array_root() {
        let src = "// hi\n[1,2,3]";
        assert_eq!(leading_comment_prefix(src), "// hi\n");
    }

    #[test]
    fn leading_prefix_returns_full_input_when_no_container() {
        let src = "// only comments\n// and more\n";
        assert_eq!(leading_comment_prefix(src), src);
    }

    #[test]
    fn leading_prefix_ignores_braces_inside_comments_and_strings() {
        // The `{` inside the block comment and inside the string must not be
        // mistaken for the JSON container start.
        let src = "/* { */// also { in line\n\"prefix\":{\"k\":\"{\"}";
        // The first real `{` is after the strings/comments — at the object literal.
        // Note: a leading `"prefix"` outside braces is malformed JSON, but
        // `leading_comment_prefix` is a pure scanner; it should still return
        // the substring up to the first unquoted/uncommented `{` or `[`.
        let prefix = leading_comment_prefix(src);
        // Prefix must include the comments and the `"prefix":` token, and
        // must NOT include the first `{` itself.
        assert!(prefix.starts_with("/* { */"));
        assert!(prefix.ends_with(":"));
        assert!(!prefix.contains("\"k\""));
    }

    #[test]
    fn apply_prefix_handles_empty_and_separator_cases() {
        assert_eq!(apply_prefix("", "{}".to_string()), "{}");
        assert_eq!(apply_prefix("// hi\n", "{}".to_string()), "// hi\n{}");
        assert_eq!(apply_prefix("// hi", "{}".to_string()), "// hi\n{}");
    }
}
