//! Additional coverage tests for amplihack-remote public API.
//!
//! These exercise public types and methods from integrator, orchestrator,
//! commands, and packager modules to increase line coverage beyond what
//! inline (private-function) tests can reach.

use amplihack_remote::{
    BranchInfo, CommandMode, IntegrationSummary, Integrator, RemoteStatus, SecretMatch,
    SessionCounts, StartSummary, VM, VMOptions, VMSize,
};

// ── CommandMode ──────────────────────────────────────────────────────

#[test]
fn command_mode_display_and_parse() {
    assert_eq!(CommandMode::Auto.to_string(), "auto");
    assert_eq!(CommandMode::Ultrathink.to_string(), "ultrathink");
    assert_eq!(CommandMode::Analyze.to_string(), "analyze");
    assert_eq!(CommandMode::Fix.to_string(), "fix");

    assert_eq!("auto".parse::<CommandMode>().unwrap(), CommandMode::Auto);
    assert_eq!(
        "ultrathink".parse::<CommandMode>().unwrap(),
        CommandMode::Ultrathink
    );
    assert_eq!(
        "analyze".parse::<CommandMode>().unwrap(),
        CommandMode::Analyze
    );
    assert_eq!("fix".parse::<CommandMode>().unwrap(), CommandMode::Fix);

    assert!("invalid".parse::<CommandMode>().is_err());
    assert!("".parse::<CommandMode>().is_err());
    assert!("AUTO".parse::<CommandMode>().is_err());
}

#[test]
fn command_mode_serialization() {
    let mode = CommandMode::Ultrathink;
    let json = serde_json::to_string(&mode).unwrap();
    assert_eq!(json, r#""ultrathink""#);
    let m2: CommandMode = serde_json::from_str(&json).unwrap();
    assert_eq!(m2, CommandMode::Ultrathink);
}

// ── StartSummary / SessionCounts / RemoteStatus ─────────────────────

#[test]
fn start_summary_roundtrip() {
    let summary = StartSummary {
        session_ids: vec!["s1".into(), "s2".into()],
    };
    let json = serde_json::to_string(&summary).unwrap();
    let s2: StartSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(s2.session_ids.len(), 2);
}

#[test]
fn session_counts_roundtrip() {
    let counts = SessionCounts {
        running: 2,
        completed: 5,
        failed: 1,
        killed: 0,
        pending: 3,
    };
    let json = serde_json::to_string(&counts).unwrap();
    let c2: SessionCounts = serde_json::from_str(&json).unwrap();
    assert_eq!(c2.running, 2);
    assert_eq!(c2.completed, 5);
    assert_eq!(c2.pending, 3);
}

#[test]
fn remote_status_roundtrip() {
    let status = RemoteStatus {
        pool: amplihack_remote::PoolStatus {
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
    let s2: RemoteStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(s2.total_sessions, 2);
    assert_eq!(s2.pool.total_vms, 1);
}

// ── list_sessions / status ──────────────────────────────────────────

#[test]
fn list_sessions_empty_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let result = amplihack_remote::list_sessions(amplihack_remote::ListOptions {
        status: None,
        state_file: Some(path),
    });
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[test]
fn status_empty_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let result = amplihack_remote::status(amplihack_remote::StatusOptions {
        state_file: Some(path),
    });
    assert!(result.is_ok());
    let s = result.unwrap();
    assert_eq!(s.total_sessions, 0);
    assert_eq!(s.pool.total_vms, 0);
}

#[test]
fn status_with_pool_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let state = serde_json::json!({
        "sessions": {},
        "vm_pool": {
            "vm1": {
                "vm": {"name": "vm1", "size": "Standard_D2s_v3", "region": "eastus"},
                "capacity": 4,
                "active_sessions": ["s1", "s2"],
                "region": "eastus"
            }
        }
    });
    std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();
    let result = amplihack_remote::status(amplihack_remote::StatusOptions {
        state_file: Some(path),
    });
    assert!(result.is_ok());
    let s = result.unwrap();
    assert_eq!(s.pool.total_vms, 1);
    assert_eq!(s.pool.total_capacity, 4);
    assert_eq!(s.pool.active_sessions, 2);
    assert_eq!(s.pool.available_capacity, 2);
}

// ── Integrator + create_summary_report ──────────────────────────────

#[test]
fn create_summary_report_no_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let integrator = Integrator::new(dir.path()).unwrap();

    let summary = IntegrationSummary {
        branches: vec![
            BranchInfo {
                name: "feature-a".into(),
                commit: "abc12345def67890".into(),
                is_new: true,
            },
            BranchInfo {
                name: "feature-b".into(),
                commit: "def67890abc12345".into(),
                is_new: false,
            },
        ],
        commits_count: 5,
        files_changed: 12,
        logs_copied: true,
        has_conflicts: false,
        conflict_details: None,
    };

    let report = integrator.create_summary_report(&summary);
    assert!(report.contains("feature-a"));
    assert!(report.contains("NEW"));
    assert!(report.contains("UPDATED"));
    assert!(report.contains("Commits: 5"));
    assert!(report.contains("Files changed: 12"));
    assert!(report.contains("Logs copied: Yes"));
    assert!(report.contains("No conflicts detected"));
}

#[test]
fn create_summary_report_with_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let integrator = Integrator::new(dir.path()).unwrap();

    let summary = IntegrationSummary {
        branches: vec![BranchInfo {
            name: "main".into(),
            commit: "abc12345".into(),
            is_new: false,
        }],
        commits_count: 2,
        files_changed: 3,
        logs_copied: false,
        has_conflicts: true,
        conflict_details: Some("Branch 'main' has diverged".into()),
    };

    let report = integrator.create_summary_report(&summary);
    assert!(report.contains("WARNING: Conflicts detected!"));
    assert!(report.contains("Branch 'main' has diverged"));
    assert!(report.contains("Logs copied: No"));
}

#[test]
fn create_summary_report_empty_branches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let integrator = Integrator::new(dir.path()).unwrap();

    let summary = IntegrationSummary {
        branches: vec![],
        commits_count: 0,
        files_changed: 0,
        logs_copied: false,
        has_conflicts: false,
        conflict_details: None,
    };

    let report = integrator.create_summary_report(&summary);
    assert!(report.contains("Branches (0):"));
    assert!(report.contains("Commits: 0"));
}

#[test]
fn integration_summary_serialization_roundtrip() {
    let summary = IntegrationSummary {
        branches: vec![
            BranchInfo {
                name: "a".into(),
                commit: "abc".into(),
                is_new: true,
            },
            BranchInfo {
                name: "b".into(),
                commit: "def".into(),
                is_new: false,
            },
        ],
        commits_count: 10,
        files_changed: 7,
        logs_copied: true,
        has_conflicts: false,
        conflict_details: None,
    };
    let json = serde_json::to_string(&summary).unwrap();
    let s2: IntegrationSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(s2.branches.len(), 2);
    assert_eq!(s2.commits_count, 10);
    assert_eq!(s2.files_changed, 7);
    assert!(s2.logs_copied);
}

// ── VM / VMOptions ──────────────────────────────────────────────────

#[test]
fn vm_options_all_fields() {
    let opts = VMOptions {
        size: "Standard_D8s_v3".into(),
        region: Some("westus2".into()),
        vm_name: Some("my-vm".into()),
        no_reuse: true,
        keep_vm: true,
        azlin_extra_args: Some(vec!["--foo".into(), "bar".into()]),
        tunnel_port: Some(8080),
    };
    assert!(opts.no_reuse);
    assert!(opts.keep_vm);
    assert_eq!(opts.region.as_deref(), Some("westus2"));
    assert_eq!(opts.azlin_extra_args.as_ref().unwrap().len(), 2);
}

#[test]
fn vm_deserialization_full() {
    let json = r#"{
        "name": "amplihack-user-20250101",
        "size": "Standard_D4s_v3",
        "region": "westus",
        "created_at": "2025-01-01T12:00:00Z",
        "tags": {"env": "prod"}
    }"#;
    let vm: VM = serde_json::from_str(json).unwrap();
    assert_eq!(vm.name, "amplihack-user-20250101");
    assert!(vm.created_at.is_some());
    assert!(vm.tags.is_some());
}

// ── SecretMatch ─────────────────────────────────────────────────────

#[test]
fn secret_match_fields() {
    let m = SecretMatch {
        file_path: "config.toml".into(),
        line_number: 10,
        line_content: "password = secret".into(),
        pattern_name: "password_assignment".into(),
    };
    assert_eq!(m.line_number, 10);
    assert_eq!(m.pattern_name, "password_assignment");
}

// ── VMSize ──────────────────────────────────────────────────────────

#[test]
fn vm_size_display_and_parse() {
    let sizes = [VMSize::S, VMSize::M, VMSize::L, VMSize::XL];
    for size in &sizes {
        let s = size.to_string();
        let parsed: VMSize = s.parse().unwrap();
        assert_eq!(&parsed, size);
    }
    assert!("invalid".parse::<VMSize>().is_err());
}
