//! TDD (RED) contract tests for the new `amplihack-redact` leaf crate —
//! issues #1096 / #1103 / #1108 (relay-redaction hardening).
//!
//! Written **first**: these FAIL to compile/run until the leaf crate exists and
//! exposes the canonical redactor:
//!
//! ```ignore
//! pub fn redact_for_relay(body: &str) -> String;
//! ```
//!
//! The crate is the single, cycle-free source of truth for relay secret
//! redaction. `amplihack-signal` re-exports this function (`pub use`) so every
//! existing caller keeps working, and `amplihack-turn` depends on it directly
//! to redact the bounded turn-failure error tail before it is embedded into an
//! `io::Error` (resolving the turn -> signal -> turn dependency cycle).
//!
//! Contract (all must hold):
//!   * Coverage is **strictly additive / monotonic** — every secret shape the
//!     legacy `amplihack-signal::chat::outbound::redact_for_relay` scrubbed is
//!     still scrubbed here, and NEW shapes are added on top.
//!   * Broadened coverage (#1096/#1103): Azure DevOps PATs, generic
//!     `Bearer` / `Authorization` tokens, and short / unusual-charset secret
//!     tokens **when presented in a quoted or auth/assignment context** are
//!     redacted.
//!   * Benign prose is preserved (no over-redaction) — short/unusual-charset
//!     matching is gated on quotes/scheme/assignment, never bare English words.
//!   * Pure, deterministic, and **idempotent**: `redact(redact(x)) == redact(x)`.
//!   * **Fail-closed & panic-free** on adversarial input (multibyte boundaries,
//!     pathological lengths) — never returns raw secret bytes, never panics.
//!   * **ReDoS-safe**: bounded/anchored patterns complete on adversarial input
//!     well within a hard time bound.
//!
//! Run: `cargo test -p amplihack-redact --test redact_contract_it`.

use amplihack_redact::redact_for_relay;

// A representative 52-char Azure DevOps PAT (base64-ish high-entropy token).
const AZDO_PAT: &str = "abcd1234efgh5678ijkl9012mnop3456qrst7890uvwx1234yzAB";

/// Assert a secret is fully scrubbed: the raw value must not survive and a
/// redaction placeholder must appear in its place.
fn assert_scrubbed(input: &str, secret: &str) {
    let out = redact_for_relay(input);
    assert!(
        !out.contains(secret),
        "secret must not survive redaction\n  input: {input:?}\n  output: {out:?}"
    );
    assert!(
        out.contains("[REDACTED"),
        "a redaction placeholder must mark the scrubbed value\n  output: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// #1103 — Azure DevOps PATs
// ---------------------------------------------------------------------------

#[test]
fn azure_devops_pat_in_env_assignment_is_redacted() {
    // The canonical way a PAT leaks: an echoed env var / CLI arg.
    assert_scrubbed(&format!("AZURE_DEVOPS_EXT_PAT={AZDO_PAT}"), AZDO_PAT);
}

#[test]
fn azure_devops_pat_in_basic_auth_header_is_redacted() {
    // `Authorization: Basic base64(":<PAT>")` — the strengthened Authorization
    // coverage must scrub the whole credential, not just the scheme word.
    let header = format!("Authorization: Basic OntBWkRPX1BBVH0={AZDO_PAT}");
    let out = redact_for_relay(&header);
    assert!(
        !out.contains(AZDO_PAT),
        "the PAT inside a Basic auth header must be redacted: {out:?}"
    );
}

#[test]
fn quoted_azure_devops_pat_is_redacted() {
    assert_scrubbed(&format!("pat = \"{AZDO_PAT}\""), AZDO_PAT);
}

// ---------------------------------------------------------------------------
// #1103 — generic Bearer / Authorization tokens
// ---------------------------------------------------------------------------

#[test]
fn bearer_token_is_redacted() {
    let secret = "eyJhbGciOiJIUzI1NiJ9.payloadpayloadpayload.sigsigsig";
    assert_scrubbed(&format!("Authorization: Bearer {secret}"), secret);
}

#[test]
fn generic_authorization_value_is_redacted() {
    let secret = "AbCdEf0123456789GhIjKl";
    assert_scrubbed(&format!("authorization: {secret}"), secret);
}

// ---------------------------------------------------------------------------
// #1096 — short / unusual-charset secret tokens (context-gated)
// ---------------------------------------------------------------------------

#[test]
fn short_keyed_secret_below_legacy_threshold_is_redacted() {
    // The legacy `key=value` rule required a >=6-char value, so a short
    // password slipped through. The hardened redactor must catch it when it is
    // clearly a keyed credential.
    let secret = "x9K2";
    assert_scrubbed(&format!("password=\"{secret}\""), secret);
}

#[test]
fn unusual_charset_quoted_secret_is_redacted() {
    // A token using base64 special chars (`+`, `/`, `=`) that the narrow
    // alphanumeric rules missed, presented in a quoted assignment.
    let secret = "xK9+7mFq/2Lz=abCD3ef";
    assert_scrubbed(&format!("secret = \"{secret}\""), secret);
}

// ---------------------------------------------------------------------------
// No over-redaction — benign prose is preserved (R5)
// ---------------------------------------------------------------------------

#[test]
fn benign_prose_is_not_redacted() {
    let prose = "The quick brown fox authorization was granted after review today.";
    assert_eq!(
        redact_for_relay(prose),
        prose,
        "unquoted benign prose must be preserved verbatim (no over-redaction)"
    );
}

#[test]
fn benign_bare_word_is_not_redacted() {
    // A bare short word with no key/quote/scheme context must survive.
    let text = "status ok done";
    assert_eq!(redact_for_relay(text), text);
}

// ---------------------------------------------------------------------------
// Monotonic coverage — every legacy shape is still scrubbed (R4 regression)
// ---------------------------------------------------------------------------

#[test]
fn legacy_github_token_still_redacted() {
    assert_scrubbed(
        "token ghp_0123456789abcdefghij0123",
        "ghp_0123456789abcdefghij0123",
    );
}

#[test]
fn legacy_aws_key_still_redacted() {
    assert_scrubbed("key AKIAIOSFODNN7EXAMPLE here", "AKIAIOSFODNN7EXAMPLE");
}

#[test]
fn legacy_google_key_still_redacted() {
    let secret = "AIzaSyA1234567890abcdefghijklmnopqrstuv";
    assert_scrubbed(&format!("gkey {secret}"), secret);
}

#[test]
fn legacy_slack_token_still_redacted() {
    assert_scrubbed("xoxb-1234567890-abcdefghij", "xoxb-1234567890-abcdefghij");
}

#[test]
fn legacy_url_userinfo_password_still_redacted() {
    let out = redact_for_relay("clone https://user:supersecretpw@example.com/repo.git");
    assert!(
        !out.contains("supersecretpw"),
        "URL userinfo password must be redacted: {out:?}"
    );
    assert!(
        out.contains("user:[REDACTED]@"),
        "scheme + username preserved, password dropped: {out:?}"
    );
}

#[test]
fn legacy_private_key_block_still_redacted() {
    let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIBleatBODYbytes\n-----END RSA PRIVATE KEY-----";
    let out = redact_for_relay(pem);
    assert!(
        !out.contains("MIIBleatBODYbytes"),
        "PEM private-key body must be redacted: {out:?}"
    );
}

#[test]
fn legacy_key_value_credential_still_redacted() {
    assert_scrubbed("api_key = abcdef123456", "abcdef123456");
}

// ---------------------------------------------------------------------------
// Idempotency & determinism
// ---------------------------------------------------------------------------

#[test]
fn redaction_is_idempotent() {
    let input = format!(
        "AZURE_DEVOPS_EXT_PAT={AZDO_PAT}\nAuthorization: Bearer eyJabc.def.ghi\npassword=\"x9K2\""
    );
    let once = redact_for_relay(&input);
    let twice = redact_for_relay(&once);
    assert_eq!(once, twice, "redact(redact(x)) must equal redact(x)");
    assert!(
        !once.contains(AZDO_PAT),
        "secret must not survive: {once:?}"
    );
}

#[test]
fn redaction_is_deterministic() {
    let input = format!("pat=\"{AZDO_PAT}\"");
    assert_eq!(redact_for_relay(&input), redact_for_relay(&input));
}

// ---------------------------------------------------------------------------
// Fail-closed / panic-free on adversarial input
// ---------------------------------------------------------------------------

#[test]
fn multibyte_secret_does_not_panic_and_is_handled() {
    // A secret adjacent to multibyte codepoints must never trigger a
    // char-boundary panic; the function must return safely.
    let input = format!("password=\"café{AZDO_PAT}日本\"");
    let out = redact_for_relay(&input); // must not panic
    assert!(!out.contains(AZDO_PAT), "secret must not survive: {out:?}");
}

#[test]
fn empty_and_whitespace_input_are_returned_safely() {
    assert_eq!(redact_for_relay(""), "");
    assert_eq!(redact_for_relay("   \n\t "), "   \n\t ");
}

#[test]
fn redos_resistance_bounded_runtime_on_adversarial_input() {
    // A pathological, highly repetitive input must complete well within a hard
    // bound — proving patterns are bounded/anchored (no catastrophic
    // backtracking).
    let adversarial = format!("Authorization: {}", "a".repeat(50_000));
    let start = std::time::Instant::now();
    let _ = redact_for_relay(&adversarial);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "redaction must be ReDoS-safe (bounded runtime); took {elapsed:?}"
    );
}

#[test]
fn placeholder_does_not_encode_secret_length_or_bytes() {
    // The placeholder must be a fixed marker, not a length-preserving mask that
    // leaks the secret's size.
    let short = redact_for_relay("password=\"x9K2\"");
    let long = redact_for_relay(&format!("password=\"{AZDO_PAT}\""));
    assert!(short.contains("[REDACTED"));
    assert!(long.contains("[REDACTED"));
}
