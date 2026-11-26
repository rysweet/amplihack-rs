//! Issue #2279 (C10, AC7): `amplihack doctor` names every settings file that
//! has lost amplihack's hooks, and the events it lacks, without printing file
//! content, and fails. It writes nothing.
//!
//! Runs the real `amplihack` binary through `isolated(..)` with a temp
//! `HOME`, a temp profile root holding two profiles (one stripped), a temp
//! `AMPLIHACK_HOME` with a launch `--settings` file and a stub hooks binary.
//! Nothing touches the real `~/.claude`, `~/.amplihack` or profile root.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const SECRET: &str = "sk-test-do-not-print";
const SUBCMDS: &[(&str, Option<&str>, &str, Option<u64>)] = &[
    ("SessionStart", None, "session-start", Some(10)),
    ("Stop", None, "stop", Some(120)),
    ("PreToolUse", Some("*"), "pre-tool-use", None),
    ("PostToolUse", Some("*"), "post-tool-use", None),
    (
        "UserPromptSubmit",
        None,
        "workflow-classification-reminder",
        Some(5),
    ),
    ("UserPromptSubmit", None, "user-prompt-submit", Some(10)),
    ("PreCompact", None, "pre-compact", Some(30)),
    ("SessionEnd", None, "session-stop-event", Some(30)),
];

fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// The block without StopFailure, built by hand: the test must not depend
/// on the crate's own builder. Doctor reads StopFailure as `missing` or
/// `extra` depending on the Claude it finds, so the assertions below name
/// only the other events.
fn hooks_for(bin: &Path) -> Value {
    let mut hooks = serde_json::Map::new();
    for (event, matcher, sub, timeout) in SUBCMDS {
        let mut hook =
            json!({"type": "command", "command": format!("\"{}\" {sub}", bin.display())});
        if let Some(t) = timeout {
            hook["timeout"] = json!(t);
        }
        let mut w = json!({"hooks": [hook]});
        if let Some(m) = matcher {
            w["matcher"] = json!(m);
        }
        hooks
            .entry(event.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(w);
    }
    Value::Object(hooks)
}

fn stamp(path: &Path) -> (Vec<u8>, i64, i64) {
    let m = fs::metadata(path).unwrap();
    (fs::read(path).unwrap(), m.mtime(), m.mtime_nsec())
}

#[test]
fn doctor_names_the_stripped_profile_and_its_events_and_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("profiles");
    let amplihack_home = home.join(".amplihack");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();

    let bin = home.join(".local/bin/amplihack-hooks");
    private(&bin, b"#!/bin/sh\nexit 0\n");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();

    let full = json!({"env": {"SENTINEL_SECRET": SECRET}, "hooks": hooks_for(&bin)});
    let stripped = json!({"env": {"SENTINEL_SECRET": SECRET}, "hooks": {}});

    let home_settings = home.join(".claude/settings.json");
    let alpha = root.join("alpha/settings.json");
    let work = root.join("work/settings.json");
    let launch = amplihack_home.join("runtime/launch-hooks-settings.json");
    private(&home_settings, full.to_string().as_bytes());
    private(&alpha, full.to_string().as_bytes());
    private(&work, stripped.to_string().as_bytes());
    private(
        &launch,
        json!({"hooks": hooks_for(&bin)}).to_string().as_bytes(),
    );
    let files: Vec<PathBuf> = vec![home_settings.clone(), alpha.clone(), work.clone(), launch];
    let before: Vec<_> = files.iter().map(|p| stamp(p)).collect();

    let out = crate::isolated_amplihack::isolated(amplihack_bin())
        .arg("doctor")
        .current_dir(&home)
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("AMPLIHACK_HOME", &amplihack_home)
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", &root)
        .env("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &bin)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let all = format!("{stdout}\n{stderr}");

    assert!(!out.status.success(), "doctor must fail on drift:\n{all}");
    let line = all
        .lines()
        .find(|l| l.contains(&work.display().to_string()))
        .unwrap_or_else(|| panic!("the stripped profile is not named:\n{all}"));
    assert!(line.contains("drift"), "{line}");
    for event in [
        "SessionStart",
        "PreToolUse",
        "PostToolUse",
        "UserPromptSubmit",
        "SessionEnd",
    ] {
        assert!(line.contains(event), "{event} not named: {line}");
    }
    assert!(
        all.lines()
            .any(|l| l.contains(&alpha.display().to_string())),
        "every profile gets a line:\n{all}"
    );
    assert!(!all.contains(SECRET), "file content printed:\n{all}");
    assert!(!line.contains('{'), "no JSON on the hook lines: {line}");

    let after: Vec<_> = files.iter().map(|p| stamp(p)).collect();
    assert_eq!(after, before, "doctor is read-only");
}
