use std::path::PathBuf;
use std::str::FromStr;

use amplihack_remote::{
    CommandMode, ExecOptions, KillOptions, ListOptions, OutputOptions, RemoteError, SessionStatus,
    StartOptions, StatusOptions, VMOptions, VMSize, capture_output, exec, kill_session,
    list_sessions, start_sessions, status,
};

fn temp_state_file(name: &str) -> PathBuf {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    dir.keep().join(name)
}

fn temp_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    std::fs::create_dir(dir.path().join(".git")).expect("fake git marker should be created");
    dir
}

#[tokio::test]
async fn exec_rejects_invalid_inputs_before_remote_side_effects() {
    let repo = temp_git_repo();

    for (prompt, max_turns, timeout_minutes, api_key, expected) in [
        ("   ", 10, 120, "sk-ant-test", "prompt"),
        ("ship it", 0, 120, "sk-ant-test", "max-turns"),
        ("ship it", 51, 120, "sk-ant-test", "max-turns"),
        ("ship it", 10, 4, "sk-ant-test", "timeout"),
        ("ship it", 10, 481, "sk-ant-test", "timeout"),
        ("ship it", 10, 120, "", "ANTHROPIC_API_KEY"),
    ] {
        let result = exec(ExecOptions {
            repo_path: repo.path().to_path_buf(),
            command: CommandMode::Auto,
            prompt: prompt.to_string(),
            max_turns,
            vm_options: VMOptions::default(),
            timeout_minutes,
            skip_secret_scan: true,
            api_key: api_key.to_string(),
        })
        .await;
        let Err(error) = result else {
            panic!("invalid exec options should fail before azlin or packaging");
        };

        assert!(
            matches!(error, RemoteError::Validation(_)),
            "invalid exec input should be a validation error, got {error:?}"
        );
        assert!(
            error.to_string().contains(expected),
            "validation error should mention `{expected}`, got {error}"
        );
    }
}

#[tokio::test]
async fn start_sessions_validates_all_prompts_and_credentials_before_state_changes() {
    let repo = temp_git_repo();
    let state_file = temp_state_file("remote-state.json");

    for (prompts, api_key, expected) in [
        (Vec::<String>::new(), "sk-ant-test".to_string(), "prompt"),
        (vec!["   ".to_string()], "sk-ant-test".to_string(), "prompt"),
        (
            vec!["implement issue #536".to_string()],
            String::new(),
            "ANTHROPIC_API_KEY",
        ),
    ] {
        let result = start_sessions(StartOptions {
            repo_path: repo.path().to_path_buf(),
            prompts,
            command: CommandMode::Auto,
            max_turns: 10,
            size: VMSize::L,
            region: None,
            tunnel_port: None,
            api_key,
            state_file: Some(state_file.clone()),
        })
        .await;
        let Err(error) = result else {
            panic!("invalid start options should fail before pool allocation");
        };

        assert!(
            matches!(error, RemoteError::Validation(_)),
            "invalid start input should be a validation error, got {error:?}"
        );
        assert!(
            error.to_string().contains(expected),
            "validation error should mention `{expected}`, got {error}"
        );
        assert!(
            !state_file.exists(),
            "validation failure must not create or partially update remote state"
        );
    }
}

#[test]
fn command_mode_parses_and_displays_python_choices_only() {
    for (raw, expected) in [
        ("auto", CommandMode::Auto),
        ("ultrathink", CommandMode::Ultrathink),
        ("analyze", CommandMode::Analyze),
        ("fix", CommandMode::Fix),
    ] {
        let parsed = CommandMode::from_str(raw).expect("valid command mode should parse");
        assert_eq!(parsed, expected);
        assert_eq!(parsed.to_string(), raw);
    }

    assert!(CommandMode::from_str("prime").is_err());
    assert!(CommandMode::from_str("").is_err());
}

#[test]
fn list_sessions_filters_by_status_from_state_file() {
    let state_file = temp_state_file("remote-state.json");
    std::fs::write(
        &state_file,
        r#"{
          "sessions": {
            "sess-20260502-203014-4f2a": {
              "session_id": "sess-20260502-203014-4f2a",
              "vm_name": "vm-a",
              "workspace": "/workspace/sess-20260502-203014-4f2a",
              "tmux_session": "sess-20260502-203014-4f2a",
              "prompt": "running task",
              "command": "auto",
              "max_turns": 10,
              "status": "running",
              "memory_mb": 32768,
              "created_at": "2026-05-02T20:30:14Z",
              "started_at": "2026-05-02T20:31:02Z",
              "completed_at": null,
              "exit_code": null
            },
            "sess-20260502-203500-8a1c": {
              "session_id": "sess-20260502-203500-8a1c",
              "vm_name": "vm-a",
              "workspace": "/workspace/sess-20260502-203500-8a1c",
              "tmux_session": "sess-20260502-203500-8a1c",
              "prompt": "completed task",
              "command": "fix",
              "max_turns": 12,
              "status": "completed",
              "memory_mb": 32768,
              "created_at": "2026-05-02T20:35:00Z",
              "started_at": "2026-05-02T20:36:00Z",
              "completed_at": "2026-05-02T20:40:00Z",
              "exit_code": 0
            }
          }
        }"#,
    )
    .expect("state fixture should be written");

    let sessions = list_sessions(ListOptions {
        status: Some(SessionStatus::Running),
        state_file: Some(state_file),
    })
    .expect("list_sessions should read local state without Azure");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "sess-20260502-203014-4f2a");
    assert_eq!(sessions[0].memory_mb, 32768);
}

#[tokio::test]
async fn output_and_kill_report_unknown_session_as_exit_code_three_error() {
    let state_file = temp_state_file("remote-state.json");
    std::fs::write(&state_file, r#"{"sessions":{},"vm_pool":{}}"#)
        .expect("state fixture should be written");

    let output_result = capture_output(OutputOptions {
        session_id: "sess-20260502-203014-4f2a".to_string(),
        lines: 100,
        state_file: Some(state_file.clone()),
    })
    .await;
    let Err(output_error) = output_result else {
        panic!("unknown output session should error");
    };
    assert!(matches!(output_error, RemoteError::SessionNotFound { .. }));

    let kill_result = kill_session(KillOptions {
        session_id: "sess-20260502-203014-4f2a".to_string(),
        force: false,
        state_file: Some(state_file),
    })
    .await;
    let Err(kill_error) = kill_result else {
        panic!("unknown kill session should error");
    };
    assert!(matches!(kill_error, RemoteError::SessionNotFound { .. }));
}

#[test]
fn status_reports_pool_and_session_counts_from_state_file() {
    let state_file = temp_state_file("remote-state.json");
    std::fs::write(
        &state_file,
        r#"{
          "sessions": {
            "sess-20260502-203014-4f2a": {
              "session_id": "sess-20260502-203014-4f2a",
              "vm_name": "vm-a",
              "workspace": "/workspace/sess-20260502-203014-4f2a",
              "tmux_session": "sess-20260502-203014-4f2a",
              "prompt": "running task",
              "command": "auto",
              "max_turns": 10,
              "status": "running",
              "memory_mb": 32768,
              "created_at": "2026-05-02T20:30:14Z",
              "started_at": "2026-05-02T20:31:02Z",
              "completed_at": null,
              "exit_code": null
            },
            "sess-20260502-203500-8a1c": {
              "session_id": "sess-20260502-203500-8a1c",
              "vm_name": "vm-a",
              "workspace": "/workspace/sess-20260502-203500-8a1c",
              "tmux_session": "sess-20260502-203500-8a1c",
              "prompt": "failed task",
              "command": "fix",
              "max_turns": 10,
              "status": "failed",
              "memory_mb": 32768,
              "created_at": "2026-05-02T20:35:00Z",
              "started_at": "2026-05-02T20:36:00Z",
              "completed_at": "2026-05-02T20:40:00Z",
              "exit_code": 1
            }
          },
          "vm_pool": {
            "vm-a": {
              "vm": {
                "name": "vm-a",
                "size": "Standard_E16s_v5",
                "region": "eastus",
                "ip_address": null,
                "created_at": "2026-05-02T20:30:00Z"
              },
              "capacity": 4,
              "active_sessions": ["sess-20260502-203014-4f2a"],
              "region": "eastus"
            }
          }
        }"#,
    )
    .expect("state fixture should be written");

    let remote_status = status(StatusOptions {
        state_file: Some(state_file),
    })
    .expect("status should read local state without Azure");

    assert_eq!(remote_status.pool.total_vms, 1);
    assert_eq!(remote_status.pool.total_capacity, 4);
    assert_eq!(remote_status.pool.active_sessions, 1);
    assert_eq!(remote_status.sessions.running, 1);
    assert_eq!(remote_status.sessions.failed, 1);
    assert_eq!(remote_status.total_sessions, 2);
}
