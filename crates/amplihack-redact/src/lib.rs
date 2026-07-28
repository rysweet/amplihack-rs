//! Canonical relay secret-redaction, extracted into a dependency-free leaf
//! crate so it can be shared without a dependency cycle.
//!
//! [`redact_for_relay`] previously lived in
//! `amplihack-signal::chat::outbound`. Because `amplihack-turn` must redact the
//! bounded turn-failure error tail before embedding it into an `io::Error`
//! (issue #1108) and the crate dependency direction is `signal -> turn`, the
//! redactor is centralised here. `amplihack-signal` re-exports this function so
//! every existing caller keeps working, and `amplihack-turn` depends on this
//! leaf crate directly. The crate pulls in only `regex` — no `tokio`,
//! networking, or process dependencies leak into `amplihack-turn`.
//!
//! ## Guarantees
//!
//! * **Additive / monotonic coverage** (#1096/#1103) — every secret shape the
//!   prior redactor scrubbed is still scrubbed; new shapes (Azure DevOps PATs,
//!   short / unusual-charset keyed secrets) are added on top.
//! * **No over-redaction** — short / unusual-charset matching is gated on an
//!   explicit credential key plus quotes/scheme/assignment, never bare prose.
//! * **Pure, deterministic, idempotent** — `redact(redact(x)) == redact(x)`.
//! * **Panic-free & UTF-8-boundary-safe** on adversarial / multibyte input.
//! * **ReDoS-safe** — the `regex` crate matches in guaranteed linear time; the
//!   patterns are anchored/bounded with no catastrophic backtracking.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// Secret shapes scrubbed from a body before it is relayed, applied in order.
///
/// The broad `name = value` assignment is redacted first so its placeholder
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
        // value so the credential is fully redacted. The value is either a
        // fully quoted string of ANY length (widened for #1096 short /
        // unusual-charset secrets) or an unquoted token of >=6 chars (the
        // pre-existing, unchanged general behaviour). Because a credential key
        // and an assignment operator are BOTH required, benign prose such as
        // "password reset" or "secret sauce" is never matched.
        (
            r#"(?i)\b(api[_-]?key|access[_-]?key|secret|token|password|passwd|pwd|credential|authorization)\b['"]?\s*[:=]\s*(?:(?:bearer|basic|token)\s+)?(?:"[^"]{1,}"|'[^']{1,}'|['"]?[A-Za-z0-9._~+/=:-]{6,}['"]?)"#,
            "$1=[REDACTED]",
        ),
        // Azure DevOps Personal Access Tokens (#1103). AzDO PATs have no vendor
        // prefix, so they are matched by CONTEXT: an identifier that is an
        // `AZURE_DEVOPS*` var, ends in `PAT`, or is a bare `pat`, followed by an
        // assignment and a long (>=16-char) base32/base64 token body. This
        // catches the real credential shape without redacting arbitrary
        // base64-looking prose.
        (
            r#"(?i)\b(?:[a-z0-9]+_)*(?:azure[_-]?devops[a-z0-9_]*|[a-z0-9]*pat)\b['"]?\s*[:=]\s*['"]?[A-Za-z0-9+/=._~-]{16,}['"]?"#,
            "[REDACTED-AZDO-PAT]",
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

/// Scrub high-frequency secret shapes out of `body`.
///
/// Pure, deterministic, idempotent, and allocation-light (only adopts a new
/// buffer on a real match). Safe to call at any emit boundary.
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

#[cfg(test)]
mod tests {
    use super::redact_for_relay;

    const AZDO_PAT: &str = "abcd1234efgh5678ijkl9012mnop3456qrst7890uvwx1234yzAB";

    fn assert_scrubbed(input: &str, secret: &str) {
        let out = redact_for_relay(input);
        assert!(
            !out.contains(secret),
            "secret survived: in={input:?} out={out:?}"
        );
        assert!(out.contains("[REDACTED"), "no placeholder: {out:?}");
    }

    #[test]
    fn azure_devops_pat_env_assignment_is_redacted() {
        assert_scrubbed(&format!("AZURE_DEVOPS_EXT_PAT={AZDO_PAT}"), AZDO_PAT);
    }

    #[test]
    fn quoted_pat_is_redacted() {
        assert_scrubbed(&format!("pat = \"{AZDO_PAT}\""), AZDO_PAT);
    }

    #[test]
    fn short_keyed_quoted_secret_is_redacted() {
        assert_scrubbed("password=\"x9K2\"", "x9K2");
    }

    #[test]
    fn unusual_charset_quoted_secret_is_redacted() {
        let secret = "xK9+7mFq/2Lz=abCD3ef";
        assert_scrubbed(&format!("secret = \"{secret}\""), secret);
    }

    #[test]
    fn generic_authorization_value_is_redacted() {
        let secret = "AbCdEf0123456789GhIjKl";
        assert_scrubbed(&format!("authorization: {secret}"), secret);
    }

    #[test]
    fn benign_prose_is_preserved() {
        let prose = "The quick brown fox authorization was granted after review today.";
        assert_eq!(redact_for_relay(prose), prose);
        assert_eq!(
            redact_for_relay("password reset requested"),
            "password reset requested"
        );
        assert_eq!(
            redact_for_relay("secret sauce recipe"),
            "secret sauce recipe"
        );
    }

    #[test]
    fn legacy_shapes_still_redacted() {
        assert_scrubbed(
            "token ghp_0123456789abcdefghij0123",
            "ghp_0123456789abcdefghij0123",
        );
        assert_scrubbed("key AKIAIOSFODNN7EXAMPLE here", "AKIAIOSFODNN7EXAMPLE");
        assert_scrubbed("api_key = abcdef123456", "abcdef123456");
    }

    #[test]
    fn is_idempotent_and_deterministic() {
        let input = format!("AZURE_DEVOPS_EXT_PAT={AZDO_PAT}\nsecret=\"x9K2\"");
        let once = redact_for_relay(&input);
        assert_eq!(once, redact_for_relay(&once));
        assert!(!once.contains(AZDO_PAT));
    }

    #[test]
    fn multibyte_does_not_panic() {
        let input = format!("secret=\"café{AZDO_PAT}日本\"");
        let out = redact_for_relay(&input);
        assert!(!out.contains(AZDO_PAT));
    }

    #[test]
    fn empty_and_whitespace_safe() {
        assert_eq!(redact_for_relay(""), "");
        assert_eq!(redact_for_relay("   \n\t "), "   \n\t ");
    }
}
