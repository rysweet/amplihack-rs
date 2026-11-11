//! Verification failures preserve diagnostics and all recovery state.
use super::*;

#[test]
fn failed_supersession_probe_preserves_cause_and_all_recovery_state() {
    let f = Fixture::new();
    let bytes = f.superseded();
    fs::rename(&f.binary, f.home.join("native-original")).unwrap();
    // The initial inventory succeeds; verification of the later completion
    // fails on the second inventory probe at the locked recovery boundary.
    crate::test_support::write_executable(
        &f.binary,
        "#!/bin/sh\nif [ -f \"$HOME/initial-probe\" ]; then exit 42; fi\n\
         : > \"$HOME/initial-probe\"\nexec \"$HOME/native-original\" \"$@\"\n",
    );
    let error = f.assert_retained(&bytes);
    assert!(
        error.contains("cannot verify Codex journal supersession"),
        "{error}"
    );
    assert!(
        error.contains("Codex native plugin command failed"),
        "{error}"
    );
    assert!(error.contains("42"), "{error}");
}

#[test]
fn invalid_supersession_ownership_preserves_deserialization_cause() {
    let f = Fixture::new();
    let bytes = f.superseded();
    let mut ledger = f.ledger();
    ledger["package_digest"] = json!(42);
    fs::write(f.root.join("ownership.json"), json_bytes(&ledger).unwrap()).unwrap();
    let error = f.assert_retained(&bytes);
    assert!(
        error.contains("cannot verify Codex journal supersession"),
        "{error}"
    );
    assert!(
        error.contains("invalid type: integer `42`, expected a string"),
        "{error}"
    );
}
