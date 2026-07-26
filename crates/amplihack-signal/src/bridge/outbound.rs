//! Outbound secret redaction, applied **before** chunking.
//!
//! An accepted agent turn is captured verbatim from `copilot` stdout and can
//! contain pasted or echoed credentials. Because the group may have more than
//! one member, secrets must be scrubbed before any byte leaves the machine.
//! [`redact_for_relay`] scrubs the high-frequency secret shapes; the bridge
//! always pipes through [`redact_and_chunk`] so redaction happens over the
//! **whole** body first and a secret can never straddle (and survive in) a
//! chunk boundary.
//!
//! The pattern set mirrors the proven full-conversation mirroring redactor in
//! `amplihack-hooks::signal_integration::outbound`; it is deliberately
//! conservative and deterministic (idempotent) so benign prose is preserved.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use super::chunk::chunk;

/// Secret shapes scrubbed from a body before it is relayed to Signal, applied
/// in order. The whole `key = value` form is redacted first so its placeholder
/// cannot re-expose a value a later, narrower pattern would leave intact.
static REDACTION_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let specs: &[(&str, &str)] = &[
        // PEM private-key blocks (multi-line): drop the whole armored body.
        (
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
            "[REDACTED-PRIVATE-KEY]",
        ),
        // Signal device-link URIs (linking a device would hijack the account).
        (
            r"(?i)\b(?:sgnl://linkdevice|tsdevice:/)\?[^\s<>'\x22]+",
            "[REDACTED-SIGNAL-LINK]",
        ),
        // `name: value` / `name = value` credential assignments. An optional
        // HTTP auth scheme word (Bearer/Basic/Token) is consumed as part of the
        // value so the credential is fully redacted.
        (
            r#"(?i)\b(api[_-]?key|access[_-]?key|secret|token|password|passwd|pwd|credential|authorization)\b['"]?\s*[:=]\s*['"]?(?:(?:bearer|basic|token)\s+)?[A-Za-z0-9._~+/=:-]{6,}['"]?"#,
            "$1=[REDACTED]",
        ),
        // GitHub tokens (PAT, OAuth, user-to-server, server-to-server, refresh).
        (
            r"\b(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{20,}\b",
            "[REDACTED-GITHUB-TOKEN]",
        ),
        // AWS access-key IDs.
        (r"\bAKIA[0-9A-Z]{16}\b", "[REDACTED-AWS-KEY]"),
        // Google API keys (fixed `AIza` prefix + 35 url-safe chars).
        (r"\bAIza[0-9A-Za-z_-]{35}\b", "[REDACTED-GOOGLE-KEY]"),
        // Credentials embedded in a URL's userinfo: keep scheme + username,
        // drop the password so pasted connection strings do not leak.
        (
            r"(?i)\b([a-z][a-z0-9+.-]*://[^\s:@/]+):[^\s@/]+@",
            "$1:[REDACTED]@",
        ),
        // Slack tokens.
        (
            r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
            "[REDACTED-SLACK-TOKEN]",
        ),
        // HTTP bearer credentials (JWTs and opaque tokens alike).
        (
            r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{12,}",
            "[REDACTED-BEARER]",
        ),
    ];
    specs
        .iter()
        .map(|(pattern, replacement)| {
            (
                Regex::new(pattern).expect("static relay redaction regex is valid"),
                *replacement,
            )
        })
        .collect()
});

/// Scrub high-frequency secret shapes out of `body`. Pure, idempotent, and
/// allocation-light (only adopts a new buffer on a real match).
#[must_use]
pub fn redact_for_relay(body: &str) -> String {
    let mut out = Cow::Borrowed(body);
    for (re, replacement) in REDACTION_PATTERNS.iter() {
        if let Cow::Owned(replaced) = re.replace_all(&out, *replacement) {
            out = Cow::Owned(replaced);
        }
    }
    out.into_owned()
}

/// Redact secrets over the whole body, **then** split into Signal-sized chunks.
///
/// Redacting before chunking guarantees no individual outbound message can leak
/// a secret that would otherwise straddle a chunk boundary.
#[must_use]
pub fn redact_and_chunk(body: &str) -> Vec<String> {
    chunk(&redact_for_relay(body))
}
