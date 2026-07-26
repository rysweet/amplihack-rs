//! R4 — full-conversation mirroring helpers: outbound secret redaction + size
//! bounding + a cross-process echo-suppression fingerprint.
//!
//! Mirroring every user prompt and assistant turn to the Signal group raises
//! three hazards:
//!
//! 1. **Secret leakage.** Prompts and assistant turns routinely contain pasted
//!    or echoed credentials. Mirroring them verbatim would copy those secrets
//!    off-box into a (potentially multi-member) group chat, crossing the
//!    project's "redact before it leaves the machine" boundary. [`redact_for_relay`]
//!    scrubs the high-frequency secret shapes first; [`prepare_for_relay`] chains
//!    it ahead of truncation.
//! 2. **Unbounded size.** Assistant turns can be huge. [`truncate_for_relay`]
//!    bounds each mirrored body to [`RELAY_MAX_BYTES`] at a UTF-8 char boundary
//!    (never splitting a multibyte code point) and appends a visible truncation
//!    marker.
//! 3. **Echo loops across processes.** The outbound mirror runs in the (short-
//!    lived) hook process while the inbound subscriber runs detached, so the
//!    in-memory echo window in `amplihack_signal::gating::Gate` cannot span the
//!    two. [`record_outbound_fingerprint`] persists a hashed, per-session
//!    fingerprint of each mirrored body that the subscriber checks via
//!    [`is_recent_outbound_fingerprint`] to drop the account's own synced-back
//!    messages instead of re-injecting them.
//!
//! Both fingerprint seams take an explicit `root`, keeping them hermetic.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest, Sha256};

/// Maximum bytes of a single mirrored message body (4 KiB).
pub const RELAY_MAX_BYTES: usize = 4096;

/// Marker appended to a truncated body. Placed so the total length up to the
/// word `truncated` never exceeds the byte cap (the cap reserves room for it).
const TRUNCATION_MARKER: &str = " [truncated]";

/// Secret shapes scrubbed from a body before it is mirrored to Signal, applied
/// in order. Full-conversation mirroring copies raw user prompts and assistant
/// turns off the box into a group chat that — in the fleet-watch scenario this
/// PR enables — can have multiple members. That crosses the project's standing
/// "redact before it leaves the machine" boundary (cf. `amplihack-remote::redact`,
/// issue #882 / CWE-532), so pasted or echoed credentials must not survive.
///
/// The list is deliberately conservative and deterministic: it targets the
/// high-frequency, unambiguously-secret token shapes rather than trying to
/// classify arbitrary prose (which would risk mangling normal conversation).
/// The whole `key = value` form is redacted first so its `[REDACTED]`
/// placeholder cannot re-expose a value that a later, narrower pattern would
/// otherwise leave partially intact.
static REDACTION_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let specs: &[(&str, &str)] = &[
        // PEM private-key blocks (multi-line): drop the whole armored body.
        (
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
            "[REDACTED-PRIVATE-KEY]",
        ),
        // Signal device-link URIs (linking another device would hijack the account).
        (
            r"(?i)\b(?:sgnl://linkdevice|tsdevice:/)\?[^\s<>'\x22]+",
            "[REDACTED-SIGNAL-LINK]",
        ),
        // `name: value` / `name = value` credential assignments. The value may
        // be prefixed by an HTTP auth scheme word (`Bearer`/`Basic`/`Token`),
        // as in `Authorization: Bearer <jwt>`; that prefix is consumed as part
        // of the value so the credential is fully redacted. Without this, the
        // key/value match would stop at the scheme word (treating `Bearer` as
        // the "value"), strip it, and leave the real token exposed for the
        // later, now-unanchored Bearer pattern to miss.
        (
            r#"(?i)\b(api[_-]?key|access[_-]?key|secret|token|password|passwd|pwd|credential|authorization)\b\s*[:=]\s*['"]?(?:(?:bearer|basic|token)\s+)?[A-Za-z0-9._~+/=:-]{6,}['"]?"#,
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
        // Credentials embedded in a URL's userinfo
        // (`scheme://user:password@host`): keep the scheme + username, drop the
        // password so pasted connection strings do not leak off-box.
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
            "Bearer [REDACTED]",
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

/// Scrub high-frequency secret shapes out of `body` before it is mirrored to a
/// Signal group. Pure, allocation-only, and idempotent: it performs no I/O and
/// running it twice yields the same result. Non-secret conversation text is
/// preserved so the mirror stays useful.
///
/// See [`REDACTION_PATTERNS`] for the covered secret classes and rationale.
#[must_use]
pub fn redact_for_relay(body: &str) -> String {
    let mut out = Cow::Borrowed(body);
    for (re, replacement) in REDACTION_PATTERNS.iter() {
        // `replace_all` returns `Cow::Borrowed` when nothing matches (the common
        // case for benign conversation), so only adopt a new buffer on a real
        // match instead of allocating a full-body copy for every pattern.
        if let Cow::Owned(replaced) = re.replace_all(&out, *replacement) {
            out = Cow::Owned(replaced);
        }
    }
    out.into_owned()
}

/// Prepare a body for Signal relay: redact secrets first, then byte-bound the
/// result. Redacting before truncating guarantees a secret is scrubbed even
/// when it would otherwise straddle (or sit past) the truncation boundary, and
/// means the persisted echo-suppression fingerprint is taken over the exact
/// bytes that are sent.
#[must_use]
pub fn prepare_for_relay(body: &str, max: usize) -> String {
    truncate_for_relay(&redact_for_relay(body), max)
}

/// Number of most-recent outbound fingerprints considered "recent" for
/// echo-suppression. Matching is restricted to this trailing window so that a
/// short operator instruction (e.g. "continue") that merely coincides with a
/// long-past mirrored line is still delivered rather than silently dropped.
/// The on-disk log is also trimmed toward this size to bound growth.
const FP_WINDOW: usize = 128;

/// Bound `body` to at most `max` bytes at a UTF-8 char boundary, appending a
/// visible truncation marker when shortened. Short or empty bodies are returned
/// unchanged.
#[must_use]
pub fn truncate_for_relay(body: &str, max: usize) -> String {
    if body.len() <= max {
        return body.to_string();
    }
    // Reserve room for the marker so the mirrored prefix (including the marker
    // text preceding "truncated") never exceeds `max`.
    let budget = max.saturating_sub(TRUNCATION_MARKER.len());
    let mut end = budget.min(body.len());
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + TRUNCATION_MARKER.len());
    out.push_str(&body[..end]);
    out.push_str(TRUNCATION_MARKER);
    out
}

/// Lowercase hex SHA-256 of `s`.
fn hash_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Per-session fingerprint log path under `root` (session id is hashed into the
/// filename so it is filesystem-safe and cannot escape `root`).
fn session_fp_path(root: &Path, session_id: &str) -> PathBuf {
    root.join("signal-outbound-fp").join(hash_hex(session_id))
}

/// Persist a fingerprint of an outbound (mirrored) `body` for `session_id` under
/// `root`, so a detached subscriber can later recognize the echo.
///
/// The log is kept bounded to the most recent [`FP_WINDOW`] fingerprints: after
/// appending, an over-long file is rewritten to retain only the tail. This caps
/// both on-disk growth and the per-inbound scan cost over a long session.
pub fn record_outbound_fingerprint(root: &Path, session_id: &str, body: &str) -> io::Result<()> {
    let path = session_fp_path(root, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", hash_hex(body))?;
    }
    // Bound the file to the most recent FP_WINDOW entries. Only rewrite when it
    // has grown past a hysteresis threshold to avoid rewriting on every call.
    // The trim is done atomically (write a sibling temp file, then rename over
    // the target) so the detached subscriber, which reads this file
    // concurrently, never observes a truncated/partial state — it sees either
    // the full pre-trim content or the full trimmed content.
    if let Ok(content) = std::fs::read_to_string(&path) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() > FP_WINDOW * 2 {
            let tail = lines[lines.len() - FP_WINDOW..].join("\n");
            let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
            std::fs::write(&tmp, format!("{tail}\n"))?;
            std::fs::rename(&tmp, &path)?;
        }
    }
    Ok(())
}

/// Whether `body` was recently mirrored outbound for `session_id` under `root`
/// (i.e. its fingerprint appears within the most recent [`FP_WINDOW`] mirrored
/// entries). Fingerprints are isolated per session.
///
/// Matching is deliberately restricted to a recent window rather than the whole
/// session history: an inbound operator message whose text happens to equal a
/// long-past mirrored line (realistic for short instructions like "continue" or
/// "yes") must still reach the CLI rather than being suppressed as a self-echo.
#[must_use]
pub fn is_recent_outbound_fingerprint(root: &Path, session_id: &str, body: &str) -> bool {
    let path = session_fp_path(root, session_id);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    let fp = hash_hex(body);
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(FP_WINDOW);
    lines[start..].iter().any(|line| line.trim() == fp)
}

/// Remove the persisted outbound-fingerprint log for `session_id` under `root`.
/// Called during per-session teardown so the log does not outlive the session.
pub fn clear_outbound_fingerprints(root: &Path, session_id: &str) {
    let path = session_fp_path(root, session_id);
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn redacts_authorization_bearer_header_fully() {
        // Regression: the `key: value` pattern lists `authorization` and runs
        // before the standalone Bearer pattern. It must consume the `Bearer`
        // scheme word *and* the token, not stop at `Bearer` and leak the token.
        let jwt = format!("eyJ0eXAiOiJKV1Q.{}.{}", "A".repeat(24), "B".repeat(24));
        let out = redact_for_relay(&format!("Authorization: Bearer {jwt}"));
        assert!(
            !out.contains(&jwt),
            "bearer token after an Authorization key must be redacted: {out}"
        );
        assert!(out.contains("[REDACTED]"), "expected redaction: {out}");

        // The `=` / no-space variant must also be fully redacted.
        let tok = format!("sk-{}", "C".repeat(32));
        let out = redact_for_relay(&format!("authorization=Bearer {tok}"));
        assert!(
            !out.contains(&tok),
            "token leaked via key=Bearer form: {out}"
        );
    }

    #[test]
    fn redacts_google_api_key() {
        // AIza prefix + exactly 35 url-safe chars.
        let key = format!(
            "AIza{}",
            "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R"
                .chars()
                .take(35)
                .collect::<String>()
        );
        assert_eq!(key.len(), 39, "google key fixture must be AIza + 35 chars");
        let out = redact_for_relay(&format!("GOOGLE_API_KEY {key} done"));
        assert!(!out.contains(&key), "google api key leaked: {out}");
        assert!(out.contains("[REDACTED-GOOGLE-KEY]"), "got: {out}");
    }

    #[test]
    fn redacts_url_userinfo_password() {
        let pw = "supersecretpw123";
        let out = redact_for_relay(&format!("postgres://svc:{pw}@db.internal:5432/prod"));
        assert!(!out.contains(pw), "url password leaked: {out}");
        assert!(out.contains("[REDACTED]"), "expected redaction: {out}");
        // Scheme + username are preserved for usefulness.
        assert!(out.contains("postgres://svc:"), "context lost: {out}");
    }

    #[test]
    fn redacts_github_token() {
        let token = format!("{}{}{}", "gh", "p_", "A".repeat(36));
        let body = format!("here is my token {token} please use it");
        let out = redact_for_relay(&body);
        assert!(
            !out.contains(&token),
            "github token must be redacted: {out}"
        );
        assert!(out.contains("[REDACTED-GITHUB-TOKEN]"), "got: {out}");
        // Surrounding conversation context is preserved.
        assert!(out.contains("here is my token"), "context lost: {out}");
        assert!(out.contains("please use it"), "context lost: {out}");
    }

    #[test]
    fn redacts_key_value_secret_forms() {
        for (body, secret) in [
            (format!("api_key: {}", "A".repeat(16)), "A".repeat(16)),
            (format!("password = {}", "B".repeat(16)), "B".repeat(16)),
            (format!("AUTHORIZATION={}", "C".repeat(16)), "C".repeat(16)),
        ] {
            let out = redact_for_relay(&body);
            assert!(out.contains("[REDACTED]"), "expected redaction in {out}");
            assert!(!out.contains(&secret), "secret value leaked: {out}");
        }
    }

    #[test]
    fn redacts_bearer_and_signal_link() {
        let bearer = format!("{} {}", "Bearer", "D".repeat(32));
        let out = redact_for_relay(&format!("call it with {bearer}"));
        assert!(!out.contains(&bearer), "bearer leaked: {out}");
        assert!(
            out.starts_with("call it with "),
            "surrounding context lost: {out}"
        );

        let out = redact_for_relay("link at sgnl://linkdevice?uuid=abc&pub_key=deadbeef now");
        assert!(!out.contains("deadbeef"), "signal link leaked: {out}");
        assert!(out.contains("[REDACTED-SIGNAL-LINK]"), "got: {out}");
        assert!(
            out.contains("link at") && out.contains("now"),
            "context lost: {out}"
        );
    }

    #[test]
    fn redaction_preserves_benign_text_and_is_idempotent() {
        let benign = "Let's refactor the parser and run the tests, then push the branch.";
        assert_eq!(
            redact_for_relay(benign),
            benign,
            "benign text must pass through"
        );

        let secret = format!("token={} and more text", "E".repeat(16));
        let once = redact_for_relay(&secret);
        assert_eq!(
            redact_for_relay(&once),
            once,
            "redaction must be idempotent"
        );
    }

    #[test]
    fn prepare_redacts_before_truncating() {
        // A secret sitting past the byte cap must still be scrubbed: redaction
        // runs on the full body before truncation.
        let secret = format!("{}{}{}", "gh", "p_", "F".repeat(36));
        let body = format!("{}{secret}", "x".repeat(RELAY_MAX_BYTES));
        let out = prepare_for_relay(&body, RELAY_MAX_BYTES);
        assert!(!out.contains(&secret), "secret past the cap leaked: {out}");
        assert!(
            out.len() <= RELAY_MAX_BYTES,
            "prepared body must respect the byte cap, got {}",
            out.len()
        );
    }

    #[test]
    fn recent_fingerprint_matches_within_window() {
        let td = TempDir::new().unwrap();
        record_outbound_fingerprint(td.path(), "s", "hello").unwrap();
        assert!(is_recent_outbound_fingerprint(td.path(), "s", "hello"));
        assert!(!is_recent_outbound_fingerprint(td.path(), "s", "other"));
    }

    #[test]
    fn old_fingerprint_outside_window_is_not_recent() {
        // A short operator instruction that coincides with a long-past mirrored
        // line must still be delivered (not suppressed as an echo).
        let td = TempDir::new().unwrap();
        record_outbound_fingerprint(td.path(), "s", "continue").unwrap();
        for i in 0..(FP_WINDOW * 2) {
            record_outbound_fingerprint(td.path(), "s", &format!("line-{i}")).unwrap();
        }
        assert!(
            !is_recent_outbound_fingerprint(td.path(), "s", "continue"),
            "a fingerprint older than FP_WINDOW must not count as recent"
        );
        // The most recent entry is still recognized.
        let last = format!("line-{}", FP_WINDOW * 2 - 1);
        assert!(is_recent_outbound_fingerprint(td.path(), "s", &last));
    }

    #[test]
    fn log_is_bounded_in_size() {
        let td = TempDir::new().unwrap();
        for i in 0..(FP_WINDOW * 4) {
            record_outbound_fingerprint(td.path(), "s", &format!("m-{i}")).unwrap();
        }
        let content = std::fs::read_to_string(session_fp_path(td.path(), "s")).unwrap();
        let lines = content.lines().count();
        assert!(
            lines <= FP_WINDOW * 2,
            "fingerprint log must stay bounded, got {lines} lines"
        );
    }

    #[test]
    fn clear_removes_fingerprint_log() {
        let td = TempDir::new().unwrap();
        record_outbound_fingerprint(td.path(), "s", "x").unwrap();
        assert!(is_recent_outbound_fingerprint(td.path(), "s", "x"));
        clear_outbound_fingerprints(td.path(), "s");
        assert!(!is_recent_outbound_fingerprint(td.path(), "s", "x"));
    }
}
