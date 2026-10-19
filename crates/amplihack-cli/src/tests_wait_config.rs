//! `wait_config` (issue #1618): usage-limit wait settings from the
//! environment, then `$HOME/.amplihack/config`, then the defaults, field by
//! field, and the file's trust checks. `write` is in
//! `tests_wait_config_write.rs`.
//!
//! Pure: `resolve_from` takes the two variables' raw values and the result of
//! `read_file`, so no test here reads or changes the process environment.
//! Wired from `lib.rs` with `#[cfg(all(test, unix))] mod tests_wait_config;`.

use crate::wait_config::{FileRead, WaitSettings, Warn, read_file, resolve_from};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const ENABLED_VAR: &str = "AMPLIHACK_USAGE_LIMIT_WAIT";
const MAX_VAR: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";

fn defaults() -> WaitSettings {
    WaitSettings {
        enabled: true,
        max_wait_secs: None,
    }
}

fn file(json: &str) -> FileRead {
    FileRead::Content(json.as_bytes().to_vec())
}

fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".amplihack")).unwrap();
    (tmp, home)
}

fn config(home: &Path) -> PathBuf {
    home.join(".amplihack").join("config")
}

fn put(home: &Path, body: &str, mode: u32) {
    let p = config(home);
    fs::write(&p, body).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
}

// --- resolve_from --------------------------------------------------------

#[test]
fn defaults_are_enabled_with_no_maximum() {
    assert_eq!(WaitSettings::default(), defaults());
    assert_eq!(
        resolve_from(None, None, FileRead::Missing),
        (defaults(), vec![])
    );
}

#[test]
fn enabled_variable_accepts_eight_spellings_in_any_case() {
    for (raw, want) in [
        ("1", true),
        ("true", true),
        ("on", true),
        ("yes", true),
        ("TRUE", true),
        ("On", true),
        ("yEs", true),
        ("0", false),
        ("false", false),
        ("off", false),
        ("no", false),
        ("FALSE", false),
        ("OFF", false),
        ("No", false),
    ] {
        // The file says the opposite, so only the variable can give `want`.
        let f = format!(r#"{{"usage_limit_wait":{{"enabled":{}}}}}"#, !want);
        let (s, warns) = resolve_from(Some(raw), None, file(&f));
        assert_eq!(s.enabled, want, "{raw:?}");
        assert!(warns.is_empty(), "{raw:?}: {warns:?}");
    }
}

#[test]
fn an_empty_variable_is_unset_and_does_not_warn() {
    let f = r#"{"usage_limit_wait":{"enabled":false,"max_wait_secs":90}}"#;
    let (s, warns) = resolve_from(Some(""), Some(""), file(f));
    assert_eq!(
        s,
        WaitSettings {
            enabled: false,
            max_wait_secs: Some(90)
        }
    );
    assert!(warns.is_empty(), "{warns:?}");
}

#[test]
fn an_invalid_enabled_variable_warns_once_and_the_file_decides() {
    let f = r#"{"usage_limit_wait":{"enabled":false}}"#;
    for raw in ["maybe", "2", "y", "enable", " 1", "1 ", "tru"] {
        let (s, warns) = resolve_from(Some(raw), None, file(f));
        assert!(!s.enabled, "{raw:?}: the file decides");
        assert_eq!(warns, vec![Warn::BadEnabledVar], "{raw:?}");
    }
}

#[test]
fn max_variable_accepts_only_unsigned_ascii_digits() {
    for (raw, want) in [("0", 0u64), ("60", 60), ("3600", 3600), ("007", 7)] {
        let (s, warns) = resolve_from(None, Some(raw), FileRead::Missing);
        assert_eq!(s.max_wait_secs, Some(want), "{raw:?}");
        assert!(warns.is_empty(), "{raw:?}");
    }
    let f = r#"{"usage_limit_wait":{"max_wait_secs":120}}"#;
    for raw in [
        "+60",
        "-1",
        " 60",
        "60 ",
        "6O",
        "1.5",
        "1e3",
        "0x10",
        "٦٠",
        "99999999999999999999999",
    ] {
        let (s, warns) = resolve_from(None, Some(raw), file(f));
        assert_eq!(s.max_wait_secs, Some(120), "{raw:?}: the file decides");
        assert_eq!(warns, vec![Warn::BadMaxVar], "{raw:?}");
    }
}

#[test]
fn each_field_is_resolved_separately() {
    let f = r#"{"usage_limit_wait":{"enabled":true,"max_wait_secs":3600}}"#;
    let (s, _) = resolve_from(Some("off"), None, file(f));
    assert_eq!(
        s,
        WaitSettings {
            enabled: false,
            max_wait_secs: Some(3600)
        }
    );
    let (s, _) = resolve_from(None, Some("10"), file(f));
    assert_eq!(
        s,
        WaitSettings {
            enabled: true,
            max_wait_secs: Some(10)
        }
    );
}

#[test]
fn the_file_key_is_read_when_no_variable_is_set() {
    let f =
        r#"{"node_options_consent":true,"usage_limit_wait":{"enabled":false,"max_wait_secs":120}}"#;
    assert_eq!(
        resolve_from(None, None, file(f)),
        (
            WaitSettings {
                enabled: false,
                max_wait_secs: Some(120)
            },
            vec![]
        )
    );
}

#[test]
fn a_missing_key_or_missing_fields_give_defaults_silently() {
    for f in [
        "{}",
        r#"{"node_options_consent":true}"#,
        r#"{"usage_limit_wait":{}}"#,
    ] {
        assert_eq!(
            resolve_from(None, None, file(f)),
            (defaults(), vec![]),
            "{f}"
        );
    }
    let (s, w) = resolve_from(
        None,
        None,
        file(r#"{"usage_limit_wait":{"max_wait_secs":5}}"#),
    );
    assert_eq!((s.enabled, s.max_wait_secs, w), (true, Some(5), vec![]));
}

#[test]
fn a_bad_file_key_gives_defaults_for_the_whole_key_with_one_warning() {
    for f in [
        "not json",
        "[1,2,3]",
        r#"{"usage_limit_wait":5}"#,
        r#"{"usage_limit_wait":"on"}"#,
        r#"{"usage_limit_wait":{"enabled":"yes","max_wait_secs":10}}"#,
        r#"{"usage_limit_wait":{"enabled":false,"max_wait_secs":-5}}"#,
        r#"{"usage_limit_wait":{"enabled":false,"max_wait_secs":1.5}}"#,
        r#"{"usage_limit_wait":{"enabled":false,"max_wait_secs":"60"}}"#,
    ] {
        assert_eq!(
            resolve_from(None, None, file(f)),
            (defaults(), vec![Warn::BadKey]),
            "{f}"
        );
    }
}

#[test]
fn an_untrusted_file_gives_defaults_with_one_warning_and_variables_still_apply() {
    assert_eq!(
        resolve_from(None, None, FileRead::Untrusted),
        (defaults(), vec![Warn::UntrustedFile])
    );
    let (s, w) = resolve_from(Some("0"), Some("30"), FileRead::Untrusted);
    assert_eq!(
        s,
        WaitSettings {
            enabled: false,
            max_wait_secs: Some(30)
        }
    );
    assert_eq!(w, vec![Warn::UntrustedFile]);
}

#[test]
fn warnings_name_the_variable_or_key_and_never_the_value() {
    let (_, w) = resolve_from(Some("maybe-SECRET"), Some("+60-SECRET"), FileRead::Missing);
    assert_eq!(w, vec![Warn::BadEnabledVar, Warn::BadMaxVar]);
    let enabled = w[0].message();
    let max = w[1].message();
    assert!(
        enabled.contains(ENABLED_VAR) && !enabled.contains(MAX_VAR),
        "{enabled}"
    );
    assert!(max.contains(MAX_VAR), "{max}");
    for m in [&enabled, &max] {
        assert!(!m.contains("SECRET") && !m.contains("maybe"), "{m}");
    }
    let key = Warn::BadKey.message();
    assert!(key.contains("usage_limit_wait"), "{key}");
    let untrusted = Warn::UntrustedFile.message();
    assert!(
        untrusted.contains("usage_limit_wait") || untrusted.contains("config"),
        "{untrusted}"
    );
    for m in [key, untrusted, enabled, max] {
        assert!(!m.contains('\n') && !m.contains('\r'), "one line: {m:?}");
    }
}

// --- read_file -----------------------------------------------------------

#[test]
fn read_file_reports_a_missing_file_as_missing() {
    let (_t, home) = home();
    assert!(matches!(read_file(&home), FileRead::Missing));
    assert!(matches!(
        read_file(Path::new("/nonexistent-1618")),
        FileRead::Missing
    ));
}

#[test]
fn read_file_returns_a_private_file_content() {
    let (_t, home) = home();
    put(&home, r#"{"usage_limit_wait":{"enabled":false}}"#, 0o600);
    match read_file(&home) {
        FileRead::Content(b) => assert_eq!(b, br#"{"usage_limit_wait":{"enabled":false}}"#),
        _ => panic!("a private regular file must be read"),
    }
    put(&home, "{}", 0o644);
    assert!(
        matches!(read_file(&home), FileRead::Content(_)),
        "0644 is fine"
    );
}

#[test]
fn read_file_refuses_a_file_others_can_write() {
    let (_t, home) = home();
    for m in [0o666, 0o602] {
        put(&home, r#"{"usage_limit_wait":{"enabled":false}}"#, m);
        assert!(
            matches!(read_file(&home), FileRead::Untrusted),
            "mode {m:o}"
        );
    }
}

#[test]
fn read_file_refuses_a_symlink() {
    let (tmp, home) = home();
    let target = tmp.path().join("elsewhere.json");
    fs::write(&target, r#"{"usage_limit_wait":{"enabled":false}}"#).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, config(&home)).unwrap();
    assert!(matches!(read_file(&home), FileRead::Untrusted));
}

#[test]
fn read_file_refuses_a_directory_or_a_file_over_64_kib() {
    let (_t, home) = home();
    fs::create_dir(config(&home)).unwrap();
    assert!(matches!(read_file(&home), FileRead::Untrusted));
    fs::remove_dir(config(&home)).unwrap();
    let big = format!(r#"{{"pad":"{}"}}"#, "x".repeat(64 * 1024));
    put(&home, &big, 0o600);
    assert!(matches!(read_file(&home), FileRead::Untrusted));
    let fits = format!(r#"{{"pad":"{}"}}"#, "x".repeat(64 * 1024 - 20));
    put(&home, &fits, 0o600);
    assert!(
        matches!(read_file(&home), FileRead::Content(_)),
        "<= 64 KiB is read"
    );
}
