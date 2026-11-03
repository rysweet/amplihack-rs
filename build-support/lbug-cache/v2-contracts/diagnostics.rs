//! Safe diagnostic formatting is separate from Cargo's machine stdout.
use crate::support::KEY;
use lbug_cache::diagnostics::Diagnostic;

#[test]
fn corruption_diagnostic_visibly_warns_and_reports_source_fallback() {
    let line = Diagnostic::warning("checksum_mismatch", "verify", Some(KEY), "source").render();
    for required in ["warning", "checksum_mismatch", "verify", KEY, "source"] {
        assert!(
            line.contains(required),
            "missing diagnostic field: {required}"
        );
    }
    assert!(line.len() <= 2048);
}

#[test]
fn untrusted_key_is_omitted_instead_of_leaking_tokens_or_injecting_lines() {
    let forged = "bad-key\nAuthorization: Bearer SECRET_SENTINEL\u{1b}[2J";
    let line = Diagnostic::warning("invalid_identity", "prepare", Some(forged), "source").render();
    assert!(!line.contains("SECRET_SENTINEL"));
    assert!(!line.trim_end_matches('\n').chars().any(char::is_control));
    assert!(line.contains("invalid_identity"));
    assert!(line.contains("source"));
}
