//! Unit tests for `agent_marker.rs`: the SessionEnd marker that tells the
//! `amplihack agent` supervisor a registered session ended cleanly (#1760,
//! R6.3, A6, A7, S9, S20b, step 5b section 6).
//!
//! Every test passes its own temp home; the environment is never changed.
//!
//! API under test (the reasons and the id rule are tested in
//! `amplihack_utils::agent_marker`):
//! - `record_clean_end(home: Option<&Path>, session_id: Option<&str>,
//!   extra: &serde_json::Value, now: u64)`: writes
//!   `home/.amplihack/agents/ended/<session_id>` (0600, directory 0700) as
//!   `{"session_id", "reason", "at"}`, or nothing; never panics or fails

use super::*;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ID: &str = "3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57";
const NOW: u64 = 1_793_322_100;

/// A temp home with private `~/.amplihack/agents/registry.json`.
fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let agents = home.join(".amplihack/agents");
    fs::create_dir_all(&agents).unwrap();
    for d in [&home.join(".amplihack"), &agents] {
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let reg = agents.join("registry.json");
    fs::write(&reg, "{\"version\":1,\"agents\":{}}\n").unwrap();
    fs::set_permissions(&reg, fs::Permissions::from_mode(0o600)).unwrap();
    (tmp, home)
}

fn ended(home: &Path) -> PathBuf {
    home.join(".amplihack/agents/ended")
}

fn reason(r: &str) -> Value {
    json!({ "reason": r })
}

/// Names in `ended/`, or none when it does not exist.
fn markers(home: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(ended(home)) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_clean_exit_writes_a_private_marker() {
    for r in ["prompt_input_exit", "logout"] {
        let (_t, home) = home();
        record_clean_end(Some(&home), Some(ID), &reason(r), NOW);
        assert_eq!(markers(&home), [ID], "{r}");
        let file = ended(&home).join(ID);
        let body: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(body, json!({ "session_id": ID, "reason": r, "at": NOW }));
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&ended(&home)), 0o700);
    }
}

#[test]
fn other_reasons_write_nothing() {
    for extra in [
        reason("clear"),
        reason("resume"),
        reason("other"),
        reason("exit"),
        reason("PROMPT_INPUT_EXIT"),
        reason(""),
        json!({}),
        json!({ "reason": 1 }),
        json!({ "reason": null }),
        json!({ "reason": ["logout"] }),
        Value::Null,
    ] {
        let (_t, home) = home();
        record_clean_end(Some(&home), Some(ID), &extra, NOW);
        assert!(markers(&home).is_empty(), "{extra}");
    }
}

#[test]
fn ids_that_fail_the_conversation_rule_write_nothing() {
    let long = "a".repeat(65);
    for id in [
        "-abc",
        "",
        long.as_str(),
        "a/b",
        "a.b",
        "../x",
        "a b",
        "a\nb",
        "é",
    ] {
        let (_t, home) = home();
        record_clean_end(Some(&home), Some(id), &reason("prompt_input_exit"), NOW);
        assert!(markers(&home).is_empty(), "{id:?}");
    }
    let (_t, home) = home();
    let max = "a".repeat(64);
    record_clean_end(Some(&home), Some(&max), &reason("logout"), NOW);
    assert_eq!(markers(&home), [max]);
}

#[test]
fn no_session_id_writes_nothing_a7() {
    let (_t, home) = home();
    record_clean_end(Some(&home), None, &reason("prompt_input_exit"), NOW);
    assert!(
        markers(&home).is_empty(),
        "the generated fallback id is never used"
    );
}

#[test]
fn no_home_or_no_registry_writes_nothing() {
    record_clean_end(None, Some(ID), &reason("logout"), NOW);
    let (_t, home) = home();
    fs::remove_file(home.join(".amplihack/agents/registry.json")).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(markers(&home).is_empty());
    let tmp = tempfile::tempdir().unwrap();
    record_clean_end(Some(tmp.path()), Some(ID), &reason("logout"), NOW);
    assert!(
        !tmp.path().join(".amplihack").exists(),
        "nothing is created"
    );
}

#[test]
fn a_symlinked_registry_writes_nothing_s20b() {
    let (tmp, home) = home();
    let reg = home.join(".amplihack/agents/registry.json");
    let real = tmp.path().join("real.json");
    fs::rename(&reg, &real).unwrap();
    std::os::unix::fs::symlink(&real, &reg).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(markers(&home).is_empty());
}

#[test]
fn a_symlinked_agents_directory_writes_nothing_s20b() {
    let (tmp, home) = home();
    let agents = home.join(".amplihack/agents");
    let real = tmp.path().join("real-agents");
    fs::rename(&agents, &real).unwrap();
    std::os::unix::fs::symlink(&real, &agents).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(
        !real.join("ended").exists(),
        "nothing written through the link"
    );
}

#[test]
fn an_unsafe_amplihack_directory_writes_nothing_s20b() {
    let (_t, home) = home();
    fs::set_permissions(home.join(".amplihack"), fs::Permissions::from_mode(0o777)).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(markers(&home).is_empty());

    let (_t, home) = self::home();
    let agents = home.join(".amplihack/agents");
    fs::set_permissions(&agents, fs::Permissions::from_mode(0o755)).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(
        markers(&home).is_empty(),
        "agents/ must pass check_private_dir"
    );
}

#[test]
fn a_symlinked_amplihack_directory_writes_nothing_s20b() {
    let (tmp, home) = home();
    let top = home.join(".amplihack");
    let real = tmp.path().join("real-amplihack");
    fs::rename(&top, &real).unwrap();
    std::os::unix::fs::symlink(&real, &top).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(!real.join("agents/ended").exists());
}

#[test]
fn a_later_end_replaces_the_marker() {
    let (_t, home) = home();
    record_clean_end(Some(&home), Some(ID), &reason("prompt_input_exit"), NOW);
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW + 5);
    assert_eq!(markers(&home), [ID], "one file, no temporary left behind");
    let body: Value = serde_json::from_slice(&fs::read(ended(&home).join(ID)).unwrap()).unwrap();
    assert_eq!(
        body,
        json!({ "session_id": ID, "reason": "logout", "at": NOW + 5 })
    );
}

#[test]
fn an_existing_open_ended_directory_is_not_used() {
    let (_t, home) = home();
    fs::create_dir(ended(&home)).unwrap();
    fs::set_permissions(ended(&home), fs::Permissions::from_mode(0o777)).unwrap();
    record_clean_end(Some(&home), Some(ID), &reason("logout"), NOW);
    assert!(markers(&home).is_empty(), "ensure_private_dir refuses it");
}
