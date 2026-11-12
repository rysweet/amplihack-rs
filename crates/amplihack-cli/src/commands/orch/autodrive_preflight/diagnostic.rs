//! One sanitizer for subprocess diagnostics before they enter any output sink.
use regex::Regex;
use std::sync::LazyLock;

pub(super) fn sanitize(input: &str) -> String {
    static ESCAPES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\|$)|\[[0-?]*[ -/]*[@-~]|.)").unwrap()
    });
    static USERINFO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)[^\s/]+@").unwrap());
    static AUTH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?im)((?:proxy-)?authorization\s*[:=]\s*|(?:(?:(?:access|github|gh|gitlab)[_-]?)?token|password|passwd|secret|api[_-]?key|oauth[_-]?token)\s*[:=]\s*)[^\r\n&\s]*(?:[^\r\n]*)"
    ).unwrap()
    });
    static TOKENS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:gh[pousr]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|sk-[A-Za-z0-9_-]{16,})")
            .unwrap()
    });
    let text = ESCAPES.replace_all(input, "");
    let text: String = text
        .chars()
        .filter(|c| {
            (!c.is_control() || *c == '\n' || *c == '\t')
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect();
    let text = USERINFO.replace_all(&text, "${1}[REDACTED]@");
    let text = AUTH.replace_all(&text, "${1}[REDACTED]");
    let text = TOKENS.replace_all(&text, "[REDACTED]");
    let mut result: String = text.chars().take(8192).collect();
    if text.chars().count() > 8192 {
        result.push_str("\n[diagnostic truncated]");
    }
    result
}

/// Apply the same boundary to diagnostic evidence assembled from filenames or
/// operation metadata, before it reaches persistence, comments or final events.
pub(super) fn sanitize_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => *text = sanitize(text),
        serde_json::Value::Array(values) => values.iter_mut().for_each(sanitize_value),
        serde_json::Value::Object(values) => values.values_mut().for_each(sanitize_value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn issue_2188_removes_secrets_and_terminal_controls_but_keeps_context() {
        let input = format!(
            "\x1b[31mfetch failed\x1b[0m https://user:secret@github.com/a/b\nAuthorization: Bearer hidden\n\x1b]0;window\x07retry denied\x00\n{}",
            "x".repeat(20000)
        );
        let output = sanitize(&input);
        for secret in [
            "user:secret",
            "Bearer",
            "hidden",
            "window",
            "\x1b",
            "\x07",
            "\x00",
        ] {
            assert!(!output.contains(secret), "{secret:?}");
        }
        assert!(output.contains("fetch failed"));
        assert!(output.contains("retry denied"));
        assert!(output.len() < 8300);
    }
}
