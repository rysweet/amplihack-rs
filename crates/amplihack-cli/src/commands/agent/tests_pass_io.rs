//! Unit tests for the I/O a pass saves: a `/proc` file longer than the first
//! read is still read whole, and an `update` whose result is the file
//! already on disk does not replace it.

use super::registry::*;
use super::store::*;
use super::tests_proc_tree::*;
use serde_json::json;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// 2026-10-30T09:12:04Z.
const T0: i64 = 1_793_351_524;

#[test]
fn a_command_line_longer_than_a_page_is_read_whole() {
    let t = tree();
    let long = "x".repeat(10_000);
    add(
        &t,
        200,
        100,
        "claude",
        &["claude", "--resume", &long],
        7,
        'S',
    );
    let s = snap(&t);
    assert_eq!(s.argv(200).map(|a| a[2].len()), Some(10_000));
}

fn entry() -> Entry {
    serde_json::from_value(json!({
        "state": "active", "tmux_session": "coord", "tmux_window": "main",
        "launch_dir": "/srv/project", "registered_at": "2026-10-30T09:12:04Z",
        "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d"
    }))
    .unwrap()
}

/// The inode and contents of `registry.json`; a rewrite is a rename, so a
/// new inode means the file was replaced.
fn file(dir: &AgentsDir) -> (u64, Vec<u8>) {
    let path = dir.registry();
    (fs::metadata(&path).unwrap().ino(), fs::read(&path).unwrap())
}

#[test]
fn an_update_that_changes_nothing_leaves_the_file_alone() {
    let home = tempfile::tempdir().unwrap();
    let dir = AgentsDir::open(home.path()).unwrap();
    update(&dir, T0, |reg| {
        reg.agents.insert("coordinator".into(), entry());
        Ok(())
    })
    .unwrap();
    let written = file(&dir);
    update(&dir, T0, |reg| {
        reg.agents.get_mut("coordinator").unwrap().lost_at = None;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        file(&dir),
        written,
        "nothing changed, so nothing is written"
    );
    update(&dir, T0, |reg| {
        reg.agents.get_mut("coordinator").unwrap().lost_at = Some(fmt_ts(T0));
        Ok(())
    })
    .unwrap();
    let changed = file(&dir);
    assert_ne!(changed.0, written.0, "a change replaces the file");
    assert!(String::from_utf8_lossy(&changed.1).contains("\"lost_at\""));
}

#[test]
fn a_file_in_another_layout_is_rewritten_once() {
    let home = tempfile::tempdir().unwrap();
    let dir = AgentsDir::open(home.path()).unwrap();
    let compact = json!({ "version": 1, "agents": { "coordinator": entry() } }).to_string();
    fs::write(dir.registry(), &compact).unwrap();
    fs::set_permissions(dir.registry(), fs::Permissions::from_mode(0o600)).unwrap();
    update(&dir, T0, |_| Ok(())).unwrap();
    let pretty = file(&dir);
    assert_ne!(pretty.1, compact.as_bytes(), "the layout is normalised");
    update(&dir, T0, |_| Ok(())).unwrap();
    assert_eq!(file(&dir), pretty);
}
