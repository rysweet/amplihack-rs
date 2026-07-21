//! Defensive utilities for working with LLM responses.
//!
//! Ported from `amplihack/utils/defensive.py`. Provides JSON extraction from
//! messy LLM output, file I/O with retry, prompt sanitization, and lightweight
//! schema validation.

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced by defensive utility functions.
#[derive(Debug, Error)]
pub enum DefensiveError {
    /// An I/O operation failed after exhausting all retries.
    #[error("I/O failed after {retries} retries: {source}")]
    IoRetryExhausted {
        /// Number of retries that were attempted.
        retries: u32,
        /// The underlying I/O error from the final attempt.
        source: std::io::Error,
    },

    /// A retried callback failed after exhausting all retries.
    #[error("retry exhausted after {retries} attempts: {last_error}")]
    RetryExhausted {
        /// Number of retries that were attempted.
        retries: u32,
        /// Description of the final error.
        last_error: String,
    },
}

/// Why JSON could not be extracted from an LLM response.
///
/// A silent fallback is a silent failure: [`parse_llm_json_result`] reports
/// *why* extraction failed so callers can log and branch (for example, retry a
/// corrupt payload but treat a missing one as "the model produced no JSON").
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseLlmJsonError {
    /// The response contained no JSON payload at all: no `{`/`[` delimiter and
    /// no fenced code block were present (e.g. the model replied in prose).
    #[error("no JSON payload found in LLM output")]
    Missing,

    /// A JSON payload was clearly attempted (a delimiter or fenced block was
    /// present) but it could not be parsed.
    #[error("JSON payload present but could not be parsed: {detail}")]
    Corrupt {
        /// The underlying parser message (or a description of the structural
        /// problem when no parser message is available).
        detail: String,
    },
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

/// Regex for ```json ... ``` fenced blocks.
static JSON_FENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```(?:json)?\s*\n?(.*?)\n?\s*```").expect("JSON_FENCE regex is valid")
});

/// Extract and parse JSON from LLM response text.
///
/// Handles multiple formats commonly emitted by language models:
/// 1. Raw JSON (the entire string is valid JSON)
/// 2. Fenced code blocks (` ```json … ``` `)
/// 3. First `{…}` or `[…]` substring (greedy brace/bracket matching)
///
/// Returns `None` when no valid JSON can be extracted. This is the best-effort
/// wrapper over [`parse_llm_json_result`]: `parse_llm_json(x)` is exactly
/// `parse_llm_json_result(x).ok()`. Prefer [`parse_llm_json_result`] when you
/// need to distinguish a *missing* payload from a *corrupt* one.
///
/// # Examples
///
/// ```
/// use amplihack_utils::parse_llm_json;
///
/// let raw = r#"{"key": "value"}"#;
/// assert!(parse_llm_json(raw).is_some());
///
/// let fenced = "Here is the output:\n```json\n{\"a\": 1}\n```\nDone.";
/// assert!(parse_llm_json(fenced).is_some());
/// ```
pub fn parse_llm_json(text: &str) -> Option<serde_json::Value> {
    parse_llm_json_result(text).ok()
}

/// Extract and parse JSON from LLM response text, reporting *why* it failed.
///
/// Uses the same extraction strategy as [`parse_llm_json`] (raw parse → fenced
/// code block → first balanced `{…}`/`[…]` substring), but distinguishes the
/// two failure modes so callers can surface them instead of silently degrading:
///
/// * [`ParseLlmJsonError::Missing`] — no delimiter and no fence were present,
///   so the model produced no JSON at all.
/// * [`ParseLlmJsonError::Corrupt`] — a JSON payload was attempted (a delimiter
///   or fenced block was present) but could not be parsed.
///
/// # Errors
///
/// Returns [`ParseLlmJsonError`] describing the failure.
///
/// # Examples
///
/// ```
/// use amplihack_utils::defensive::{parse_llm_json_result, ParseLlmJsonError};
///
/// assert!(parse_llm_json_result(r#"{"a": 1}"#).is_ok());
/// assert_eq!(parse_llm_json_result("just prose"), Err(ParseLlmJsonError::Missing));
/// assert!(matches!(
///     parse_llm_json_result(r#"{"a": }"#),
///     Err(ParseLlmJsonError::Corrupt { .. })
/// ));
/// ```
pub fn parse_llm_json_result(text: &str) -> Result<serde_json::Value, ParseLlmJsonError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ParseLlmJsonError::Missing);
    }

    // 1. Try raw parse.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Ok(v);
    }

    // Track the most specific parser error so a `Corrupt` result carries a
    // useful message rather than an empty string.
    let mut last_error: Option<String> = None;

    // 2. Try fenced code blocks.
    let mut saw_fence = false;
    for cap in JSON_FENCE.captures_iter(text) {
        if let Some(inner) = cap.get(1) {
            saw_fence = true;
            match serde_json::from_str::<serde_json::Value>(inner.as_str().trim()) {
                Ok(v) => return Ok(v),
                Err(e) => last_error = Some(e.to_string()),
            }
        }
    }

    // 3. Try extracting the first top-level JSON object or array.
    if let Some(extracted) = extract_balanced_json(trimmed) {
        match serde_json::from_str::<serde_json::Value>(&extracted) {
            Ok(v) => return Ok(v),
            Err(e) => last_error = Some(e.to_string()),
        }
    }

    // Nothing parsed. Distinguish a genuine JSON attempt (a delimiter or fence
    // was present) from output that never contained JSON at all.
    if saw_fence || find_json_start(trimmed).is_some() {
        Err(ParseLlmJsonError::Corrupt {
            detail: last_error
                .unwrap_or_else(|| "unbalanced or truncated JSON payload".to_string()),
        })
    } else {
        Err(ParseLlmJsonError::Missing)
    }
}

/// Walk through `text` and return the first balanced `{…}` or `[…]` substring.
fn extract_balanced_json(text: &str) -> Option<String> {
    let (open, close) = find_json_start(text)?;
    let chars: Vec<char> = text.chars().collect();
    let start_idx = chars.iter().position(|&c| c == open)?;

    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    let mut end_idx = None;

    for (i, &ch) in chars.iter().enumerate().skip(start_idx) {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                end_idx = Some(i);
                break;
            }
        }
    }

    end_idx.map(|ei| chars[start_idx..=ei].iter().collect())
}

/// Return the matching open/close pair for the first JSON delimiter found.
fn find_json_start(text: &str) -> Option<(char, char)> {
    for ch in text.chars() {
        match ch {
            '{' => return Some(('{', '}')),
            '[' => return Some(('[', ']')),
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Retry helpers
// ---------------------------------------------------------------------------

/// Retry a fallible function with exponential back-off.
///
/// `f` is called up to `max_retries + 1` times (one initial attempt plus
/// `max_retries` retries). The delay doubles after each failure, starting
/// from `initial_delay`.
///
/// # Errors
///
/// Returns [`DefensiveError::RetryExhausted`] if every attempt fails.
///
/// # Examples
///
/// ```
/// use amplihack_utils::defensive::retry_with_feedback;
/// use std::time::Duration;
///
/// let mut counter = 0u32;
/// let result = retry_with_feedback(
///     || {
///         counter += 1;
///         if counter < 3 { Err("not yet".into()) } else { Ok(counter) }
///     },
///     3,
///     Duration::from_millis(1),
/// );
/// assert_eq!(result.unwrap(), 3);
/// ```
pub fn retry_with_feedback<T, F>(
    mut f: F,
    max_retries: u32,
    initial_delay: Duration,
) -> Result<T, DefensiveError>
where
    F: FnMut() -> Result<T, String>,
{
    let mut delay = initial_delay;
    let mut last_error = String::new();

    for attempt in 0..=max_retries {
        match f() {
            Ok(val) => return Ok(val),
            Err(e) => {
                tracing::warn!(attempt, error = %e, "retry_with_feedback: attempt failed");
                last_error = e;
                if attempt < max_retries {
                    std::thread::sleep(delay);
                    delay *= 2;
                }
            }
        }
    }

    Err(DefensiveError::RetryExhausted {
        retries: max_retries,
        last_error,
    })
}

// ---------------------------------------------------------------------------
// Prompt isolation
// ---------------------------------------------------------------------------

/// Regex matching XML-style tags.
static XML_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"</?[a-zA-Z][a-zA-Z0-9_-]*(?:\s[^>]*)?>").expect("XML_TAGS regex is valid")
});

/// Regex matching common LLM role prefixes like "Assistant:" or "Human:".
static ROLE_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(Assistant|Human|System|User)\s*:\s*").expect("ROLE_PREFIX regex is valid")
});

/// Strip XML tags, markdown artifacts, and LLM role prefixes from text.
///
/// Useful for isolating the actual content from LLM responses that contain
/// formatting noise.
///
/// # Examples
///
/// ```
/// use amplihack_utils::defensive::isolate_prompt;
///
/// let noisy = "Assistant: <thinking>internal</thinking>Hello!";
/// assert_eq!(isolate_prompt(noisy), "internalHello!");
/// ```
pub fn isolate_prompt(text: &str) -> String {
    let no_xml = XML_TAGS.replace_all(text, "");
    let no_roles = ROLE_PREFIX.replace_all(&no_xml, "");
    // Collapse multiple blank lines and trim.
    let lines: Vec<&str> = no_roles
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// File I/O with retry
// ---------------------------------------------------------------------------

/// Read a file to string, retrying on transient I/O errors.
///
/// # Errors
///
/// Returns [`DefensiveError::IoRetryExhausted`] if every attempt fails.
pub fn read_file_with_retry(
    path: &Path,
    max_retries: u32,
    delay: Duration,
) -> Result<String, DefensiveError> {
    let mut current_delay = delay;
    let mut last_err: Option<std::io::Error> = None;

    for attempt in 0..=max_retries {
        match std::fs::read_to_string(path) {
            Ok(content) => return Ok(content),
            Err(e) => {
                tracing::warn!(
                    attempt,
                    path = %path.display(),
                    error = %e,
                    "read_file_with_retry: attempt failed"
                );
                last_err = Some(e);
                if attempt < max_retries {
                    std::thread::sleep(current_delay);
                    current_delay *= 2;
                }
            }
        }
    }

    Err(DefensiveError::IoRetryExhausted {
        retries: max_retries,
        source: last_err.expect("at least one attempt was made"),
    })
}

/// Write content to a file, retrying on transient I/O errors.
///
/// Creates parent directories if they do not exist.
///
/// # Errors
///
/// Returns [`DefensiveError::IoRetryExhausted`] if every attempt fails.
pub fn write_file_with_retry(
    path: &Path,
    content: &str,
    max_retries: u32,
    delay: Duration,
) -> Result<(), DefensiveError> {
    let mut current_delay = delay;
    let mut last_err: Option<std::io::Error> = None;

    for attempt in 0..=max_retries {
        // Ensure parent dir exists on each attempt (idempotent).
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(
                attempt,
                path = %path.display(),
                error = %e,
                "write_file_with_retry: mkdir failed"
            );
            last_err = Some(e);
            if attempt < max_retries {
                std::thread::sleep(current_delay);
                current_delay *= 2;
            }
            continue;
        }

        match std::fs::write(path, content) {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::warn!(
                    attempt,
                    path = %path.display(),
                    error = %e,
                    "write_file_with_retry: write failed"
                );
                last_err = Some(e);
                if attempt < max_retries {
                    std::thread::sleep(current_delay);
                    current_delay *= 2;
                }
            }
        }
    }

    Err(DefensiveError::IoRetryExhausted {
        retries: max_retries,
        source: last_err.expect("at least one attempt was made"),
    })
}

// ---------------------------------------------------------------------------
// Schema validation
// ---------------------------------------------------------------------------

/// Validate that a JSON object contains all required fields.
///
/// Returns a list of field names that are missing from `data`. An empty
/// vector means validation passed.
///
/// # Examples
///
/// ```
/// use amplihack_utils::validate_json_schema;
/// use serde_json::json;
///
/// let data = json!({"name": "Alice", "age": 30});
/// let missing = validate_json_schema(&data, &["name", "email"]);
/// assert_eq!(missing, vec!["email"]);
/// ```
pub fn validate_json_schema(data: &serde_json::Value, required_fields: &[&str]) -> Vec<String> {
    let obj = match data.as_object() {
        Some(o) => o,
        None => return required_fields.iter().map(|f| (*f).to_owned()).collect(),
    };

    required_fields
        .iter()
        .filter(|field| !obj.contains_key(**field))
        .map(|field| (*field).to_owned())
        .collect()
}

#[cfg(test)]
#[path = "tests/defensive_tests.rs"]
mod tests;
