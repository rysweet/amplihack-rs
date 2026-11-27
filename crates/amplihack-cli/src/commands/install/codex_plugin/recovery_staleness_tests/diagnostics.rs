//! Verification failures preserve diagnostics and all recovery state.
use super::*;

#[test]
fn failed_supersession_probe_preserves_cause_and_all_recovery_state() {
    if crate::test_support::isolated_install_test() {
        return;
    }

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
    if crate::test_support::isolated_install_test() {
        return;
    }

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

#[test]
fn current_attempt_foreign_edit_still_refuses_without_archival() {
    if crate::test_support::isolated_install_test() {
        return;
    }

    let f = Fixture::new();
    assert!(f.install().unwrap());
    let before = fs::read(f.root.join("ownership.json")).unwrap();
    f.upgrade(b"current-attempt");
    fs::write(f.home.join("edit-during-add"), b"yes").unwrap();
    let error = format!("{:#}", f.install().unwrap_err());
    assert!(error.contains("foreign Codex config changed"), "{error}");
    assert!(f.root.join("pending.json").is_file());
    assert!(f.archives().is_empty());
    assert!(
        fs::read(f.root.join("ownership.json")).unwrap() == before,
        "foreign bytes must be rejected before completed ownership publication"
    );
}

#[test]
fn nonregular_targets_are_errors_not_absence() {
    if crate::test_support::isolated_install_test() {
        return;
    }

    for kind in ["symlink", "directory"] {
        let f = Fixture::new();
        let bytes = f.orphan();
        let config = f.codex.join("config.toml");
        if kind == "symlink" {
            std::os::unix::fs::symlink(f.home.join("missing"), &config).unwrap();
        } else {
            fs::create_dir(&config).unwrap();
        }
        assert!(f.install().is_err(), "{kind}");
        assert_eq!(fs::read(f.root.join("pending.json")).unwrap(), bytes);
        assert!(f.archives().is_empty());
    }
}
