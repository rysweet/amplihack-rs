//! Unit tests for the `commands` module (validators, CommandMode, pool helpers,
//! state file I/O, and session release).

use std::path::PathBuf;
use std::str::FromStr;

use amplihack_remote::commands::*;
use amplihack_remote::orchestrator::VM;
use amplihack_remote::vm_pool::VMPoolEntry;

// ── CommandMode ────────────────────────────────────────────────

#[test]
fn command_mode_from_str_valid() {
    assert_eq!(CommandMode::from_str("auto").unwrap(), CommandMode::Auto);
    assert_eq!(
        CommandMode::from_str("ultrathink").unwrap(),
        CommandMode::Ultrathink
    );
    assert_eq!(
        CommandMode::from_str("analyze").unwrap(),
        CommandMode::Analyze
    );
    assert_eq!(CommandMode::from_str("fix").unwrap(), CommandMode::Fix);
}

#[test]
fn command_mode_from_str_invalid() {
    let err = CommandMode::from_str("bogus").unwrap_err();
    assert!(err.contains("invalid command mode"));
}

#[test]
fn command_mode_display() {
    assert_eq!(CommandMode::Auto.to_string(), "auto");
    assert_eq!(CommandMode::Ultrathink.to_string(), "ultrathink");
    assert_eq!(CommandMode::Analyze.to_string(), "analyze");
    assert_eq!(CommandMode::Fix.to_string(), "fix");
}

#[test]
fn command_mode_roundtrip() {
    for mode in [
        CommandMode::Auto,
        CommandMode::Ultrathink,
        CommandMode::Analyze,
        CommandMode::Fix,
    ] {
        let s = mode.to_string();
        assert_eq!(CommandMode::from_str(&s).unwrap(), mode);
    }
}

#[test]
fn command_mode_serde_roundtrip() {
    for mode in [
        CommandMode::Auto,
        CommandMode::Ultrathink,
        CommandMode::Analyze,
        CommandMode::Fix,
    ] {
        let json = serde_json::to_string(&mode).unwrap();
        let back: CommandMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode);
    }
}

// ── SessionCounts serde ────────────────────────────────────────

#[test]
fn session_counts_serde_roundtrip() {
    let counts = SessionCounts {
        running: 1,
        completed: 2,
        failed: 3,
        killed: 4,
        pending: 5,
    };
    let json = serde_json::to_string(&counts).unwrap();
    let back: SessionCounts = serde_json::from_str(&json).unwrap();
    assert_eq!(back.running, 1);
    assert_eq!(back.completed, 2);
    assert_eq!(back.failed, 3);
    assert_eq!(back.killed, 4);
    assert_eq!(back.pending, 5);
}

// ── VMPoolEntry helpers ────────────────────────────────────────

fn make_entry(cap: usize, active: usize) -> VMPoolEntry {
    VMPoolEntry {
        vm: VM {
            name: format!("vm-{cap}"),
            size: "Standard_D8".into(),
            region: "eastus".into(),
            created_at: None,
            tags: None,
        },
        capacity: cap,
        active_sessions: (0..active).map(|i| format!("s-{i}")).collect(),
        region: "eastus".into(),
    }
}

#[test]
fn vm_pool_entry_available_capacity() {
    let e = make_entry(4, 1);
    assert_eq!(e.available_capacity(), 3);
}

#[test]
fn vm_pool_entry_at_capacity() {
    let e = make_entry(2, 2);
    assert_eq!(e.available_capacity(), 0);
}

#[test]
fn vm_pool_entry_over_capacity_saturates() {
    let e = make_entry(1, 3);
    assert_eq!(e.available_capacity(), 0);
}

// ── OutputResult / StartSummary serde ──────────────────────────

#[test]
fn start_summary_serde_roundtrip() {
    let s = StartSummary {
        session_ids: vec!["a".into(), "b".into()],
    };
    let json = serde_json::to_string(&s).unwrap();
    let back: StartSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(back.session_ids, vec!["a", "b"]);
}

// ── RemoteStatus serde ─────────────────────────────────────────

#[test]
fn remote_status_serde_roundtrip() {
    let status = RemoteStatus {
        pool: amplihack_remote::vm_pool::PoolStatus {
            total_vms: 1,
            total_capacity: 4,
            active_sessions: 2,
            available_capacity: 2,
        },
        sessions: SessionCounts {
            running: 1,
            completed: 0,
            failed: 0,
            killed: 0,
            pending: 1,
        },
        total_sessions: 2,
        vms: vec![],
    };
    let json = serde_json::to_string(&status).unwrap();
    let back: RemoteStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back.total_sessions, 2);
    assert_eq!(back.pool.total_vms, 1);
}

// ── ExecOptions / StartOptions / ListOptions construction ──────

#[test]
fn exec_options_construction() {
    let opts = ExecOptions {
        repo_path: PathBuf::from("/repo"),
        command: CommandMode::Auto,
        prompt: "hello".into(),
        max_turns: 10,
        vm_options: amplihack_remote::orchestrator::VMOptions::default(),
        timeout_minutes: 30,
        skip_secret_scan: false,
        api_key: "key".into(),
    };
    assert_eq!(opts.command, CommandMode::Auto);
    assert_eq!(opts.max_turns, 10);
}

#[test]
fn list_options_construction() {
    let opts = ListOptions {
        status: None,
        state_file: None,
    };
    assert!(opts.status.is_none());
}

#[test]
fn kill_options_construction() {
    let opts = KillOptions {
        session_id: "s-1".into(),
        force: true,
        state_file: None,
    };
    assert!(opts.force);
    assert_eq!(opts.session_id, "s-1");
}
