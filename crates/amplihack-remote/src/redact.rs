//! Redaction of sensitive Azure identifiers from arbitrary text.
//!
//! `azlin` / Azure CLI stderr can contain sensitive identifiers — subscription
//! and tenant GUIDs, resource URIs, and SAS token query parameters (notably the
//! `sig` bearer-capability signature). This module scrubs those values before
//! they are folded into a [`crate::error::RemoteError`] or a log line.
//!
//! Addresses issue #882 (CWE-532: sensitive data written to logs).

use std::sync::LazyLock;

use regex::Regex;

/// Canonical Azure GUID (subscription / tenant / resource IDs): 8-4-4-4-12 hex.
static GUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
        .expect("static GUID redaction regex is valid")
});

/// SAS query-parameter `name=value` pairs. `sig` is a true bearer credential;
/// the remaining metadata params are conservatively redacted to avoid leaking
/// account/container/permission context. Parameter *names* are retained so the
/// resulting log line stays diagnosable.
static SAS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(sig|se|sp|sv|st|sr|ss|srt|spr|skoid|sktid|skt|ske|sks|skv)=([^&\s]+)")
        .expect("static SAS redaction regex is valid")
});

/// Placeholder substituted for any redacted secret.
const REDACTED: &str = "[REDACTED]";

/// Scrub known-sensitive Azure identifiers out of arbitrary text (typically
/// `azlin` / Azure CLI stderr) before it is folded into a `RemoteError` or a
/// log line.
///
/// This is a pure, allocation-only transform: it never performs I/O, has no
/// side effects, and is idempotent. It removes:
///
/// * SAS query-parameter values (`sig`, `se`, `sp`, ...), retaining the
///   parameter name so the error remains diagnosable.
/// * Canonical Azure GUIDs (subscription / tenant / resource IDs).
///
/// SAS scrubbing runs first so that its `[REDACTED]` placeholder cannot
/// interfere with subsequent GUID matching. All non-sensitive context (hosts,
/// paths, VM names, status text) is preserved so logs stay actionable.
///
/// This addresses issue #882 (CWE-532: sensitive data in logs). It is
/// intentionally scoped to GUIDs and SAS parameters; other secret classes
/// (connection strings, bearer/JWT tokens, account keys) are tracked as a P2
/// follow-up.
pub(crate) fn redact_sensitive(input: &str) -> String {
    let sas_scrubbed = SAS_RE.replace_all(input, "${1}=[REDACTED]");
    GUID_RE.replace_all(&sas_scrubbed, REDACTED).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_scrubs_subscription_guid() {
        let stderr = "ERROR: subscription 00000000-0000-0000-0000-000000000000 not found";
        let out = redact_sensitive(stderr);
        assert!(
            !out.contains("00000000-0000-0000-0000-000000000000"),
            "GUID must be redacted, got: {out}"
        );
        assert!(
            out.contains("[REDACTED]"),
            "expected placeholder, got: {out}"
        );
        // Surrounding, non-sensitive context is preserved so the log stays
        // actionable.
        assert!(out.contains("subscription"), "context lost: {out}");
        assert!(out.contains("not found"), "context lost: {out}");
    }

    #[test]
    fn redact_scrubs_sas_token_query_params() {
        let stderr = "failed to upload to \
             https://acct.blob.core.windows.net/c/b?sv=2021-08-06&ss=b&srt=o&\
             sp=rwdlac&se=2030-01-01T00:00:00Z&st=2020-01-01T00:00:00Z&spr=https&\
             sig=EXAMPLE-fake-sas-signature-do-not-use";
        let out = redact_sensitive(stderr);
        // The bearer-capability signature must never survive.
        assert!(
            !out.contains("EXAMPLE-fake-sas-signature-do-not-use"),
            "SAS sig value must be redacted, got: {out}"
        );
        // Other SAS parameter *values* are conservatively redacted too.
        assert!(!out.contains("2021-08-06"), "sv value leaked: {out}");
        assert!(!out.contains("rwdlac"), "sp value leaked: {out}");
        // Parameter names are retained so the error remains diagnosable.
        assert!(out.contains("sig="), "sig param name lost: {out}");
        assert!(
            out.contains("[REDACTED]"),
            "expected placeholder, got: {out}"
        );
        // Non-secret host/path context is preserved.
        assert!(
            out.contains("acct.blob.core.windows.net"),
            "host context lost: {out}"
        );
    }

    #[test]
    fn redact_preserves_benign_text() {
        let stderr = "ERROR: VM 'ci-runner-01' is in state 'Deallocated'; retry after 30s";
        let out = redact_sensitive(stderr);
        assert_eq!(
            out, stderr,
            "benign text must pass through unchanged, got: {out}"
        );
        assert!(
            !out.contains("[REDACTED]"),
            "false-positive redaction: {out}"
        );
    }

    #[test]
    fn redact_handles_multiple_secrets() {
        let stderr = "sub 00000000-0000-0000-0000-000000000000 tenant \
             11111111-1111-1111-1111-111111111111 url ?sig=EXAMPLE-fake-sig&se=2030-01-01";
        let out = redact_sensitive(stderr);
        assert!(
            !out.contains("00000000-0000-0000-0000-000000000000"),
            "sub GUID leaked: {out}"
        );
        assert!(
            !out.contains("11111111-1111-1111-1111-111111111111"),
            "tenant GUID leaked: {out}"
        );
        assert!(!out.contains("EXAMPLE-fake-sig"), "sig leaked: {out}");
        // At least three independent redactions occurred.
        assert!(
            out.matches("[REDACTED]").count() >= 3,
            "expected >=3 redactions, got: {out}"
        );
    }
}
