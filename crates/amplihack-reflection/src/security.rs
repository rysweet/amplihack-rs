//! Content sanitization for reflection output (port of `security.py`).
//!
//! Uses static compiled regex tables to redact secrets before display
//! and to truncate overlong content. No user-supplied regex compilation.

use once_cell::sync::Lazy;
use regex::Regex;

const REDACTED: &str = "[REDACTED]";
const TRUNCATED_SUFFIX: &str = "...[TRUNCATED]";

static SENSITIVE_KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "token",
    "auth",
    "bearer",
    "key",
    "secret",
    "private",
    "credential",
    "cred",
    "api_key",
    "apikey",
    "oauth",
];

// Patterns mirror reflection/security.py. Built once, applied in order.
static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    let raw = [
        // key=value style credentials
        r#"(?i)\b(?:password|passwd|pwd|token|auth|bearer|key|secret|private)\s*[=:]\s*[^\s'"]+"#,
        r#"(?i)\b(?:credential|cred|api_key|apikey)\s*[=:]\s*[^\s'"]+"#,
        // long alphanumeric / hex blobs
        r"(?i)\b(?:[A-Za-z0-9]{20,}|[A-Fa-f0-9]{32,})\b",
        // env var references containing secret words
        r"(?i)\$\{?[A-Z_]*(?:KEY|SECRET|TOKEN|PASSWORD|CRED)[A-Z_]*\}?",
        // URLs with basic-auth credentials
        r"(?i)https?://[^/\s]*:[^@\s]*@[^\s]+",
    ];
    raw.iter()
        .map(|p| Regex::new(p).expect("static regex"))
        .collect()
});

/// Stateless sanitizer; cheap to construct.
#[derive(Default, Debug, Clone, Copy)]
pub struct ContentSanitizer;

impl ContentSanitizer {
    pub fn new() -> Self {
        Self
    }

    /// Redact secrets and truncate to `max_length`.
    pub fn sanitize_content(&self, content: &str, max_length: usize) -> String {
        // Early bound to avoid pathological work on huge inputs.
        let cap = max_length.saturating_mul(3).max(max_length + 32);
        let mut work: String = if content.len() > cap {
            content.chars().take(cap).collect()
        } else {
            content.to_string()
        };

        for pat in PATTERNS.iter() {
            work = pat.replace_all(&work, REDACTED).into_owned();
        }

        if work.contains('\n') {
            work = work
                .split('\n')
                .map(|line| {
                    let lower = line.to_ascii_lowercase();
                    if SENSITIVE_KEYWORDS.iter().any(|k| lower.contains(k)) {
                        if line.contains(REDACTED) {
                            line.to_string()
                        } else {
                            "[LINE WITH SENSITIVE DATA REDACTED]".to_string()
                        }
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
        }

        if work.len() > max_length {
            let head_target = max_length.saturating_sub(TRUNCATED_SUFFIX.len());
            // Truncate on a UTF-8 char boundary <= head_target.
            let mut end = head_target.min(work.len());
            while end > 0 && !work.is_char_boundary(end) {
                end -= 1;
            }
            let mut out = String::with_capacity(end + TRUNCATED_SUFFIX.len());
            out.push_str(&work[..end]);
            out.push_str(TRUNCATED_SUFFIX);
            return out;
        }
        work
    }

    /// Build a short preview string suitable for inline display.
    pub fn create_safe_preview(&self, content: &str, context: &str) -> String {
        let mut out = self.sanitize_content(content, 100);
        if out.len() > 50 {
            let mut end = 47;
            while end > 0 && !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str("...");
        }
        if context.is_empty() {
            out
        } else {
            format!("{context}: {out}")
        }
    }

    /// Sanitize a free-form pattern suggestion. Returns generic text if the
    /// suggestion still contains a sensitive keyword after redaction.
    pub fn filter_pattern_suggestion(&self, suggestion: &str) -> String {
        let safe = self.sanitize_content(suggestion, 150);
        let lower = safe.to_ascii_lowercase();
        // Only generic-fallback if a sensitive keyword leaked through *outside*
        // a [REDACTED] marker.
        let stripped = lower.replace("[redacted]", "");
        if SENSITIVE_KEYWORDS.iter().any(|k| stripped.contains(k)) {
            return "Improve security and data handling practices".to_string();
        }
        safe
    }
}

/// Module-level convenience wrappers.
pub fn sanitize_content(content: &str, max_length: usize) -> String {
    ContentSanitizer::new().sanitize_content(content, max_length)
}

pub fn create_safe_preview(content: &str, context: &str) -> String {
    ContentSanitizer::new().create_safe_preview(content, context)
}

pub fn filter_pattern_suggestion(suggestion: &str) -> String {
    ContentSanitizer::new().filter_pattern_suggestion(suggestion)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benign_passthrough() {
        let s = ContentSanitizer::new();
        assert_eq!(s.sanitize_content("hello world", 200), "hello world");
    }
}
