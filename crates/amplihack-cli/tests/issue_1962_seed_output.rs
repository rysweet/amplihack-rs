//! Issue #1962, S6: account values reach `seed-record`'s stdout and nowhere else, a
//! server value with an escape sequence reaches no terminal, and no message
//! recommends `profile verify --fix`, which stays barred until #1960.

#![cfg(unix)]

use crate::support::profile_1962::{Answer, World, account_in, printed};
use crate::support::profile_rotate::text;
use std::fs;
use std::path::Path;

const ESC: char = '\u{1b}';

fn assert_no_account_in_state(w: &World) {
    let log = w.f.log();
    for secret in ["work@example.com", "org-work", "fake-task1962"] {
        assert!(!log.contains(secret), "the log holds {secret}: {log}");
    }
    let ledger = fs::read_to_string(w.ledger_path()).unwrap_or_default();
    assert!(
        !ledger.contains("fake-task1962"),
        "the ledger holds a token: {ledger}"
    );
}

#[test]
fn write_prints_the_account_to_stdout_only() {
    let w = World::new();
    w.no_record("work");
    let out = w.seed("work", true);
    assert!(out.status.success(), "{}", printed(&out));
    assert!(text(&out.stdout).contains("work@example.com"));
    let stderr = text(&out.stderr);
    assert!(!stderr.contains("work@example.com"), "{stderr}");
    assert!(!stderr.contains("org-work"), "{stderr}");
    assert_no_account_in_state(&w);
    assert!(
        w.f.log()
            .contains("seed-record: recorded account for profile work")
    );
}

/// Review fix: once the record is linked it is reported as recorded, even when
/// the audit line cannot be written.
#[test]
fn a_written_record_is_reported_when_the_audit_log_fails() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new();
    w.no_record("work");
    // A rotate directory that is not private: `ensure` refuses it.
    fs::set_permissions(w.f.rotate_dir(), fs::Permissions::from_mode(0o750)).unwrap();
    let out = w.seed("work", true);
    fs::set_permissions(w.f.rotate_dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(out.status.success(), "{}", printed(&out));
    assert!(w.record("work").is_some(), "the record was written");
    assert!(
        text(&out.stderr)
            .contains("seed-record work: recorded, but the audit log could not be written"),
        "{}",
        printed(&out)
    );
}

#[test]
fn a_cycle_seed_keeps_the_account_out_of_the_log() {
    let w = World::new();
    w.no_record("work");
    let _ = w.cycle();
    assert!(
        w.f.log()
            .contains("seeded: recorded account for live profile work")
    );
    assert_no_account_in_state(&w);
}

/// The value may be read as missing or printed as `<unprintable>`. Either way it
/// must not reach the terminal, and it must not be recorded.
#[test]
fn escape_sequences_reach_no_terminal_and_no_record() {
    for body in [
        account_in(
            "uuid-work",
            &format!("{ESC}[31mevil@example.com"),
            "org-work",
        ),
        account_in("uuid-work", "work@example.com", &format!("org{ESC}]0;evil")),
        account_in(&format!("uuid{ESC}[2J"), "work@example.com", "org-work"),
    ] {
        let w = World::new();
        w.no_record("work");
        w.fake.answer("work", Answer::Account(body));
        for write in [false, true] {
            let out = w.seed("work", write);
            assert!(
                !printed(&out).contains(ESC),
                "escape printed: {:?}",
                printed(&out)
            );
            if write {
                assert!(!out.status.success(), "an unsafe account is never recorded");
            }
            assert_eq!(w.record("work"), None);
        }
        assert!(!w.f.log().contains(ESC));
    }
}

/// Security review: a bidirectional override can make the account shown differ
/// from the one recorded, so it is refused like an escape.
#[test]
fn bidi_overrides_reach_no_terminal_and_no_record() {
    const RLO: char = '\u{202e}';
    let w = World::new();
    w.no_record("work");
    let email = format!("work{RLO}moc.elpmaxe@");
    w.fake.answer(
        "work",
        Answer::Account(account_in("uuid-work", &email, "org-work")),
    );
    for write in [false, true] {
        let out = w.seed("work", write);
        assert!(!printed(&out).contains(RLO), "{:?}", printed(&out));
        if write {
            assert!(!out.status.success(), "an unsafe account is never recorded");
        }
        assert_eq!(w.record("work"), None);
    }
    assert!(!w.f.log().contains(RLO));
}

/// Server text never becomes a cause: only the fixed status class reaches the log.
#[test]
fn a_server_error_body_never_reaches_the_cause() {
    let w = World::new();
    w.no_record("spare");
    w.fake.answer("spare", Answer::Status(418));
    let out = w.seed("spare", true);
    assert!(
        printed(&out).contains("identity read failed: http 4xx"),
        "{}",
        printed(&out)
    );
    assert!(!printed(&out).contains("fake"), "{}", printed(&out));
}

/// No message or how-to may send anyone to `--fix`. The reference may still
/// describe the flag under its "Do not run" warning.
#[test]
fn nothing_recommends_verify_fix() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let profile = manifest.join("src/commands/profile");
    let mut files = vec![manifest.join("../../docs/howto/claude-profile-rotation.md")];
    files.push(profile.join("seed_cmd.rs"));
    for entry in fs::read_dir(profile.join("rotate")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".rs") && !name.starts_with("tests") {
            files.push(path);
        }
    }
    for path in files {
        let body = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        for (i, line) in body.lines().enumerate() {
            assert!(
                !line.contains("verify --fix")
                    || line.contains("Do not")
                    || line.contains("not go through"),
                "{}:{}: {line}",
                path.display(),
                i + 1
            );
        }
    }
}
