//! Unit tests for `validate.rs`: every field rule (R2.5, S7, S8, S8b), and
//! `entry`, the whole-entry check that `load` (C1) and `update` (C2) run.
//! A rule never repeats the rejected value; callers add the subject (`--dir`,
//! `NAME`, `discovered --model`) in front of the rule text.
//!
//! API under test (each `Err` holds the rule text only, without a subject):
//! - `agent_name(&str)`, `conversation_id(&str)`, `session_name(&str)`,
//!   `window_name(&str)`, `profile_name(&str)`, `env_value(&str)`,
//!   `launch_id(&str)` -> `Result<(), String>`
//! - `option_value(v: &str) -> Result<(), String>` (model, effort,
//!   permission mode: 1 to 256 bytes, no control character, no leading `-`)
//! - `socket_path(&Path) -> Result<(), String>` (absolute, UTF-8, no
//!   control character, at most 107 bytes)
//! - `launch_dir(&Path) -> Result<(), String>` (lexical rule plus "exists and
//!   is a directory"; `entry` applies only the lexical part)
//! - `add_dir(p: &Path, base: &Path) -> Result<PathBuf, String>` (relative
//!   paths joined to `base`; no control character)
//! - `FieldError { pub field: &'static str }`, `Display` = `field FIELD is not
//!   valid`; `entry(name: &str, e: &Entry) -> Result<(), FieldError>`
//!   (`field` is `name` for a bad agent name)

use super::registry::Entry;
use super::validate::*;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
const S7_RULE: &str = "must be 1 to 64 letters, digits or '-', starting with a letter or digit";
const PROFILE_RULE: &str =
    "names match [A-Za-z0-9][A-Za-z0-9._-]{0,63}, without '..' or a trailing '.'";

fn refused(r: Result<(), String>, value: &str) -> String {
    let e = r.expect_err(&format!("{value:?} should be refused"));
    if value.len() > 1 {
        assert!(!e.contains(value), "the rule repeated {value:?}: {e}");
    }
    e
}

#[test]
fn agent_names() {
    let max = format!("a{}", "b".repeat(63));
    for ok in ["a", "coordinator", "area-reports", "A1.b_c-d", "0x", &*max] {
        assert!(agent_name(ok).is_ok(), "{ok:?}");
    }
    let long = "a".repeat(65);
    for bad in [
        "", "-x", ".x", "_x", "a/b", "a b", "a:b", "a#b", "é", "a\n", &*long,
    ] {
        refused(agent_name(bad), bad);
    }
}

#[test]
fn conversation_ids_start_with_a_letter_or_digit() {
    let max = format!("a{}", "0".repeat(63));
    for ok in [ID, "abc", "0", &*max] {
        assert!(conversation_id(ok).is_ok(), "{ok:?}");
    }
    let (long, dsp) = ("a".repeat(65), "--dangerously-skip-permissions");
    for bad in [
        "", "-p", "--print", dsp, "-0f8e", "_abc", "a_b", "a.b", "a b", "a/b", "../x", "a\nb",
        &*long,
    ] {
        assert_eq!(refused(conversation_id(bad), bad), S7_RULE, "{bad:?}");
    }
}

#[test]
fn session_names_refuse_what_tmux_rewrites() {
    let (max, over) = ("s".repeat(64), "s".repeat(65));
    for ok in ["mistt", "my session", "a-b_c", &*max] {
        assert!(session_name(ok).is_ok(), "{ok:?}");
    }
    for bad in ["", "a:b", "a.b", "a#b", "a\tb", "a\u{1b}b", &*over] {
        refused(session_name(bad), bad);
    }
}

#[test]
fn window_names_allow_dots_and_colons_but_not_hash() {
    let (max, over) = ("w".repeat(64), "w".repeat(65));
    for ok in ["coordinator", "a.b", "a:b", "w 1", &*max] {
        assert!(window_name(ok).is_ok(), "{ok:?}");
    }
    for bad in ["", "w#1", "a\nb", "a\u{7f}b", &*over] {
        refused(window_name(bad), bad);
    }
}

#[test]
fn socket_paths() {
    assert!(socket_path(Path::new("/tmp/tmux-1000/default")).is_ok());
    let exactly = format!("/{}", "s".repeat(106));
    assert!(
        socket_path(Path::new(&exactly)).is_ok(),
        "107 bytes is the limit"
    );
    let over = format!("/{}", "s".repeat(107));
    for bad in [&*over, "tmux-1000/default", "/tmp/a\nb", ""] {
        assert!(socket_path(Path::new(bad)).is_err(), "{bad:?}");
    }
    assert!(socket_path(Path::new(OsStr::from_bytes(b"/tmp/\xff"))).is_err());
}

#[test]
fn launch_dirs_must_be_absolute_existing_directories_without_hash() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(launch_dir(tmp.path()).is_ok());
    let hash = tmp.path().join("a#b");
    std::fs::create_dir(&hash).unwrap();
    assert!(launch_dir(&hash).is_err(), "tmux expands '#' in -c (D11)");
    let ctl = tmp.path().join("a\u{1b}b");
    std::fs::create_dir(&ctl).unwrap();
    assert!(launch_dir(&ctl).is_err());
    let file = tmp.path().join("f");
    std::fs::write(&file, "").unwrap();
    assert!(launch_dir(&file).is_err(), "not a directory");
    assert!(launch_dir(&tmp.path().join("missing")).is_err());
    assert!(launch_dir(Path::new("relative")).is_err());
    let not_utf8 = tmp.path().join(OsStr::from_bytes(b"x\xff"));
    std::fs::create_dir(&not_utf8).unwrap();
    assert!(launch_dir(&not_utf8).is_err());
}

#[test]
fn add_dirs_are_made_absolute_against_the_launch_directory() {
    let base = Path::new("/srv/project");
    assert_eq!(
        add_dir(Path::new("/srv/docs"), base).unwrap(),
        PathBuf::from("/srv/docs")
    );
    assert_eq!(
        add_dir(Path::new("docs"), base).unwrap(),
        PathBuf::from("/srv/project/docs")
    );
    let e = add_dir(Path::new("/srv/\u{7}secret"), base).unwrap_err();
    assert!(!e.contains("secret"), "{e}");
}

#[test]
fn option_values() {
    assert!(option_value("claude-opus-5-5").is_ok());
    assert!(option_value(&"x".repeat(256)).is_ok());
    let over = "x".repeat(257);
    for bad in ["", "-x", "--print", "a\u{0}b", "a\rb", &*over] {
        refused(option_value(bad), bad);
    }
}

#[test]
fn profile_names_follow_the_profile_command() {
    for ok in ["work", "w2", "a.b_c-d", "Z9"] {
        assert!(profile_name(ok).is_ok(), "{ok:?}");
    }
    let over = "p".repeat(65);
    for bad in [
        "", "../x", "a/b", ".x", "x.", "a..b", "-x", "CON", "lpt1", &*over,
    ] {
        assert_eq!(refused(profile_name(bad), bad), PROFILE_RULE, "{bad:?}");
    }
}

#[test]
fn env_values_and_launch_ids() {
    assert!(env_value("high").is_ok());
    assert!(env_value(&"v".repeat(4096)).is_ok());
    let over = "v".repeat(4097);
    for bad in ["a\nb", "a\0b", &*over] {
        assert_eq!(
            env_value(bad).unwrap_err(),
            "value must be at most 4096 bytes, with no control characters"
        );
    }
    assert!(launch_id("6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d").is_ok());
    let (g, upper) = (
        "g".repeat(32),
        "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d".to_uppercase(),
    );
    for bad in ["", &*upper, "6f1c0d8e", &*g] {
        assert!(launch_id(bad).is_err(), "{bad:?}");
    }
}

/// A valid entry as stored on disk. `launch_dir` need not exist: `entry` is
/// lexical (the existence check belongs to `register` and the wrapper).
fn good() -> Value {
    json!({
        "state": "active", "tmux_session": "coord", "tmux_window": "main",
        "tmux_socket": "/tmp/tmux-1000/default", "launch_dir": "/nonexistent/project",
        "conversation_id": ID, "claude_pid": 41237, "proc_start": 88123456,
        "model": "opus", "effort": "high", "add_dirs": ["/srv/docs"],
        "skip_permissions": true,
        "amplihack_flags": { "subprocess_safe": true, "no_reflection": false, "profile": "work" },
        "env": { "CLAUDE_CODE_EFFORT_LEVEL": "high", "AMPLIHACK_AGENT_BINARY": "claude" },
        "registered_at": "2026-10-30T09:12:04Z", "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d", "lost_at": "2026-10-30T10:01:09Z",
        "restarts": ["2026-10-30T09:40:00Z"],
        "last_recovery": { "logged_at": "2026-10-30T09:40:00Z", "conversation_id": ID,
            "lost": "2026-10-30T09:39:00Z", "resumed": "2026-10-30T09:40:00Z",
            "result": "resumed" },
        "alert": "free text \u{7} is not checked"
    })
}

fn check(v: Value) -> Result<(), FieldError> {
    let e: Entry = serde_json::from_value(v).expect("well-typed entry");
    entry("coordinator", &e)
}

#[test]
fn a_valid_entry_passes_and_a_bad_name_is_reported_without_quoting_it() {
    check(good()).expect("valid entry");
    let e: Entry = serde_json::from_value(good()).unwrap();
    let err = entry("bad name!", &e).unwrap_err();
    assert_eq!(err.field, "name");
    assert!(!err.to_string().contains("bad name!"), "{err}");
}

/// One case per row of data model section 4.1: (JSON pointer, bad value).
/// The reported field is the pointer with `/` as `.`; for `add_dirs`,
/// `restarts` and `env` it is the field without the element.
#[test]
fn every_field_rule_is_applied_and_named_without_the_value() {
    let cases: &[(&str, Value)] = &[
        ("/tmux_session", json!("a.b")),
        ("/tmux_window", json!("w#1")),
        ("/tmux_socket", json!("relative/sock")),
        ("/launch_dir", json!("/srv/a#b")),
        ("/conversation_id", json!("--print")),
        ("/last_recovery/conversation_id", json!("-p")),
        ("/model", json!("-evil")),
        ("/effort", json!("hi\u{7}gh")),
        ("/permission_mode", json!("")),
        ("/amplihack_flags/profile", json!("../x")),
        ("/add_dirs/0", json!("relative/dir")),
        ("/env/AMPLIHACK_AGENT_BINARY", json!("/bin/sh")),
        ("/env/CLAUDE_CODE_EFFORT_LEVEL", json!("a\nb")),
        ("/registered_at", json!("yesterday")),
        ("/launched_at", json!("2026-10-30 09:12:04")),
        ("/lost_at", json!("soon")),
        ("/restarts/0", json!("2026-13-01T00:00:00Z")),
        ("/last_recovery/logged_at", json!("x")),
        ("/last_recovery/lost", json!("x")),
        ("/last_recovery/resumed", json!("x")),
        ("/launch_id", json!("NOT-HEX")),
    ];
    for (ptr, bad) in cases {
        let mut v = good();
        if *ptr == "/permission_mode" {
            v["permission_mode"] = bad.clone();
            v["skip_permissions"] = json!(false);
        } else {
            *v.pointer_mut(ptr).unwrap_or_else(|| panic!("{ptr}")) = bad.clone();
        }
        let path = ptr[1..].replace('/', ".");
        let field = match path.split('.').next().unwrap() {
            top @ ("add_dirs" | "restarts" | "env") => top.to_string(),
            _ => path.clone(),
        };
        let err = check(v).expect_err(ptr);
        assert_eq!(err.field, field, "{ptr}");
        assert_eq!(err.to_string(), format!("field {field} is not valid"));
    }
}

#[test]
fn env_keys_must_be_on_the_allowlist() {
    for key in [
        "ANTHROPIC_API_KEY",
        "PATH",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE",
    ] {
        let mut v = good();
        v["env"] = json!({ key: "x" });
        let err = check(v).expect_err(key);
        assert_eq!(err.field, "env");
        assert!(!err.to_string().contains(key));
    }
}

#[test]
fn alert_result_and_process_pair_are_not_checked_beyond_their_types() {
    let mut v = good();
    v["last_recovery"]["result"] = json!("failed: tmux: \u{1b}[31m odd");
    v.as_object_mut().unwrap().remove("proc_start");
    check(v).expect("alert, result and a lone pid are accepted (C1, I3)");
}
