//! Refusal tests for the review hardening of `validate.rs` and `report.rs`:
//! V4 (`--add-dir` refuses `#`), L1 (Unicode direction and zero-width
//! characters are refused like control characters and shown as `?`), L2
//! (an environment value refuses every control character) and D2 (no
//! recorded environment value appears in either `status` form).

use super::registry::Registry;
use super::report::{sanitize, status_json, status_text};
use super::validate::*;
use serde_json::json;
use std::path::Path;

/// One of each kind [`unsafe_char`] adds to the control characters.
const FORMAT: [char; 7] = [
    '\u{200B}', '\u{200F}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}', '\u{FEFF}',
];

#[test]
fn add_dirs_refuse_a_hash() {
    let base = Path::new("/srv/project");
    let e = add_dir(Path::new("/srv/a#b"), base).unwrap_err();
    assert_eq!(e, "must be a UTF-8 path without '#' or control characters");
    assert!(add_dir(Path::new("docs#1"), base).is_err(), "relative too");
}

#[test]
fn unsafe_chars_are_control_and_format_characters_only() {
    for c in FORMAT
        .into_iter()
        .chain(['\0', '\n', '\r', '\u{1b}', '\u{7f}'])
    {
        assert!(unsafe_char(c), "{:?}", c as u32);
    }
    for c in ['a', ' ', '#', 'é', '\u{2010}', '\u{2070}', '\u{FEFE}'] {
        assert!(!unsafe_char(c), "{:?}", c as u32);
    }
}

#[test]
fn format_characters_are_refused_in_names_options_and_paths() {
    for c in FORMAT {
        let s = format!("coord{c}x");
        assert!(session_name(&s).is_err(), "session {:?}", c as u32);
        assert!(window_name(&s).is_err(), "window {:?}", c as u32);
        assert!(option_value(&s).is_err(), "option {:?}", c as u32);
        let p = format!("/srv/{s}");
        assert!(socket_path(Path::new(&p)).is_err(), "socket {:?}", c as u32);
        assert!(launch_dir(Path::new(&p)).is_err(), "dir {:?}", c as u32);
        let base = Path::new("/srv");
        assert!(add_dir(Path::new(&p), base).is_err(), "add {:?}", c as u32);
    }
}

#[test]
fn sanitize_replaces_format_characters() {
    assert_eq!(sanitize("a\u{202E}b\u{200B}c\u{FEFF}", 200), "a?b?c?");
    assert_eq!(sanitize("café", 200), "café");
}

#[test]
fn env_values_refuse_every_control_character() {
    let rule = "value must be at most 4096 bytes, with no control characters";
    for bad in ["a\rb", "a\u{1b}[31mb", "a\tb", "a\u{7f}b", "a\nb", "a\0b"] {
        assert_eq!(env_value(bad).unwrap_err(), rule, "{bad:?}");
    }
    assert!(env_value("claude-opus-5-5 high").is_ok());
}

/// D2: recorded environment values are never printed by `status`.
#[test]
fn status_never_prints_a_recorded_environment_value() {
    let secret = "env-value-that-must-not-be-shown";
    let reg: Registry = serde_json::from_value(json!({ "version": 1, "agents": { "coord": {
        "state": "active", "tmux_session": "coord", "tmux_window": "main",
        "launch_dir": "/srv/project",
        "registered_at": "2026-10-30T08:00:00Z", "launched_at": "2026-10-30T08:00:00Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d",
        "env": { "ANTHROPIC_MODEL": secret, "CLAUDE_CODE_EFFORT_LEVEL": secret }
    } } }))
    .unwrap();
    let json = serde_json::to_string(&status_json(&reg, 1_793_361_600)).unwrap();
    for out in [json.as_str(), status_text(&reg).as_str()] {
        assert!(!out.contains(secret), "{out}");
        assert!(!out.contains("ANTHROPIC_MODEL"), "{out}");
    }
}
