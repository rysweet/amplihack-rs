use super::*;
use crate::command_error;
use crate::test_support::{home_env_lock, restore_cwd, set_cwd};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn restore_var(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => unsafe { env::set_var(name, value) },
        None => unsafe { env::remove_var(name) },
    }
}

fn write_executable(path: &Path, content: &str) {
    use std::io::Write as _;
    let mut file = fs::File::create(path).unwrap();
    file.write_all(content.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn native_reasoner_backend_propagates_shared_env_context() {
    let _home_guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let reasoner = temp.path().join("claude");
    let amplihack_home = temp.path().join("amplihack-home");
    fs::create_dir_all(&amplihack_home).unwrap();
    write_executable(
        &reasoner,
        "#!/bin/sh\nprintf '%s\\n' \"$AMPLIHACK_AGENT_BINARY|$AMPLIHACK_NONINTERACTIVE|$AMPLIHACK_TREE_ID|$AMPLIHACK_SESSION_DEPTH|$AMPLIHACK_MAX_DEPTH|$AMPLIHACK_MAX_SESSIONS|$AMPLIHACK_HOME|$AMPLIHACK_GRAPH_DB_PATH\"\n",
    );

    let prev_home = env::var_os("AMPLIHACK_HOME");
    let prev_tree = env::var_os("AMPLIHACK_TREE_ID");
    let prev_depth = env::var_os("AMPLIHACK_SESSION_DEPTH");
    let prev_max_depth = env::var_os("AMPLIHACK_MAX_DEPTH");
    let prev_max_sessions = env::var_os("AMPLIHACK_MAX_SESSIONS");
    let previous_cwd = set_cwd(temp.path()).unwrap();
    unsafe {
        env::set_var("AMPLIHACK_HOME", &amplihack_home);
        env::set_var("AMPLIHACK_TREE_ID", "tree1234");
        env::set_var("AMPLIHACK_SESSION_DEPTH", "2");
        env::set_var("AMPLIHACK_MAX_DEPTH", "4");
        env::set_var("AMPLIHACK_MAX_SESSIONS", "12");
    }

    let output = NativeReasonerBackend::Claude(reasoner)
        .complete("inspect")
        .unwrap();

    restore_cwd(&previous_cwd).unwrap();
    restore_var("AMPLIHACK_HOME", prev_home);
    restore_var("AMPLIHACK_TREE_ID", prev_tree);
    restore_var("AMPLIHACK_SESSION_DEPTH", prev_depth);
    restore_var("AMPLIHACK_MAX_DEPTH", prev_max_depth);
    restore_var("AMPLIHACK_MAX_SESSIONS", prev_max_sessions);

    assert_eq!(
        output.trim(),
        format!(
            "claude|1|tree1234|2|4|12|{}|{}",
            amplihack_home.display(),
            temp.path().join(".amplihack").join("graph_db").display()
        )
    );
}

#[test]
fn parses_native_setup_command() {
    match parse_native_fleet_command(&[String::from("setup")]) {
        Some(NativeFleetCommand::Setup) => {}
        other => panic!("expected setup command, got {other:?}"),
    }
}

#[test]
fn parses_native_status_command() {
    match parse_native_fleet_command(&[String::from("status")]) {
        Some(NativeFleetCommand::Status) => {}
        other => panic!("expected status command, got {other:?}"),
    }
}

#[test]
fn parses_native_snapshot_command() {
    match parse_native_fleet_command(&[String::from("snapshot")]) {
        Some(NativeFleetCommand::Snapshot) => {}
        other => panic!("expected snapshot command, got {other:?}"),
    }
}

#[test]
fn parses_native_tui_command() {
    match parse_native_fleet_command(&[
        String::from("tui"),
        String::from("--interval"),
        String::from("10"),
        String::from("--capture-lines"),
        String::from("80"),
    ]) {
        Some(NativeFleetCommand::Tui {
            interval,
            capture_lines,
        }) => {
            assert_eq!(interval, 10);
            assert_eq!(capture_lines, 80);
        }
        other => panic!("expected tui command, got {other:?}"),
    }
}

#[test]
fn parses_native_start_command() {
    match parse_native_fleet_command(&[
        String::from("start"),
        String::from("--max-cycles"),
        String::from("2"),
        String::from("--interval"),
        String::from("15"),
        String::from("--adopt"),
        String::from("--stuck-threshold"),
        String::from("45"),
        String::from("--max-agents-per-vm"),
        String::from("4"),
        String::from("--capture-lines"),
        String::from("80"),
    ]) {
        Some(NativeFleetCommand::Start {
            max_cycles,
            interval,
            adopt,
            stuck_threshold,
            max_agents_per_vm,
            capture_lines,
        }) => {
            assert_eq!(max_cycles, 2);
            assert_eq!(interval, 15);
            assert!(adopt);
            assert_eq!(stuck_threshold, 45.0);
            assert_eq!(max_agents_per_vm, 4);
            assert_eq!(capture_lines, 80);
        }
        other => panic!("expected start command, got {other:?}"),
    }
}

#[test]
fn parses_native_run_once_command() {
    match parse_native_fleet_command(&[String::from("run-once")]) {
        Some(NativeFleetCommand::RunOnce) => {}
        other => panic!("expected run-once command, got {other:?}"),
    }
}

#[test]
fn parses_native_dry_run_command() {
    match parse_native_fleet_command(&[
        String::from("dry-run"),
        String::from("--vm"),
        String::from("vm-1"),
        String::from("--priorities"),
        String::from("Quality first"),
        String::from("--backend"),
        String::from("auto"),
    ]) {
        Some(NativeFleetCommand::DryRun {
            vm,
            priorities,
            backend,
        }) => {
            assert_eq!(vm, vec!["vm-1".to_string()]);
            assert_eq!(priorities, "Quality first");
            assert_eq!(backend, "auto");
        }
        other => panic!("expected dry-run command, got {other:?}"),
    }
}

#[test]
fn parses_native_scout_command() {
    match parse_native_fleet_command(&[
        String::from("scout"),
        String::from("--vm"),
        String::from("vm-1"),
        String::from("--session"),
        String::from("vm-1:work-1"),
        String::from("--skip-adopt"),
        String::from("--incremental"),
        String::from("--save"),
        String::from("/tmp/scout.json"),
    ]) {
        Some(NativeFleetCommand::Scout {
            vm,
            session_target,
            skip_adopt,
            incremental,
            save_path,
        }) => {
            assert_eq!(vm.as_deref(), Some("vm-1"));
            assert_eq!(session_target.as_deref(), Some("vm-1:work-1"));
            assert!(skip_adopt);
            assert!(incremental);
            assert_eq!(save_path.as_deref(), Some(Path::new("/tmp/scout.json")));
        }
        other => panic!("expected scout command, got {other:?}"),
    }
}

#[test]
fn parses_native_advance_command() {
    match parse_native_fleet_command(&[
        String::from("advance"),
        String::from("--vm"),
        String::from("vm-1"),
        String::from("--session"),
        String::from("vm-1:work-1"),
        String::from("--force"),
        String::from("--save"),
        String::from("/tmp/advance.json"),
    ]) {
        Some(NativeFleetCommand::Advance {
            vm,
            session_target,
            force,
            save_path,
        }) => {
            assert_eq!(vm.as_deref(), Some("vm-1"));
            assert_eq!(session_target.as_deref(), Some("vm-1:work-1"));
            assert!(force);
            assert_eq!(save_path.as_deref(), Some(Path::new("/tmp/advance.json")));
        }
        other => panic!("expected advance command, got {other:?}"),
    }
}

#[test]
fn parses_native_auth_command() {
    match parse_native_fleet_command(&[
        String::from("auth"),
        String::from("vm-1"),
        String::from("--services"),
        String::from("github"),
        String::from("--services"),
        String::from("azure"),
    ]) {
        Some(NativeFleetCommand::Auth { vm_name, services }) => {
            assert_eq!(vm_name, "vm-1");
            assert_eq!(services, vec!["github".to_string(), "azure".to_string()]);
        }
        other => panic!("expected auth command, got {other:?}"),
    }
}

#[test]
fn parses_native_adopt_command() {
    match parse_native_fleet_command(&[
        String::from("adopt"),
        String::from("vm-1"),
        String::from("--sessions"),
        String::from("work-1"),
        String::from("--sessions"),
        String::from("work-2"),
    ]) {
        Some(NativeFleetCommand::Adopt { vm_name, sessions }) => {
            assert_eq!(vm_name, "vm-1");
            assert_eq!(sessions, vec!["work-1".to_string(), "work-2".to_string()]);
        }
        other => panic!("expected adopt command, got {other:?}"),
    }
}

#[test]
fn parses_native_observe_command() {
    match parse_native_fleet_command(&[String::from("observe"), String::from("vm-1")]) {
        Some(NativeFleetCommand::Observe { vm_name }) => assert_eq!(vm_name, "vm-1"),
        other => panic!("expected observe command, got {other:?}"),
    }
}

#[test]
fn parses_native_report_command() {
    match parse_native_fleet_command(&[String::from("report")]) {
        Some(NativeFleetCommand::Report) => {}
        other => panic!("expected report command, got {other:?}"),
    }
}

#[test]
fn parses_native_queue_command() {
    match parse_native_fleet_command(&[String::from("queue")]) {
        Some(NativeFleetCommand::Queue) => {}
        other => panic!("expected queue command, got {other:?}"),
    }
}

#[test]
fn parses_native_add_task_command_with_defaults() {
    match parse_native_fleet_command(&[String::from("add-task"), String::from("Fix the login bug")])
    {
        Some(NativeFleetCommand::AddTask {
            prompt,
            repo,
            priority,
            agent,
            mode,
            max_turns,
            protected,
        }) => {
            assert_eq!(prompt, "Fix the login bug");
            assert_eq!(repo, "");
            assert!(matches!(priority, NativeTaskPriorityArg::Medium));
            assert!(matches!(agent, NativeAgentArg::Claude));
            assert!(matches!(mode, NativeAgentModeArg::Auto));
            assert_eq!(max_turns, DEFAULT_MAX_TURNS);
            assert!(!protected);
        }
        other => panic!("expected add-task command, got {other:?}"),
    }
}

#[test]
fn parses_native_graph_command() {
    match parse_native_fleet_command(&[String::from("graph")]) {
        Some(NativeFleetCommand::Graph) => {}
        other => panic!("expected graph command, got {other:?}"),
    }
}

#[test]
fn parses_native_copilot_status_command() {
    match parse_native_fleet_command(&[String::from("copilot-status")]) {
        Some(NativeFleetCommand::CopilotStatus) => {}
        other => panic!("expected copilot-status command, got {other:?}"),
    }
}

#[test]
fn parses_native_copilot_log_command() {
    match parse_native_fleet_command(&[
        String::from("copilot-log"),
        String::from("--tail"),
        String::from("3"),
    ]) {
        Some(NativeFleetCommand::CopilotLog { tail }) => assert_eq!(tail, 3),
        other => panic!("expected copilot-log command, got {other:?}"),
    }
}

#[test]
fn parses_native_dashboard_command() {
    match parse_native_fleet_command(&[String::from("dashboard")]) {
        Some(NativeFleetCommand::Dashboard) => {}
        other => panic!("expected dashboard command, got {other:?}"),
    }
}

#[test]
fn parses_native_watch_command() {
    match parse_native_fleet_command(&[
        String::from("watch"),
        String::from("test-vm"),
        String::from("session-1"),
    ]) {
        Some(NativeFleetCommand::Watch {
            vm_name,
            session_name,
            lines,
        }) => {
            assert_eq!(vm_name, "test-vm");
            assert_eq!(session_name, "session-1");
            assert_eq!(lines, 30);
        }
        other => panic!("expected watch command, got {other:?}"),
    }
}

#[test]
fn parses_native_project_add_command() {
    match parse_native_fleet_command(&[
        String::from("project"),
        String::from("add"),
        String::from("https://github.com/org/repo"),
        String::from("--identity"),
        String::from("bot-account"),
        String::from("--priority"),
        String::from("high"),
        String::from("--name"),
        String::from("custom-name"),
    ]) {
        Some(NativeFleetCommand::Project {
            command:
                NativeFleetProjectCommand::Add {
                    repo_url,
                    identity,
                    priority,
                    name,
                },
        }) => {
            assert_eq!(repo_url, "https://github.com/org/repo");
            assert_eq!(identity, "bot-account");
            assert!(matches!(priority, NativeProjectPriorityArg::High));
            assert_eq!(name, "custom-name");
        }
        other => panic!("expected project add command, got {other:?}"),
    }
}

#[test]
fn empty_and_help_fleet_commands_bypass_subcommand_parsing() {
    assert!(parse_native_fleet_command(&[]).is_none());
    assert!(
        parse_native_fleet_command(&[String::from("status"), String::from("--help")]).is_none()
    );
}

#[test]
fn azlin_path_prefers_environment_override() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("custom-azlin");
    write_executable(&azlin, "#!/bin/sh\nexit 0\n");

    let previous = env::var_os("AZLIN_PATH");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };

    let found = get_azlin_path().unwrap();

    match previous {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }

    assert_eq!(found, azlin);
}

#[test]
fn run_setup_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(&azlin, "#!/bin/sh\necho 'azlin 1.2.3'\n");

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::set_var("AZLIN_PATH", &azlin);
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
    }

    let result = run_setup();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_setup_returns_exit_error_when_azlin_missing() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let empty_path = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::remove_var("AZLIN_PATH");
        env::set_var("PATH", empty_path.path());
        env::set_var("HOME", home.path());
    }

    let result = run_setup();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    let err = result.expect_err("missing azlin should fail");
    assert_eq!(command_error::exit_code(&err), Some(1));
}

#[test]
fn parse_vm_json_supports_list_and_location_fallback() {
    let vms =
        FleetState::parse_vm_json(r#"[{"name":"vm-1","status":"Running","location":"eastus"}]"#);
    assert_eq!(vms.len(), 1);
    assert_eq!(vms[0].name, "vm-1");
    assert_eq!(vms[0].region, "eastus");
}

#[test]
fn parse_vm_json_supports_dict_wrapped_vms() {
    let vms = FleetState::parse_vm_json(
        r#"{"vms":[{"name":"vm-2","status":"Stopped","region":"westus2"}]}"#,
    );
    assert_eq!(vms.len(), 1);
    assert_eq!(vms[0].name, "vm-2");
    assert_eq!(vms[0].status, "Stopped");
}

#[test]
fn parse_vm_text_extracts_rows() {
    let text = concat!(
        "│ Session     │ Tmux │ OS     │ Status  │ IP       │ Region  │\n",
        "┣━━━━━━━━━━━━━╋━━━━━━╋━━━━━━━━╋━━━━━━━━━╋━━━━━━━━━━╋━━━━━━━━━┫\n",
        "│ fleet-vm-1  │ yes  │ Ubuntu │ Running │ 10.0.0.5 │ westus2 │\n",
        "│ fleet-vm-2  │ no   │ Ubuntu │ Stopped │ 10.0.0.6 │ eastus  │\n"
    );
    let vms = FleetState::parse_vm_text(text);
    assert_eq!(vms.len(), 2);
    assert_eq!(vms[0].name, "fleet-vm-1");
    assert_eq!(vms[0].status, "Running");
    assert_eq!(vms[1].region, "eastus");
}

#[test]
fn poll_tmux_sessions_parses_multiple_sessions() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        "#!/bin/sh\nif [ \"$1\" = connect ]; then\n  printf \"amplihack-ultra|||1|||1\\nbart|||2|||0\\n\";\nelse\n  exit 1\nfi\n",
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::set_var("AZLIN_PATH", &azlin);
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
    }

    let state = FleetState::new(azlin.clone());
    let sessions = state.poll_tmux_sessions("test-vm");

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session_name, "amplihack-ultra");
    assert_eq!(sessions[0].windows, 1);
    assert!(sessions[0].attached);
    assert_eq!(sessions[1].windows, 2);
    assert!(!sessions[1].attached);
}

#[test]
fn summary_uses_expected_icons() {
    let mut state = FleetState::new(PathBuf::from("/tmp/azlin"));
    state.vms = vec![
        VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: String::new(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus".to_string(),
            tmux_sessions: vec![
                TmuxSessionInfo {
                    session_name: "s1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Completed,
                    last_output: String::new(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
                TmuxSessionInfo {
                    session_name: "s2".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Stuck,
                    last_output: String::new(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
            ],
        },
        VmInfo {
            name: "vm-2".to_string(),
            session_name: "vm-2".to_string(),
            os: String::new(),
            status: "Stopped".to_string(),
            ip: String::new(),
            region: "eastus".to_string(),
            tmux_sessions: Vec::new(),
        },
    ];

    let summary = state.summary();

    assert!(summary.contains("[+] vm-1 (westus) - Running"));
    assert!(summary.contains("[=] s1 (completed)"));
    assert!(summary.contains("[!] s2 (stuck)"));
    assert!(summary.contains("[-] vm-2 (eastus) - Stopped"));
}

#[test]
fn task_queue_summary_matches_python_shape() {
    let queue = TaskQueue {
        tasks: vec![
            FleetTask {
                id: "abc123".to_string(),
                prompt: "High priority task".to_string(),
                repo_url: String::new(),
                branch: String::new(),
                priority: TaskPriority::High,
                status: TaskStatus::Queued,
                agent_command: "claude".to_string(),
                agent_mode: "auto".to_string(),
                max_turns: DEFAULT_MAX_TURNS,
                protected: false,
                assigned_vm: None,
                assigned_session: None,
                assigned_at: None,
                created_at: now_isoformat(),
                started_at: None,
                completed_at: None,
                result: None,
                pr_url: None,
                error: None,
            },
            FleetTask {
                id: "def456".to_string(),
                prompt: "Assigned task".to_string(),
                repo_url: String::new(),
                branch: String::new(),
                priority: TaskPriority::Low,
                status: TaskStatus::Assigned,
                agent_command: "claude".to_string(),
                agent_mode: "auto".to_string(),
                max_turns: DEFAULT_MAX_TURNS,
                protected: false,
                assigned_vm: Some("vm-1".to_string()),
                assigned_session: None,
                assigned_at: None,
                created_at: now_isoformat(),
                started_at: None,
                completed_at: None,
                result: None,
                pr_url: None,
                error: None,
            },
        ],
        persist_path: None,
    };

    let summary = queue.summary();
    assert!(summary.contains("Task Queue (2 tasks)"));
    assert!(summary.contains("  QUEUED (1):"));
    assert!(summary.contains("    [H] abc123: High priority task"));
    assert!(summary.contains("  ASSIGNED (1):"));
    assert!(summary.contains("    [L] def456: Assigned task -> vm-1"));
}

#[test]
fn task_queue_persists_and_loads_python_compatible_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("task_queue.json");

    let mut queue = TaskQueue::load(Some(path.clone())).unwrap();
    let task = queue
        .add_task(
            "Persistent task",
            "https://github.com/org/repo",
            TaskPriority::High,
            "amplifier",
            "ultrathink",
            50,
        )
        .unwrap();

    let loaded = TaskQueue::load(Some(path.clone())).unwrap();
    assert_eq!(loaded.tasks.len(), 1);
    assert_eq!(loaded.tasks[0].id, task.id);
    assert_eq!(loaded.tasks[0].repo_url, "https://github.com/org/repo");
    assert_eq!(loaded.tasks[0].agent_command, "amplifier");
    assert_eq!(loaded.tasks[0].agent_mode, "ultrathink");
    assert_eq!(loaded.tasks[0].max_turns, 50);
}

#[test]
fn task_queue_load_surfaces_corrupt_json_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("task_queue.json");
    fs::write(&path, "{not json").unwrap();

    let error = TaskQueue::load(Some(path.clone())).unwrap_err();
    let backup = path.with_extension("json.bak");

    assert!(error.to_string().contains("failed to parse"));
    assert!(error.to_string().contains("fleet task queue JSON"));
    assert!(backup.exists());
    assert_eq!(fs::read_to_string(&backup).unwrap(), "{not json");
}

#[test]
fn run_add_task_creates_default_queue_file() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_add_task(
        "Refactor auth module",
        "https://github.com/org/repo",
        NativeTaskPriorityArg::High,
        NativeAgentArg::Amplifier,
        NativeAgentModeArg::Ultrathink,
        50,
        false,
    );

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());

    let queue_path = home.path().join(".amplihack/fleet/task_queue.json");
    let loaded = TaskQueue::load(Some(queue_path)).unwrap();
    assert_eq!(loaded.tasks.len(), 1);
    assert_eq!(loaded.tasks[0].prompt, "Refactor auth module");
    assert_eq!(loaded.tasks[0].repo_url, "https://github.com/org/repo");
    assert_eq!(loaded.tasks[0].priority, TaskPriority::High);
}

#[test]
fn fleet_graph_summary_matches_python_shape() {
    let graph = FleetGraphSummary {
        node_types: vec![
            "project".to_string(),
            "task".to_string(),
            "task".to_string(),
        ],
        edge_types: vec!["contains".to_string(), "conflicts".to_string()],
    };

    let summary = graph.summary();
    assert!(summary.contains("Fleet Graph: 3 nodes, 2 edges"));
    assert!(summary.contains("  Nodes: project=1, task=2"));
    assert!(summary.contains("  Edges: conflicts=1, contains=1"));
    assert!(summary.contains("  !! 1 conflicts detected"));
}

#[test]
fn fleet_graph_loads_python_json_shape() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.json");
    fs::write(
        &path,
        r#"{
  "nodes": {
"proj-1": {"type": "project", "label": "proj-1", "metadata": {}},
"task-1": {"type": "task", "label": "task-1", "metadata": {}}
  },
  "edges": [
{"source": "proj-1", "target": "task-1", "type": "contains", "metadata": {}}
  ]
}"#,
    )
    .unwrap();

    let graph = FleetGraphSummary::load(Some(path)).unwrap();
    assert_eq!(graph.node_types.len(), 2);
    assert_eq!(graph.edge_types, vec!["contains".to_string()]);
}

#[test]
fn fleet_graph_load_surfaces_corrupt_json_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.json");
    fs::write(&path, "{bad graph").unwrap();

    let error = FleetGraphSummary::load(Some(path.clone())).unwrap_err();
    let backup = path.with_extension("json.bak");

    assert!(error.to_string().contains("failed to parse"));
    assert!(error.to_string().contains("fleet graph JSON"));
    assert!(backup.exists());
    assert_eq!(fs::read_to_string(&backup).unwrap(), "{bad graph");
}

#[test]
fn fleet_dashboard_summary_matches_python_shape() {
    let dashboard = FleetDashboardSummary {
        projects: vec![ProjectInfo {
            repo_url: "https://github.com/org/repo".to_string(),
            name: "repo".to_string(),
            github_identity: "user1".to_string(),
            priority: "medium".to_string(),
            notes: String::new(),
            vms: vec!["vm-01".to_string()],
            tasks_total: 4,
            tasks_completed: 2,
            tasks_failed: 1,
            tasks_in_progress: 1,
            prs_created: vec!["pr-url".to_string()],
            estimated_cost_usd: 5.25,
            started_at: Some(now_isoformat()),
            last_activity: Some(now_isoformat()),
        }],
        persist_path: None,
    };

    let summary = dashboard.summary();
    assert!(summary.contains("FLEET DASHBOARD"));
    assert!(summary.contains("Projects: 1"));
    assert!(summary.contains("Tasks: 2/4 completed"));
    assert!(summary.contains("PRs created: 1"));
    assert!(summary.contains("Estimated cost: $5.25"));
    assert!(summary.contains("[repo] (user1)"));
    assert!(summary.contains("!! 1 failed tasks"));
}

#[test]
fn fleet_dashboard_updates_from_queue() {
    let mut dashboard = FleetDashboardSummary {
        projects: Vec::new(),
        persist_path: None,
    };
    let mut queue = TaskQueue {
        tasks: Vec::new(),
        persist_path: None,
    };
    let mut completed = FleetTask::new(
        "Fix bug",
        "https://github.com/org/repo",
        TaskPriority::High,
        "claude",
        "auto",
        DEFAULT_MAX_TURNS,
    );
    completed.status = TaskStatus::Completed;
    completed.pr_url = Some("https://github.com/org/repo/pull/1".to_string());
    let mut assigned = FleetTask::new(
        "Add auth",
        "https://github.com/org/repo",
        TaskPriority::Medium,
        "claude",
        "auto",
        DEFAULT_MAX_TURNS,
    );
    assigned.status = TaskStatus::Assigned;
    assigned.assigned_vm = Some("vm-01".to_string());
    queue.tasks = vec![completed, assigned];

    dashboard.update_from_queue(&queue).unwrap();

    assert_eq!(dashboard.projects.len(), 1);
    let project = &dashboard.projects[0];
    assert_eq!(project.tasks_total, 2);
    assert_eq!(project.tasks_completed, 1);
    assert_eq!(project.tasks_in_progress, 1);
    assert_eq!(
        project.prs_created,
        vec!["https://github.com/org/repo/pull/1"]
    );
    assert_eq!(project.vms, vec!["vm-01".to_string()]);
}

#[test]
fn fleet_dashboard_load_surfaces_corrupt_json_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dashboard.json");
    fs::write(&path, "{bad dashboard").unwrap();

    let error = FleetDashboardSummary::load(Some(path.clone())).unwrap_err();
    let backup = path.with_extension("json.bak");

    assert!(error.to_string().contains("failed to parse"));
    assert!(error.to_string().contains("fleet dashboard JSON"));
    assert!(backup.exists());
    assert_eq!(fs::read_to_string(&backup).unwrap(), "{bad dashboard");
}

#[test]
fn run_project_add_persists_dashboard_and_projects_toml() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_project_add(
        "https://github.com/org/my-repo",
        "bot-account",
        "high",
        "custom-name",
    );

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());

    let dashboard =
        FleetDashboardSummary::load(Some(home.path().join(".amplihack/fleet/dashboard.json")))
            .unwrap();
    assert_eq!(dashboard.projects.len(), 1);
    assert_eq!(dashboard.projects[0].name, "custom-name");
    assert_eq!(dashboard.projects[0].github_identity, "bot-account");

    let projects =
        load_projects_registry(&home.path().join(".amplihack/fleet/projects.toml")).unwrap();
    assert!(projects.contains_key("custom-name"));
    assert_eq!(
        projects["custom-name"].repo_url,
        "https://github.com/org/my-repo"
    );
}

#[test]
fn run_project_remove_only_updates_dashboard() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };

    run_project_add("https://github.com/org/my-repo", "", "medium", "my-repo").unwrap();
    let result = run_project_remove("my-repo");

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());

    let dashboard =
        FleetDashboardSummary::load(Some(home.path().join(".amplihack/fleet/dashboard.json")))
            .unwrap();
    assert!(dashboard.projects.is_empty());

    let projects =
        load_projects_registry(&home.path().join(".amplihack/fleet/projects.toml")).unwrap();
    assert!(projects.contains_key("my-repo"));
}

#[test]
fn load_projects_registry_surfaces_corrupt_toml_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects.toml");
    fs::write(&path, "[project\nbroken").unwrap();

    let error = load_projects_registry(&path).unwrap_err();
    let backup = path.with_extension("toml.bak");

    assert!(error.to_string().contains("failed to parse"));
    assert!(error.to_string().contains("fleet projects registry TOML"));
    assert!(backup.exists());
    assert_eq!(fs::read_to_string(&backup).unwrap(), "[project\nbroken");
}

#[test]
fn render_project_list_matches_python_shape() {
    let dashboard = FleetDashboardSummary {
        projects: vec![ProjectInfo {
            name: "repo".to_string(),
            repo_url: "https://github.com/org/repo".to_string(),
            github_identity: "user1".to_string(),
            priority: "high".to_string(),
            tasks_total: 4,
            tasks_completed: 2,
            tasks_failed: 0,
            tasks_in_progress: 2,
            vms: vec!["vm-01".to_string(), "vm-02".to_string()],
            notes: "tracking auth work".to_string(),
            prs_created: vec!["https://github.com/org/repo/pull/1".to_string()],
            estimated_cost_usd: 0.0,
            started_at: None,
            last_activity: None,
        }],
        persist_path: None,
    };

    let rendered = render_project_list(&dashboard);
    assert!(rendered.contains("Fleet Projects (1)"));
    assert!(rendered.contains("[!!!] repo"));
    assert!(rendered.contains("Repo: https://github.com/org/repo"));
    assert!(rendered.contains("Identity: user1"));
    assert!(rendered.contains("Priority: high"));
    assert!(rendered.contains("VMs: 2 | Tasks: 2/4 | PRs: 1"));
    assert!(rendered.contains("Notes: tracking auth work"));
}

#[test]
fn render_copilot_status_matches_python_cases() {
    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path();

    assert_eq!(
        render_copilot_status(lock_dir).unwrap(),
        "Copilot: not active"
    );

    fs::write(lock_dir.join(".lock_active"), "locked").unwrap();
    assert_eq!(
        render_copilot_status(lock_dir).unwrap(),
        "Copilot: active (no goal)"
    );

    fs::write(lock_dir.join(".lock_goal"), "Fix authentication bug\n").unwrap();
    assert_eq!(
        render_copilot_status(lock_dir).unwrap(),
        "Copilot: active\nGoal: Fix authentication bug"
    );
}

#[test]
fn read_copilot_log_matches_python_shape() {
    let temp = tempfile::tempdir().unwrap();
    let log_dir = temp.path();
    let report = read_copilot_log(log_dir, 0).unwrap();
    assert_eq!(report.rendered, "No decisions recorded.");
    assert_eq!(report.malformed_entries, 0);

    fs::write(log_dir.join("decisions.jsonl"), "").unwrap();
    let report = read_copilot_log(log_dir, 0).unwrap();
    assert_eq!(report.rendered, "No decisions recorded.");
    assert_eq!(report.malformed_entries, 0);

    fs::write(
        log_dir.join("decisions.jsonl"),
        r#"{"timestamp":"2026-03-03T10:00:00","action":"send_input","reasoning":"Agent is idle at prompt","confidence":0.85}
{"timestamp":"2026-03-03T10:05:00","action":"wait","reasoning":"Agent has a tool call in flight","confidence":0.95}"#,
    )
    .unwrap();
    let report = read_copilot_log(log_dir, 0).unwrap();
    assert!(
        report
            .rendered
            .contains("[2026-03-03T10:00:00] send_input (confidence=0.85)")
    );
    assert!(report.rendered.contains("Agent is idle at prompt"));
    assert!(
        report
            .rendered
            .contains("[2026-03-03T10:05:00] wait (confidence=0.95)")
    );
    assert_eq!(report.malformed_entries, 0);
}

#[test]
fn read_copilot_log_applies_tail_and_tracks_malformed_entries() {
    let temp = tempfile::tempdir().unwrap();
    let log_dir = temp.path();
    fs::write(
        log_dir.join("decisions.jsonl"),
        r#"{"timestamp":"2026-03-03T10:00:00","action":"action_0","reasoning":"reason_0","confidence":0.8}
not-json
{"timestamp":"2026-03-03T10:08:00","action":"action_8","reasoning":"reason_8","confidence":0.8}
{"timestamp":"2026-03-03T10:09:00","action":"action_9","reasoning":"reason_9","confidence":0.8}"#,
    )
    .unwrap();

    let report = read_copilot_log(log_dir, 2).unwrap();
    assert!(!report.rendered.contains("action_0"));
    assert!(report.rendered.contains("action_8"));
    assert!(report.rendered.contains("action_9"));
    assert_eq!(report.malformed_entries, 1);
}

#[test]
fn observer_classifies_running_and_completed_output() {
    let azlin = PathBuf::from("/bin/true");
    let mut observer = FleetObserver::new(azlin);

    let (status, confidence, pattern) = observer.classify_output(
        &[
            "Step 5: Implementing authentication module".to_string(),
            "Reading file auth.py".to_string(),
        ],
        "vm-1",
        "sess-1",
    );
    assert_eq!(status, AgentStatus::Running);
    assert_eq!(confidence, CONFIDENCE_RUNNING);
    assert_eq!(pattern, r"Step \d+");

    let (status, confidence, pattern) = observer.classify_output(
        &[
            "Step 22: Creating pull request".to_string(),
            "PR #42 created: https://github.com/org/repo/pull/42".to_string(),
        ],
        "vm-1",
        "sess-1",
    );
    assert_eq!(status, AgentStatus::Completed);
    assert_eq!(confidence, CONFIDENCE_COMPLETION);
    assert_eq!(pattern, "completion_detected");
}

#[test]
fn observer_classifies_waiting_and_idle_output() {
    let azlin = PathBuf::from("/bin/true");
    let mut observer = FleetObserver::new(azlin);

    let (status, _, _) = observer.classify_output(
        &["Continue with this approach? [Y/n]".to_string()],
        "vm-1",
        "sess-1",
    );
    assert_eq!(status, AgentStatus::WaitingInput);

    let (status, _, _) = observer.classify_output(
        &["azureuser@fleet-exp-1:~/code$ ".to_string()],
        "vm-1",
        "sess-2",
    );
    assert_eq!(status, AgentStatus::Shell);
}

#[test]
fn auth_propagator_rejects_unknown_service() {
    let auth = AuthPropagator::new(PathBuf::from("/bin/true"));
    let results = auth.propagate_all("vm-1", &[String::from("nonexistent")]);
    assert_eq!(results.len(), 1);
    assert!(!results[0].success);
    assert_eq!(results[0].service, "nonexistent");
    assert_eq!(results[0].vm_name, "vm-1");
    assert!(
        results[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Unknown service")
    );
}

#[test]
fn validate_chmod_mode_rejects_unsafe_values() {
    assert!(validate_chmod_mode("600").is_ok());
    assert!(validate_chmod_mode("0644").is_ok());
    assert!(validate_chmod_mode("abc").is_err());
    assert!(validate_chmod_mode("999").is_err());
}

#[test]
fn parse_discovery_output_extracts_session_context() {
    let adopter = SessionAdopter::new(PathBuf::from("/bin/true"));
    let sessions = adopter.parse_discovery_output(
        "vm-01",
        "===SESSION:dev-1===\nCWD:/workspace/myrepo\nCMD:node /usr/local/bin/claude\nBRANCH:feat/login\nREPO:https://github.com/org/myrepo.git\nPR:https://github.com/org/myrepo/pull/42\nLAST_MSG:Implementing authentication\n===DONE===\n",
    );

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.vm_name, "vm-01");
    assert_eq!(session.session_name, "dev-1");
    assert_eq!(session.working_directory, "/workspace/myrepo");
    assert_eq!(session.agent_type, "claude");
    assert_eq!(session.inferred_branch, "feat/login");
    assert_eq!(session.inferred_repo, "https://github.com/org/myrepo.git");
    assert_eq!(session.inferred_pr, "https://github.com/org/myrepo/pull/42");
    assert_eq!(session.inferred_task, "Implementing authentication");
}

#[test]
fn adopt_sessions_creates_running_tasks() {
    let adopter = SessionAdopter::new(PathBuf::from("/bin/true"));
    let mut queue = TaskQueue {
        tasks: Vec::new(),
        persist_path: None,
    };
    let output = "===SESSION:work-1===\nCMD:claude\nREPO:https://github.com/org/repo.git\nLAST_MSG:Working on feature X\n===DONE===\n";
    let sessions = adopter.parse_discovery_output("vm-01", output);

    let adopted = adopter
        .adopt_discovered_sessions("vm-01", sessions, &mut queue, None)
        .unwrap();

    assert_eq!(adopted.len(), 1);
    assert!(adopted[0].task_id.is_some());
    assert_eq!(queue.tasks.len(), 1);
    assert_eq!(queue.tasks[0].status, TaskStatus::Running);
    assert_eq!(queue.tasks[0].assigned_vm.as_deref(), Some("vm-01"));
    assert_eq!(queue.tasks[0].assigned_session.as_deref(), Some("work-1"));
}

#[test]
fn render_snapshot_matches_python_shape_for_empty_fleet() {
    let state = FleetState {
        vms: Vec::new(),
        timestamp: None,
        azlin_path: PathBuf::from("/bin/true"),
        exclude_vms: Vec::new(),
    };
    let mut observer = FleetObserver::new(PathBuf::from("/bin/true"));
    let rendered = render_snapshot(&state, &mut observer).unwrap();
    assert_eq!(
        rendered,
        format!("Fleet Snapshot (0 managed VMs)\n{}", "=".repeat(60))
    );
}

// ── render_scout_report tests (mirrors test_fleet_cli_session_ops.py::TestFormatScoutReport) ──

#[test]
fn render_scout_report_contains_header_and_session_info() {
    let decisions = vec![SessionDecisionRecord {
        vm: "vm-1".to_string(),
        session: "work-1".to_string(),
        status: "idle".to_string(),
        branch: "feat/login".to_string(),
        pr: String::new(),
        action: "wait".to_string(),
        confidence: 0.9,
        reasoning: "Session is idle, nothing to do".to_string(),
        input_text: String::new(),
        error: None,
        project: String::new(),
        objectives: Vec::new(),
    }];
    let output = render_scout_report(&decisions, 2, 1, 1, false);
    assert!(
        output.contains("FLEET SCOUT REPORT"),
        "must contain report header"
    );
    assert!(output.contains("vm-1/work-1"), "must contain vm/session");
    assert!(output.contains("wait"), "must contain action");
    assert!(output.contains("feat/login"), "must contain branch");
    assert!(
        output.contains("Sessions analyzed: 1"),
        "must contain session count"
    );
    assert!(
        output.contains("Adopted sessions: 1"),
        "must contain adopted count"
    );
}

#[test]
fn render_scout_report_with_error_shows_error_text() {
    let decisions = vec![SessionDecisionRecord {
        vm: "vm-2".to_string(),
        session: "sess-2".to_string(),
        status: "running".to_string(),
        branch: String::new(),
        pr: String::new(),
        action: "error".to_string(),
        confidence: 0.0,
        reasoning: String::new(),
        input_text: String::new(),
        error: Some("Connection refused".to_string()),
        project: String::new(),
        objectives: Vec::new(),
    }];
    let output = render_scout_report(&decisions, 1, 1, 0, false);
    assert!(
        output.contains("ERROR"),
        "must show ERROR label on failed session"
    );
    assert!(
        output.contains("Connection refused"),
        "must include error message"
    );
}

#[test]
fn render_scout_report_with_skip_adopt_shows_skipped_label() {
    let output = render_scout_report(&[], 1, 1, 0, true);
    assert!(
        output.contains("Adoption: skipped"),
        "must label adoption as skipped"
    );
}

#[test]
fn render_scout_report_empty_decisions_shows_zero_sessions() {
    let output = render_scout_report(&[], 3, 2, 0, false);
    assert!(
        output.contains("Sessions analyzed: 0"),
        "must show zero when no decisions"
    );
    assert!(
        output.contains("Running VMs: 2"),
        "must show running vm count"
    );
    assert!(
        output.contains("VMs discovered: 3"),
        "must show total vm count"
    );
}

// ── render_advance_report tests (mirrors test_fleet_cli_session_ops.py::TestFormatAdvanceReport) ──

#[test]
fn render_advance_report_contains_header_and_executed_sessions() {
    let decisions = vec![SessionDecisionRecord {
        vm: "vm-1".to_string(),
        session: "work-1".to_string(),
        status: "idle".to_string(),
        branch: String::new(),
        pr: String::new(),
        action: "advance".to_string(),
        confidence: 0.85,
        reasoning: "Ready to advance".to_string(),
        input_text: String::new(),
        error: None,
        project: String::new(),
        objectives: Vec::new(),
    }];
    let executed = vec![SessionExecutionRecord {
        vm: "vm-1".to_string(),
        session: "work-1".to_string(),
        action: "advance".to_string(),
        executed: true,
        error: None,
    }];
    let output = render_advance_report(&decisions, &executed);
    assert!(
        output.contains("FLEET ADVANCE REPORT"),
        "must contain report header"
    );
    assert!(output.contains("vm-1/work-1"), "must contain vm/session");
    assert!(output.contains("[OK]"), "must show OK for executed action");
    assert!(
        output.contains("Sessions analyzed: 1"),
        "must contain session count"
    );
}

#[test]
fn render_advance_report_failed_execution_shows_error() {
    let decisions = vec![SessionDecisionRecord {
        vm: "vm-3".to_string(),
        session: "sess-3".to_string(),
        status: "running".to_string(),
        branch: String::new(),
        pr: String::new(),
        action: "provide_input".to_string(),
        confidence: 0.7,
        reasoning: "Needs input".to_string(),
        input_text: "yes".to_string(),
        error: None,
        project: String::new(),
        objectives: Vec::new(),
    }];
    let executed = vec![SessionExecutionRecord {
        vm: "vm-3".to_string(),
        session: "sess-3".to_string(),
        action: "provide_input".to_string(),
        executed: false,
        error: Some("Timeout sending input".to_string()),
    }];
    let output = render_advance_report(&decisions, &executed);
    assert!(
        output.contains("[ERROR]"),
        "must show ERROR label on failed execution"
    );
    assert!(
        output.contains("Timeout sending input"),
        "must include error message"
    );
}

#[test]
fn render_advance_report_skipped_execution_shows_skipped() {
    let decisions = vec![SessionDecisionRecord {
        vm: "vm-4".to_string(),
        session: "sess-4".to_string(),
        status: "idle".to_string(),
        branch: String::new(),
        pr: String::new(),
        action: "wait".to_string(),
        confidence: 1.0,
        reasoning: "Nothing to do".to_string(),
        input_text: String::new(),
        error: None,
        project: String::new(),
        objectives: Vec::new(),
    }];
    let executed = vec![SessionExecutionRecord {
        vm: "vm-4".to_string(),
        session: "sess-4".to_string(),
        action: "wait".to_string(),
        executed: false,
        error: None,
    }];
    let output = render_advance_report(&decisions, &executed);
    assert!(
        output.contains("[SKIPPED]"),
        "must show SKIPPED label for non-executed action"
    );
}

#[test]
fn render_advance_report_empty_executions_still_shows_header() {
    let output = render_advance_report(&[], &[]);
    assert!(
        output.contains("FLEET ADVANCE REPORT"),
        "header always present"
    );
    assert!(
        output.contains("Sessions analyzed: 0"),
        "zero count for empty input"
    );
}

#[test]
fn render_report_matches_python_shape() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "ubuntu".to_string(),
            status: "Running".to_string(),
            ip: "10.0.0.1".to_string(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: String::new(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("/bin/true"),
        exclude_vms: Vec::new(),
    };
    let queue = TaskQueue {
        tasks: vec![FleetTask::new(
            "Working on feature X",
            "https://github.com/org/repo.git",
            TaskPriority::Medium,
            "claude",
            "auto",
            DEFAULT_MAX_TURNS,
        )],
        persist_path: None,
    };

    let rendered = render_report(&state, &queue);
    assert!(rendered.contains("Fleet Admiral Report — Cycle 0"));
    assert!(rendered.contains("Fleet State"));
    assert!(rendered.contains("Task Queue (1 tasks)"));
    assert!(rendered.contains("Admiral log: 0 actions recorded"));
    assert!(rendered.contains("Stats: 0 actions, 0 successes, 0 failures"));
}

#[test]
fn run_status_returns_exit_error_when_azlin_missing() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let empty_path = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::remove_var("AZLIN_PATH");
        env::set_var("PATH", empty_path.path());
        env::set_var("HOME", home.path());
    }

    let result = run_status();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    let err = result.expect_err("missing azlin should fail");
    assert_eq!(command_error::exit_code(&err), Some(1));
}

#[test]
fn run_status_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        "#!/bin/sh\nif [ \"$1\" = list ] && [ \"$2\" = \"--json\" ]; then\n  echo '[{\"name\":\"vm-1\",\"status\":\"Running\",\"region\":\"westus2\"}]'\nelif [ \"$1\" = connect ]; then\n  printf \"work|||1|||1\\n\";\nelse\n  exit 1\nfi\n",
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::set_var("AZLIN_PATH", &azlin);
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
    }

    let result = run_status();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_snapshot_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
*"tmux list-sessions"*)
  printf '%s\n' "claude-1|||1|||0"
  exit 0
  ;;
*"tmux capture-pane -t 'claude-1'"*)
  printf '%s\n' "Step 5: Implementing auth" "Reading file auth.py" "Running tests"
  exit 0
  ;;
  esac
fi
exit 1
"#,
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_snapshot();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn render_tui_once_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
*"tmux list-sessions"*)
  printf '%s\n' "claude-1|||1|||0"
  exit 0
  ;;
*"tmux capture-pane -t 'claude-1'"*)
  printf '%s\n' "Reading file auth.py" "Running tests"
  exit 0
  ;;
  esac
fi
exit 1
"#,
    );

    let rendered = render_tui_once(&azlin, 30, 50).unwrap();

    assert!(rendered.contains("FLEET DASHBOARD"));
    assert!(rendered.contains("[fleet]"));
    assert!(rendered.contains("q quit"));
    assert!(rendered.contains("vm-1"));
    assert!(rendered.contains("claude-1"));
    assert!(rendered.contains("Running tests"));
}

#[test]
fn collect_observed_fleet_state_reports_progress_per_vm() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"},{"name":"vm-2","status":"Running","region":"eastus","session_name":"vm-2"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
*"PANE_START"*)
  printf '%s\n' \
    '===SESSION:claude-1===' \
    'CWD:/tmp/demo' \
    'CMD:claude' \
    'REPO:https://github.com/org/demo.git' \
    'BRANCH:main' \
    'LAST_MSG:Awaiting operator confirmation' \
    '===DONE==='
  exit 0
  ;;
*"tmux list-sessions"*)
  printf '%s\n' 'claude-1|||1|||0'
  exit 0
  ;;
*"tmux capture-pane"*)
  printf '%s\n' 'Proceed with deploy? [y/n]'
  exit 0
  ;;
  esac
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-2" ]; then
  case "$5" in
*"PANE_START"*)
  printf '%s\n' \
    '===SESSION:copilot-9===' \
    'CWD:/tmp/excluded' \
    'CMD:copilot' \
    'REPO:https://github.com/org/excluded.git' \
    'BRANCH:side-quest' \
    'LAST_MSG:Waiting for operator review' \
    '===DONE==='
  exit 0
  ;;
*"tmux list-sessions"*)
  printf '%s\n' 'copilot-9|||1|||0'
  exit 0
  ;;
*"tmux capture-pane"*)
  printf '%s\n' 'Working outside fleet'
  exit 0
  ;;
  esac
fi
exit 1
"#,
    );

    let mut snapshots = Vec::new();
    let state = collect_observed_fleet_state_with_progress(&azlin, 10, |state, progress| {
        snapshots.push((
            progress.completed_vms,
            progress.total_vms,
            progress.current_vm.clone(),
            state
                .vms
                .iter()
                .map(|vm| format!("{}:{}", vm.name, vm.tmux_sessions.len()))
                .collect::<Vec<_>>(),
        ));
        Ok(())
    })
    .unwrap();

    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots[0].0, 0);
    assert_eq!(snapshots[0].1, 2);
    assert_eq!(snapshots[0].2.as_deref(), Some("vm-1"));
    assert_eq!(snapshots[0].3, vec!["vm-1:0", "vm-2:0"]);
    assert_eq!(snapshots[1].0, 1);
    assert_eq!(snapshots[1].2.as_deref(), Some("vm-2"));
    assert_eq!(snapshots[1].3, vec!["vm-1:1", "vm-2:0"]);
    assert_eq!(snapshots[2].0, 2);
    assert!(snapshots[2].2.is_none());
    assert_eq!(snapshots[2].3, vec!["vm-1:1", "vm-2:1"]);
    assert_eq!(state.vms.len(), 2);
    assert_eq!(state.vms[0].tmux_sessions.len(), 1);
    assert_eq!(state.vms[1].tmux_sessions.len(), 1);
}

#[test]
fn fleet_tui_ui_state_tracks_selection() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![
                TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "first".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
                TmuxSessionInfo {
                    session_name: "claude-2".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "second".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
            ],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState::default();

    ui_state.sync_to_state(&state);
    assert!(ui_state.selection_matches("vm-1", "claude-1"));

    ui_state.move_selection(&state, 1);
    assert!(ui_state.selection_matches("vm-1", "claude-2"));

    ui_state.move_selection(&state, 1);
    assert!(ui_state.selection_matches("vm-1", "claude-1"));
}

#[test]
fn fleet_tui_ui_state_tracks_only_visible_filtered_sessions() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![
                TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "first".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
                TmuxSessionInfo {
                    session_name: "claude-2".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "second".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
            ],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        status_filter: Some(StatusFilter::Waiting),
        ..Default::default()
    };

    ui_state.sync_to_state(&state);
    assert!(ui_state.selection_matches("vm-1", "claude-2"));

    ui_state.move_selection(&state, 1);
    assert!(ui_state.selection_matches("vm-1", "claude-2"));
}

#[test]
fn fleet_tui_ui_state_tracks_only_visible_search_sessions() {
    let state = FleetState {
        vms: vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "first".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "copilot-9".to_string(),
                    vm_name: "vm-2".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "second".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
        ],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        session_search: Some("VM-2".to_string()),
        fleet_subview: FleetSubview::AllSessions,
        ..Default::default()
    };

    ui_state.sync_to_state(&state);
    assert!(ui_state.selection_matches("vm-2", "copilot-9"));

    ui_state.move_selection(&state, 1);
    assert!(ui_state.selection_matches("vm-2", "copilot-9"));
}

#[test]
fn fleet_tui_ui_state_tracks_new_session_vm_selection() {
    let state = FleetState {
        vms: vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "first".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: Vec::new(),
            },
        ],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::NewSession,
        ..Default::default()
    };

    ui_state.sync_to_state(&state);
    assert_eq!(ui_state.new_session_vm.as_deref(), Some("vm-1"));

    ui_state.move_selection(&state, 1);
    assert_eq!(ui_state.new_session_vm.as_deref(), Some("vm-2"));

    ui_state.move_selection(&state, 1);
    assert_eq!(ui_state.new_session_vm.as_deref(), Some("vm-1"));
}

#[test]
fn fleet_tui_ui_state_tracks_project_selection() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };
    let dashboard_path = home.path().join(".amplihack/fleet/dashboard.json");
    fs::create_dir_all(dashboard_path.parent().unwrap()).unwrap();
    fs::write(
        &dashboard_path,
        serde_json::json!([
            {
                "repo_url": "https://github.com/org/repo-a",
                "name": "repo-a",
                "github_identity": "",
                "priority": "medium",
                "notes": "",
                "vms": [],
                "tasks_total": 0,
                "tasks_completed": 0,
                "tasks_failed": 0,
                "tasks_in_progress": 0,
                "prs_created": [],
                "estimated_cost_usd": 0.0,
                "started_at": now_isoformat(),
                "last_activity": null
            },
            {
                "repo_url": "https://github.com/org/repo-b",
                "name": "repo-b",
                "github_identity": "",
                "priority": "high",
                "notes": "",
                "vms": [],
                "tasks_total": 0,
                "tasks_completed": 0,
                "tasks_failed": 0,
                "tasks_in_progress": 0,
                "prs_created": [],
                "estimated_cost_usd": 0.0,
                "started_at": now_isoformat(),
                "last_activity": null
            }
        ])
        .to_string(),
    )
    .unwrap();

    let state = FleetState::new(PathBuf::from("azlin"));
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Projects,
        ..Default::default()
    };

    ui_state.sync_to_state(&state);
    assert_eq!(
        ui_state.selected_project_repo.as_deref(),
        Some("https://github.com/org/repo-a")
    );

    ui_state.move_selection(&state, 1);
    assert_eq!(
        ui_state.selected_project_repo.as_deref(),
        Some("https://github.com/org/repo-b")
    );

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }
}

#[test]
fn projects_tab_sync_surfaces_corrupt_dashboard_load() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let fleet_dir = home.path().join(".amplihack/fleet");
    fs::create_dir_all(&fleet_dir).unwrap();
    fs::write(fleet_dir.join("dashboard.json"), "{broken dashboard").unwrap();

    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };

    let state = FleetState::new(PathBuf::from("azlin"));
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Projects,
        ..Default::default()
    };

    ui_state.sync_to_state(&state);

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(ui_state.selected_project_repo.is_none());
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("failed to parse"))
    );
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("fleet dashboard JSON"))
    );
}

#[test]
fn fleet_tui_ui_state_can_switch_to_all_sessions_subview() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-2".to_string(),
            session_name: "vm-2".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "eastus".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "copilot-1".to_string(),
                vm_name: "vm-2".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "awaiting input".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: vec!["vm-2".to_string()],
    };
    let mut ui_state = FleetTuiUiState::default();

    ui_state.sync_to_state(&state);
    assert!(ui_state.selected.is_none());

    ui_state.cycle_fleet_subview(&state);

    assert_eq!(ui_state.fleet_subview, FleetSubview::AllSessions);
    assert!(ui_state.selection_matches("vm-2", "copilot-1"));
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("Fleet view set to All Sessions.")
    );
}

#[test]
fn render_tui_detail_view_shows_selected_session_and_decision() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 2,
                attached: true,
                agent_status: AgentStatus::WaitingInput,
                last_output: "Need instruction\nWaiting for confirmation".to_string(),
                working_directory: "/tmp/demo".to_string(),
                repo_url: "https://github.com/org/demo.git".to_string(),
                git_branch: "main".to_string(),
                pr_url: "https://github.com/org/demo/pull/42".to_string(),
                task_summary: "Awaiting operator confirmation".to_string(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Detail,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);
    ui_state.last_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "Run cargo test".to_string(),
        reasoning: "Session is waiting on the next command.".to_string(),
        confidence: 0.91,
    });

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("[detail]"));
    assert!(rendered.contains("Session Detail"));
    assert!(rendered.contains("branch: main"));
    assert!(rendered.contains("repo: https://github.com/org/demo.git"));
    assert!(rendered.contains("cwd: /tmp/demo"));
    assert!(rendered.contains("pr: https://github.com/org/demo/pull/42"));
    assert!(rendered.contains("task: Awaiting operator confirmation"));
    assert!(rendered.contains("Need instruction"));
    assert!(rendered.contains("Prepared proposal"));
    assert!(rendered.contains("Run cargo test"));
    assert!(rendered.contains("Detail actions"));
    assert!(rendered.contains("d rerun proposal | e edit | a apply | x skip"));
}

#[test]
fn render_tui_detail_view_prefers_refreshed_tmux_capture() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "summary line".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Detail,
        detail_capture: Some(FleetDetailCapture {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
            output: "full capture line 1\nfull capture line 2".to_string(),
        }),
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("full capture line 1"));
    assert!(rendered.contains("full capture line 2"));
    assert!(!rendered.contains("summary line"));
    assert!(rendered.contains("Detail actions"));
    assert!(rendered.contains("d prepare proposal"));
}

#[test]
fn load_selected_proposal_into_editor_switches_tabs_and_preserves_input() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "waiting".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState::default();
    ui_state.sync_to_state(&state);
    ui_state.last_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "y\n".to_string(),
        reasoning: "Needs confirmation.".to_string(),
        confidence: 0.9,
    });

    ui_state.load_selected_proposal_into_editor();

    assert_eq!(ui_state.tab, FleetTuiTab::Editor);
    let editor = ui_state.editor_decision.as_ref().expect("editor decision");
    assert_eq!(editor.input_text, "y\n");
    assert_eq!(editor.action, SessionAction::SendInput);
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("Loaded proposal into editor"))
    );
}

#[test]
fn load_selected_proposal_into_editor_requires_matching_proposal() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "waiting".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Detail,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    ui_state.load_selected_proposal_into_editor();

    assert_eq!(ui_state.tab, FleetTuiTab::Detail);
    assert!(ui_state.editor_decision.is_none());
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("No prepared proposal for the selected session.")
    );
}

#[test]
fn render_tui_editor_view_shows_prepopulated_decision() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "y\n".to_string(),
        reasoning: "Needs confirmation.".to_string(),
        confidence: 0.9,
    });

    let rendered =
        render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 15, &ui_state).unwrap();

    assert!(rendered.contains("[editor]"));
    assert!(rendered.contains("Action Editor"));
    assert!(rendered.contains("Action: send_input"));
    assert!(rendered.contains("Action choices"));
    assert!(rendered.contains("> send_input"));
    assert!(rendered.contains("  wait"));
    assert!(rendered.contains("Needs confirmation."));
    assert!(rendered.contains("e reload  i focus editor"));
}

#[test]
fn run_tui_edit_input_activates_multiline_editor() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "y\n".to_string(),
        reasoning: "Needs confirmation.".to_string(),
        confidence: 0.9,
    });
    ui_state.enter_multiline_editor("y\n");
    ui_state.editor_active = false;

    let result = run_tui_edit_input(&mut ui_state);

    assert!(result.is_ok());
    assert!(ui_state.editor_active);
    assert_eq!(ui_state.editor_content(), "y\n");
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("Ctrl-S saves"))
    );
}

#[test]
fn start_inline_session_search_prefills_existing_text() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Fleet,
        session_search: Some("copilot".to_string()),
        ..Default::default()
    };

    ui_state.start_inline_session_search();

    let input = ui_state.inline_input.as_ref().expect("inline input");
    assert_eq!(input.mode, FleetTuiInlineInputMode::SearchSessions);
    assert_eq!(input.buffer, "copilot");
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("Searching fleet sessions inline"))
    );
}

#[test]
fn render_tui_editor_view_shows_multiline_editor_focus() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "y\n".to_string(),
        reasoning: "Needs confirmation.".to_string(),
        confidence: 0.9,
    });
    ui_state.enter_multiline_editor("line one\nline two");

    let rendered =
        render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 15, &ui_state).unwrap();

    assert!(rendered.contains("line one"));
    assert!(rendered.contains("line two_"));
    assert!(rendered.contains("Typing mode"));
    assert!(rendered.contains("Enter newline  Ctrl-S save  Esc cancel"));
}

#[test]
fn apply_inline_editor_input_expands_newline_escapes() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: String::new(),
        reasoning: "Needs confirmation.".to_string(),
        confidence: 0.9,
    });

    ui_state.apply_inline_editor_input("line one\\nline two");

    let decision = ui_state.editor_decision.as_ref().expect("editor decision");
    assert_eq!(decision.input_text, "line one\nline two");
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("Updated editor input for vm-1/claude-1.")
    );
}

#[test]
fn apply_inline_session_search_trims_and_clears_empty_value() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Fleet,
        ..Default::default()
    };

    ui_state.apply_inline_session_search("  vm-2  ");
    assert_eq!(ui_state.session_search.as_deref(), Some("vm-2"));
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("Searching fleet sessions for 'vm-2'.")
    );

    ui_state.apply_inline_session_search("   ");
    assert!(ui_state.session_search.is_none());
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("Cleared fleet session search.")
    );
}

#[test]
fn render_tui_frame_shows_logo_by_default() {
    let rendered = render_tui_frame(
        &FleetState::new(PathBuf::from("azlin")),
        15,
        &FleetTuiUiState::default(),
    )
    .unwrap();

    assert!(rendered.contains("A M P L I H A C K   F L E E T"));
    assert!(rendered.contains("|  ☠  |"));
}

#[test]
fn render_tui_frame_hides_logo_when_toggled_off() {
    let ui_state = FleetTuiUiState {
        show_logo: false,
        ..Default::default()
    };
    let rendered =
        render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 15, &ui_state).unwrap();

    assert!(!rendered.contains("A M P L I H A C K   F L E E T"));
    assert!(!rendered.contains("|  ☠  |"));
}

#[test]
fn render_tui_fleet_view_shows_placeholder_for_running_vm_without_sessions() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "empty-vm".to_string(),
            session_name: "empty-vm".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus".to_string(),
            tmux_sessions: Vec::new(),
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };

    let rendered = render_tui_frame(&state, 15, &FleetTuiUiState::default()).unwrap();

    assert!(rendered.contains("empty-vm/(no sessions)"));
    // new cockpit renderer shows "empty" label instead of "no tmux sessions detected"
    assert!(rendered.contains("(no sessions)"));
}

#[test]
fn render_tui_fleet_view_shows_row_status_icons() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![
                TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "actively working".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
                TmuxSessionInfo {
                    session_name: "claude-2".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Idle,
                    last_output: String::new(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
                TmuxSessionInfo {
                    session_name: "claude-3".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Error,
                    last_output: "Traceback: boom".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                },
            ],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };

    let rendered = render_tui_frame(&state, 15, &FleetTuiUiState::default()).unwrap();

    assert!(rendered.contains("◉"));
    assert!(rendered.contains("●"));
    assert!(rendered.contains("✗"));
    assert!(rendered.contains("RUNNING"));
    assert!(rendered.contains("IDLE"));
    assert!(rendered.contains("ERROR"));
}

#[test]
fn render_tui_fleet_view_shows_inline_search_prompt() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: "first".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let ui_state = FleetTuiUiState {
        inline_input: Some(FleetTuiInlineInput {
            mode: FleetTuiInlineInputMode::SearchSessions,
            buffer: "cla".to_string(),
        }),
        ..Default::default()
    };

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("Search sessions > cla_"));
    assert!(rendered.contains("Enter apply | Esc cancel | Backspace delete"));
}

#[test]
fn render_tui_fleet_view_search_matches_vm_name_case_insensitively() {
    let state = FleetState {
        vms: vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "first".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "copilot-9".to_string(),
                    vm_name: "vm-2".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "second".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
        ],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        session_search: Some("VM-2".to_string()),
        fleet_subview: FleetSubview::AllSessions,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("Search: VM-2 (press / to edit, Esc to clear)"));
    assert!(rendered.contains("Selected session: vm-2/copilot-9"));
    assert!(rendered.contains("[search: VM-2]"));
}

#[test]
fn render_tui_frame_shows_refresh_progress() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: "first".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let ui_state = FleetTuiUiState {
        refresh_progress: Some(FleetRefreshProgress {
            completed_vms: 1,
            total_vms: 2,
            current_vm: Some("vm-2".to_string()),
        }),
        ..Default::default()
    };

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("[refresh: 1/2 polling vm-2]"));
}

#[test]
fn decode_dashboard_key_bytes_handles_arrow_sequences() {
    assert_eq!(
        decode_dashboard_key_bytes(&[0x1b, b'[', b'C']),
        Some(DashboardKey::Right)
    );
    assert_eq!(
        decode_dashboard_key_bytes(&[0x1b, b'[', b'D']),
        Some(DashboardKey::Left)
    );
    assert_eq!(
        decode_dashboard_key_bytes(&[0x1b, b'[', b'A']),
        Some(DashboardKey::Up)
    );
    assert_eq!(
        decode_dashboard_key_bytes(&[0x1b, b'[', b'B']),
        Some(DashboardKey::Down)
    );
    assert_eq!(
        decode_dashboard_key_bytes(b"x"),
        Some(DashboardKey::Char('x'))
    );
}

#[test]
fn cycle_tab_helpers_follow_python_tab_order() {
    let mut ui_state = FleetTuiUiState::default();

    assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

    ui_state.cycle_tab_forward();
    assert_eq!(ui_state.tab, FleetTuiTab::Detail);

    ui_state.cycle_tab_forward();
    assert_eq!(ui_state.tab, FleetTuiTab::Editor);

    ui_state.cycle_tab_forward();
    assert_eq!(ui_state.tab, FleetTuiTab::Projects);

    ui_state.cycle_tab_forward();
    assert_eq!(ui_state.tab, FleetTuiTab::NewSession);

    ui_state.cycle_tab_forward();
    assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

    ui_state.cycle_tab_backward();
    assert_eq!(ui_state.tab, FleetTuiTab::NewSession);

    ui_state.cycle_tab_backward();
    assert_eq!(ui_state.tab, FleetTuiTab::Projects);

    ui_state.cycle_tab_backward();
    assert_eq!(ui_state.tab, FleetTuiTab::Editor);

    ui_state.cycle_tab_backward();
    assert_eq!(ui_state.tab, FleetTuiTab::Detail);

    ui_state.cycle_tab_backward();
    assert_eq!(ui_state.tab, FleetTuiTab::Fleet);
}

#[test]
fn toggle_filter_matches_press_again_to_clear_contract() {
    let mut ui_state = FleetTuiUiState::default();

    ui_state.toggle_filter(StatusFilter::Errors);
    assert_eq!(ui_state.status_filter, Some(StatusFilter::Errors));

    ui_state.toggle_filter(StatusFilter::Errors);
    assert_eq!(ui_state.status_filter, None);

    ui_state.toggle_filter(StatusFilter::Waiting);
    assert_eq!(ui_state.status_filter, Some(StatusFilter::Waiting));

    ui_state.toggle_filter(StatusFilter::Active);
    assert_eq!(ui_state.status_filter, Some(StatusFilter::Active));
}

#[test]
fn render_tui_help_overlay_shows_keybinding_reference() {
    let ui_state = FleetTuiUiState {
        show_help: true,
        ..Default::default()
    };
    let rendered =
        render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 15, &ui_state).unwrap();

    assert!(rendered.contains("KEYBINDING HELP"));
    assert!(rendered.contains("1 / f / F"));
    assert!(rendered.contains("5 / n / N"));
    assert!(rendered.contains("Esc / b / B"));
    assert!(rendered.contains("x / X"));
    assert!(rendered.contains("/              Search fleet sessions"));
    assert!(rendered.contains("Filters"));
}

#[test]
fn navigate_back_matches_editor_and_detail_hierarchy() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };

    ui_state.navigate_back();
    assert_eq!(ui_state.tab, FleetTuiTab::Detail);

    ui_state.navigate_back();
    assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

    ui_state.tab = FleetTuiTab::Projects;
    ui_state.navigate_back();
    assert_eq!(ui_state.tab, FleetTuiTab::Fleet);

    ui_state.tab = FleetTuiTab::NewSession;
    ui_state.navigate_back();
    assert_eq!(ui_state.tab, FleetTuiTab::Fleet);
}

#[test]
fn render_tui_new_session_view_shows_running_vms_and_agent() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: Vec::new(),
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::NewSession,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("[new]"));
    assert!(rendered.contains("New Session"));
    assert!(rendered.contains("Agent type: claude"));
    assert!(rendered.contains("> vm-1"));
    assert!(rendered.contains("Enter create"));
}

#[test]
fn render_tui_fleet_view_marks_unmanaged_sessions_in_all_subview() {
    let state = FleetState {
        vms: vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "shipping".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "copilot-1".to_string(),
                    vm_name: "vm-2".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "waiting".to_string(),
                    working_directory: String::new(),
                    repo_url: String::new(),
                    git_branch: String::new(),
                    pr_url: String::new(),
                    task_summary: String::new(),
                }],
            },
        ],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: vec!["vm-2".to_string()],
    };
    let mut ui_state = FleetTuiUiState {
        fleet_subview: FleetSubview::AllSessions,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("[all]"));
    assert!(rendered.contains("All Sessions"));
    assert!(rendered.contains("vm-2"));
    assert!(rendered.contains("unmanaged"));
    assert!(rendered.contains("copilot-1"));
}

#[test]
fn render_tui_fleet_view_shows_managed_subview_header() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: "shipping".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };

    let rendered = render_tui_frame(&state, 15, &FleetTuiUiState::default()).unwrap();

    assert!(rendered.contains("[managed]"));
    assert!(rendered.contains("Managed Sessions"));
    assert!(rendered.contains("vm-1"));
    assert!(rendered.contains("claude-1"));
}

#[test]
fn render_tui_fleet_view_shows_selected_session_preview() {
    let state = FleetState {
        vms: vec![
            VmInfo {
                name: "vm-1".to_string(),
                session_name: "vm-1".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "westus2".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "claude-1".to_string(),
                    vm_name: "vm-1".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::Running,
                    last_output: "build ok".to_string(),
                    working_directory: "/tmp/demo".to_string(),
                    repo_url: "https://github.com/org/demo.git".to_string(),
                    git_branch: "main".to_string(),
                    pr_url: String::new(),
                    task_summary: "Build is passing".to_string(),
                }],
            },
            VmInfo {
                name: "vm-2".to_string(),
                session_name: "vm-2".to_string(),
                os: "linux".to_string(),
                status: "Running".to_string(),
                ip: String::new(),
                region: "eastus".to_string(),
                tmux_sessions: vec![TmuxSessionInfo {
                    session_name: "copilot-1".to_string(),
                    vm_name: "vm-2".to_string(),
                    windows: 1,
                    attached: false,
                    agent_status: AgentStatus::WaitingInput,
                    last_output: "review queued\nneed human ack".to_string(),
                    working_directory: "/tmp/excluded".to_string(),
                    repo_url: "https://github.com/org/excluded.git".to_string(),
                    git_branch: "side-quest".to_string(),
                    pr_url: "https://github.com/org/excluded/pull/7".to_string(),
                    task_summary: "Waiting for operator review".to_string(),
                }],
            },
        ],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: vec!["vm-2".to_string()],
    };
    let mut ui_state = FleetTuiUiState {
        fleet_subview: FleetSubview::AllSessions,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);
    ui_state.move_selection(&state, 1);

    let rendered = render_tui_frame(&state, 18, &ui_state).unwrap();

    assert!(rendered.contains("Selected session: vm-2/copilot-1"));
    assert!(rendered.contains("branch: side-quest"));
    assert!(rendered.contains("repo: https://github.com/org/excluded.git"));
    assert!(rendered.contains("cwd: /tmp/excluded"));
    assert!(rendered.contains("task: Waiting for operator review"));
    assert!(rendered.contains("need human ack"));
}

#[test]
fn skip_selected_proposal_clears_matching_detail_and_editor_state() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "waiting".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let decision = SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "y\n".to_string(),
        reasoning: "Needs confirmation.".to_string(),
        confidence: 0.9,
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        last_decision: Some(decision.clone()),
        editor_decision: Some(decision),
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    ui_state.skip_selected_proposal();

    assert_eq!(ui_state.tab, FleetTuiTab::Detail);
    assert!(ui_state.last_decision.is_none());
    assert!(ui_state.editor_decision.is_none());
    assert_eq!(ui_state.status_message.as_deref(), Some("Skipped."));
}

#[test]
fn render_tui_detail_view_shows_skipped_proposal_notice() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "waiting".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Detail,
        proposal_notice: Some(FleetProposalNotice {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
            title: "Proposal status".to_string(),
            message: "Skipped.".to_string(),
        }),
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("Proposal status"));
    assert!(rendered.contains("Skipped."));
}

#[test]
fn render_tui_detail_view_shows_reasoner_notice_alongside_proposal() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "waiting".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Detail,
        last_decision: Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::Wait,
            input_text: String::new(),
            reasoning: "Waiting for confirmation.".to_string(),
            confidence: 0.6,
        }),
        proposal_notice: Some(FleetProposalNotice {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
            title: "Reasoner status".to_string(),
            message: "Native claude reasoner failed: ANTHROPIC_API_KEY missing.".to_string(),
        }),
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("Prepared proposal"));
    assert!(rendered.contains("Reasoner status"));
    assert!(rendered.contains("ANTHROPIC_API_KEY missing"));
}

#[test]
fn run_tui_dry_run_surfaces_reasoner_api_key_failure_notice() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    let claude = dir.path().join("claude");
    write_executable(&azlin, "#!/bin/sh\nexit 0\n");
    write_executable(
        &claude,
        "#!/bin/sh\nprintf '%s\\n' 'ANTHROPIC_API_KEY missing.' >&2\nexit 1\n",
    );

    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "Proceed with deploy? [y/n]".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: azlin.clone(),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState::default();
    ui_state.sync_to_state(&state);

    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    let previous_reasoner = env::var_os("AMPLIHACK_FLEET_REASONER_BINARY_PATH");
    unsafe {
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
        env::set_var("AMPLIHACK_FLEET_REASONER_BINARY_PATH", &claude);
    }
    let result = run_tui_dry_run(&azlin, &state, &mut ui_state);
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }
    match previous_reasoner {
        Some(value) => unsafe { env::set_var("AMPLIHACK_FLEET_REASONER_BINARY_PATH", value) },
        None => unsafe { env::remove_var("AMPLIHACK_FLEET_REASONER_BINARY_PATH") },
    }

    assert!(result.is_ok());
    assert_eq!(ui_state.tab, FleetTuiTab::Detail);
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.starts_with("Prepared proposal for vm-1/claude-1:"))
    );
    let notice = ui_state
        .proposal_notice
        .as_ref()
        .expect("expected reasoner failure notice");
    assert_eq!(notice.title, "Reasoner status");
    assert!(notice.message.contains("ANTHROPIC_API_KEY missing"));
    assert!(
        notice
            .message
            .contains("Showing a heuristic proposal instead.")
    );
    assert!(ui_state.last_decision.is_some());
}

#[test]
fn render_tui_editor_view_shows_apply_failure_notice() {
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "waiting".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        editor_decision: Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "rm -rf /".to_string(),
            reasoning: "testing dangerous input".to_string(),
            confidence: 0.95,
        }),
        proposal_notice: Some(FleetProposalNotice {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
            title: "Apply status".to_string(),
            message: "Edited apply failed: blocked by dangerous-input policy.".to_string(),
        }),
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    let rendered = render_tui_frame(&state, 15, &ui_state).unwrap();

    assert!(rendered.contains("Action Editor"));
    assert!(rendered.contains("Apply status"));
    assert!(rendered.contains("dangerous-input policy"));
}

#[test]
fn run_tui_apply_sets_persistent_success_notice() {
    let mut ui_state = FleetTuiUiState {
        selected: Some(FleetTuiSelection {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
        }),
        last_decision: Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::Wait,
            input_text: String::new(),
            reasoning: "testing apply".to_string(),
            confidence: 1.0,
        }),
        ..Default::default()
    };

    let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("Applied wait"))
    );
    let notice = ui_state
        .proposal_notice
        .as_ref()
        .expect("apply success should leave a persistent notice");
    assert_eq!(notice.title, "Apply status");
    assert!(notice.message.contains("Applied wait"));
}

#[test]
fn run_tui_apply_edited_returns_to_detail_on_success() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::Wait,
        input_text: String::new(),
        reasoning: "testing apply".to_string(),
        confidence: 1.0,
    });

    let result = run_tui_apply_edited(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert_eq!(ui_state.tab, FleetTuiTab::Detail);
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("Applied edited wait"))
    );
    let notice = ui_state
        .proposal_notice
        .as_ref()
        .expect("edited apply success should leave a persistent notice");
    assert_eq!(notice.title, "Apply status");
    assert!(notice.message.contains("Applied edited wait"));
}

#[test]
fn handle_tui_editor_active_key_escape_discards_and_returns_detail() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "safe".to_string(),
        reasoning: "testing cancel".to_string(),
        confidence: 1.0,
    });
    ui_state.enter_multiline_editor("edited");

    let result = handle_tui_editor_active_key(
        Path::new("azlin"),
        &mut ui_state,
        DashboardKey::Char('\u{1b}'),
    );

    assert!(result.is_ok());
    assert_eq!(ui_state.tab, FleetTuiTab::Detail);
    assert!(!ui_state.editor_active);
    assert!(ui_state.editor_lines.is_empty());
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("Editor changes discarded.")
    );
    let decision = ui_state.editor_decision.as_ref().expect("editor decision");
    assert_eq!(decision.input_text, "safe");
}

#[test]
fn run_tui_apply_reports_missing_prepared_proposal() {
    let mut ui_state = FleetTuiUiState {
        selected: Some(FleetTuiSelection {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
        }),
        ..Default::default()
    };

    let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("No prepared proposal to apply.")
    );
}

#[test]
fn run_tui_apply_reports_missing_selection_even_with_stale_proposal() {
    let mut ui_state = FleetTuiUiState {
        last_decision: Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "y".to_string(),
            reasoning: "stale prepared proposal".to_string(),
            confidence: 0.95,
        }),
        ..Default::default()
    };

    let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("No session selected to apply.")
    );
}

#[test]
fn run_tui_apply_requires_proposal_for_current_selection() {
    let mut ui_state = FleetTuiUiState {
        selected: Some(FleetTuiSelection {
            vm_name: "vm-2".to_string(),
            session_name: "copilot-9".to_string(),
        }),
        last_decision: Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "y".to_string(),
            reasoning: "proposal belongs to another session".to_string(),
            confidence: 0.95,
        }),
        ..Default::default()
    };

    let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("No prepared proposal to apply.")
    );
}

#[test]
fn run_tui_apply_reports_dangerous_input_block() {
    let mut ui_state = FleetTuiUiState {
        selected: Some(FleetTuiSelection {
            vm_name: "vm-1".to_string(),
            session_name: "claude-1".to_string(),
        }),
        last_decision: Some(SessionDecision {
            session_name: "claude-1".to_string(),
            vm_name: "vm-1".to_string(),
            action: SessionAction::SendInput,
            input_text: "rm -rf /".to_string(),
            reasoning: "testing dangerous input".to_string(),
            confidence: 0.95,
        }),
        ..Default::default()
    };

    let result = run_tui_apply(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("dangerous-input policy"))
    );
    let notice = ui_state
        .proposal_notice
        .as_ref()
        .expect("apply failure should leave a persistent notice");
    assert_eq!(notice.title, "Apply status");
    assert!(notice.message.contains("dangerous-input policy"));
}

#[test]
fn run_tui_apply_edited_reports_dangerous_input_block() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "rm -rf /".to_string(),
        reasoning: "testing dangerous input".to_string(),
        confidence: 0.95,
    });

    let result = run_tui_apply_edited(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("dangerous-input policy"))
    );
    let notice = ui_state
        .proposal_notice
        .as_ref()
        .expect("edited apply failure should leave a persistent notice");
    assert_eq!(notice.title, "Apply status");
    assert!(notice.message.contains("dangerous-input policy"));
}

#[test]
fn run_tui_apply_edited_uses_active_multiline_editor_content() {
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    ui_state.editor_decision = Some(SessionDecision {
        session_name: "claude-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "safe".to_string(),
        reasoning: "testing dangerous input".to_string(),
        confidence: 0.95,
    });
    ui_state.enter_multiline_editor("rm -rf /");

    let result = run_tui_apply_edited(Path::new("azlin"), &mut ui_state);

    assert!(result.is_ok());
    assert!(
        ui_state
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("dangerous-input policy"))
    );
    let notice = ui_state
        .proposal_notice
        .as_ref()
        .expect("live editor content should drive apply validation");
    assert!(notice.message.contains("dangerous-input policy"));
}

#[test]
fn run_tui_adopt_selected_session_adds_running_task() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  printf '%s\n' \
"===SESSION:work-1===" \
"CWD:/workspace/repo" \
"CMD:claude" \
"REPO:https://github.com/org/repo.git" \
"BRANCH:feat/login" \
"LAST_MSG:Resume work on auth" \
"===DONE==="
  exit 0
fi
exit 1
"#,
    );

    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "work-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: "Working".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: azlin.clone(),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState::default();
    ui_state.sync_to_state(&state);

    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };
    let result = run_tui_adopt_selected_session(&azlin, &state, &mut ui_state);
    let queue =
        TaskQueue::load(Some(home.path().join(".amplihack/fleet/task_queue.json"))).unwrap();
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
    assert_eq!(queue.tasks.len(), 1);
    assert_eq!(queue.tasks[0].status, TaskStatus::Running);
    assert_eq!(queue.tasks[0].assigned_vm.as_deref(), Some("vm-1"));
    assert_eq!(queue.tasks[0].assigned_session.as_deref(), Some("work-1"));
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("Adopted vm-1/work-1 into the fleet queue.")
    );
}

#[test]
fn run_tui_adopt_selected_session_rejects_duplicate_active_assignment() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "work-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: "Working".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState::default();
    ui_state.sync_to_state(&state);

    let queue_path = home.path().join(".amplihack/fleet/task_queue.json");
    let mut queue = TaskQueue {
        tasks: Vec::new(),
        persist_path: Some(queue_path.clone()),
    };
    let mut task = FleetTask::new(
        "Resume work on auth",
        "https://github.com/org/repo.git",
        TaskPriority::Medium,
        "claude",
        "auto",
        DEFAULT_MAX_TURNS,
    );
    task.status = TaskStatus::Running;
    task.assigned_vm = Some("vm-1".to_string());
    task.assigned_session = Some("work-1".to_string());
    queue.tasks.push(task);
    queue.save().unwrap();

    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };
    let result = run_tui_adopt_selected_session(Path::new("azlin"), &state, &mut ui_state);
    let reloaded = TaskQueue::load(Some(queue_path)).unwrap();
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
    assert_eq!(reloaded.tasks.len(), 1);
    assert_eq!(
        ui_state.status_message.as_deref(),
        Some("vm-1/work-1 is already adopted into the active fleet queue.")
    );
}

#[test]
fn render_tui_projects_view_uses_dashboard_file() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };
    let dashboard_path = home.path().join(".amplihack/fleet/dashboard.json");
    fs::create_dir_all(dashboard_path.parent().unwrap()).unwrap();
    fs::write(
        &dashboard_path,
        serde_json::json!([{
                "repo_url": "https://github.com/org/repo",
                "name": "repo",
                "github_identity": "bot",
                "priority": "high",
                "notes": "Important",
                "vms": ["vm-1"],
                "tasks_total": 3,
                "tasks_completed": 2,
                "tasks_failed": 0,
                "tasks_in_progress": 1,
                "prs_created": ["https://github.com/org/repo/pull/1"],
                "estimated_cost_usd": 1.0,
                "started_at": now_isoformat(),
                "last_activity": now_isoformat()
            }]
        )
        .to_string(),
    )
    .unwrap();

    let state = FleetState::new(PathBuf::from("azlin"));
    let ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Projects,
        ..Default::default()
    };
    let rendered = render_tui_frame(&state, 20, &ui_state).unwrap();

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(rendered.contains("[projects]"));
    assert!(rendered.contains("Fleet Projects (1)"));
    assert!(rendered.contains("https://github.com/org/repo"));
    assert!(rendered.contains("i add project repo"));
}

#[test]
fn run_tui_add_and_remove_project_updates_dashboard_file() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };

    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Projects,
        ..Default::default()
    };

    add_project_from_repo_input(&mut ui_state, "https://github.com/org/new-repo").unwrap();
    let dashboard = FleetDashboardSummary::load(Some(default_dashboard_path())).unwrap();
    assert_eq!(dashboard.projects.len(), 1);
    assert_eq!(dashboard.projects[0].name, "new-repo");
    assert_eq!(
        ui_state.selected_project_repo.as_deref(),
        Some("https://github.com/org/new-repo")
    );

    run_tui_remove_project(&mut ui_state).unwrap();
    let dashboard = FleetDashboardSummary::load(Some(default_dashboard_path())).unwrap();
    assert!(dashboard.projects.is_empty());
    assert!(ui_state.selected_project_repo.is_none());

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }
}

#[test]
fn render_tui_projects_view_shows_inline_add_prompt() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().unwrap();
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("HOME", home.path()) };

    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Projects,
        ..Default::default()
    };
    ui_state.start_inline_project_input();
    ui_state.push_inline_input_char('h');
    ui_state.push_inline_input_char('t');
    ui_state.push_inline_input_char('t');
    ui_state.push_inline_input_char('p');
    ui_state.push_inline_input_char('s');

    let rendered = render_tui_frame(&FleetState::new(PathBuf::from("azlin")), 20, &ui_state)
        .expect("rendered projects view");

    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(rendered.contains("Add project repo > https_"));
    assert!(rendered.contains("Enter add | Esc cancel | Backspace delete"));
}

#[test]
fn run_auth_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "cp" ]; then
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
"mkdir -p '~/.config/gh'"|"chmod 600 '~/.config/gh/hosts.yml'"|"chmod 600 '~/.config/gh/config.yml'")
  exit 0
  ;;
"gh auth status")
  exit 0
  ;;
"az account show --query name -o tsv")
  exit 1
  ;;
  esac
fi
exit 1
"#,
    );

    fs::create_dir_all(home.path().join(".config/gh")).unwrap();
    fs::write(home.path().join(".config/gh/hosts.yml"), "hosts").unwrap();
    fs::write(home.path().join(".config/gh/config.yml"), "config").unwrap();

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_auth("vm-1", &[String::from("github")]);

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_adopt_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  printf '%s\n' \
"===SESSION:work-1===" \
"CWD:/workspace/repo" \
"CMD:claude" \
"REPO:https://github.com/org/repo.git" \
"BRANCH:feat/login" \
"===DONE==="
  exit 0
fi
exit 1
"#,
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_adopt("vm-1", &[]);
    let queue =
        TaskQueue::load(Some(home.path().join(".amplihack/fleet/task_queue.json"))).unwrap();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
    assert_eq!(queue.tasks.len(), 1);
    assert_eq!(queue.tasks[0].status, TaskStatus::Running);
    assert_eq!(queue.tasks[0].assigned_vm.as_deref(), Some("vm-1"));
    assert_eq!(queue.tasks[0].assigned_session.as_deref(), Some("work-1"));
}

#[test]
fn run_report_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
*"tmux list-sessions"*)
  printf '%s\n' "claude-1|||1|||0"
  exit 0
  ;;
*"tmux capture-pane -t 'claude-1'"*)
  printf '%s\n' "Step 5: Implementing auth"
  exit 0
  ;;
  esac
fi
exit 1
"#,
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_report();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn fleet_admiral_reason_emits_lifecycle_and_batch_actions() {
    let temp = tempfile::tempdir().unwrap();
    let mut admiral = FleetAdmiral::new(
        PathBuf::from("/bin/true"),
        TaskQueue {
            tasks: vec![
                FleetTask {
                    id: "done-task".to_string(),
                    prompt: "Finish feature".to_string(),
                    repo_url: "https://github.com/org/repo.git".to_string(),
                    branch: String::new(),
                    priority: TaskPriority::High,
                    status: TaskStatus::Running,
                    agent_command: "claude".to_string(),
                    agent_mode: "auto".to_string(),
                    max_turns: DEFAULT_MAX_TURNS,
                    protected: false,
                    assigned_vm: Some("vm-1".to_string()),
                    assigned_session: Some("session-1".to_string()),
                    assigned_at: Some(now_isoformat()),
                    created_at: now_isoformat(),
                    started_at: Some(now_isoformat()),
                    completed_at: None,
                    result: None,
                    pr_url: None,
                    error: None,
                },
                FleetTask {
                    id: "queued-task".to_string(),
                    prompt: "Implement auth".to_string(),
                    repo_url: "https://github.com/org/repo.git".to_string(),
                    branch: String::new(),
                    priority: TaskPriority::Medium,
                    status: TaskStatus::Queued,
                    agent_command: "claude".to_string(),
                    agent_mode: "auto".to_string(),
                    max_turns: DEFAULT_MAX_TURNS,
                    protected: false,
                    assigned_vm: None,
                    assigned_session: None,
                    assigned_at: None,
                    created_at: now_isoformat(),
                    started_at: None,
                    completed_at: None,
                    result: None,
                    pr_url: None,
                    error: None,
                },
            ],
            persist_path: None,
        },
        Some(temp.path().join("logs")),
    )
    .unwrap();
    admiral.coordination_dir = temp.path().join("coordination");
    admiral.fleet_state.vms = vec![
        VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "ubuntu".to_string(),
            status: "Running".to_string(),
            ip: "10.0.0.1".to_string(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "session-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Completed,
                last_output: "PR created".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        },
        VmInfo {
            name: "vm-2".to_string(),
            session_name: "vm-2".to_string(),
            os: "ubuntu".to_string(),
            status: "Running".to_string(),
            ip: "10.0.0.2".to_string(),
            region: "westus2".to_string(),
            tmux_sessions: vec![],
        },
    ];

    let actions = admiral.reason().unwrap();

    assert!(
        actions
            .iter()
            .any(|action| action.action_type == ActionType::MarkComplete)
    );
    assert!(
        actions
            .iter()
            .any(|action| action.action_type == ActionType::StartAgent)
    );
}

#[test]
fn fleet_admiral_start_agent_updates_task_state_and_log() {
    let temp = tempfile::tempdir().unwrap();
    let azlin = temp.path().join("azlin");
    write_executable(&azlin, "#!/bin/sh\nexit 0\n");

    let task = FleetTask {
        id: "queued-task".to_string(),
        prompt: "Implement auth".to_string(),
        repo_url: String::new(),
        branch: String::new(),
        priority: TaskPriority::Medium,
        status: TaskStatus::Queued,
        agent_command: "claude".to_string(),
        agent_mode: "auto".to_string(),
        max_turns: DEFAULT_MAX_TURNS,
        protected: false,
        assigned_vm: None,
        assigned_session: None,
        assigned_at: None,
        created_at: now_isoformat(),
        started_at: None,
        completed_at: None,
        result: None,
        pr_url: None,
        error: None,
    };
    let mut admiral = FleetAdmiral::new(
        azlin,
        TaskQueue {
            tasks: vec![task.clone()],
            persist_path: Some(temp.path().join("task_queue.json")),
        },
        Some(temp.path().join("logs")),
    )
    .unwrap();
    admiral.coordination_dir = temp.path().join("coordination");

    let action = DirectorAction::new(
        ActionType::StartAgent,
        Some(task),
        Some("vm-1".to_string()),
        Some("fleet-queued-task".to_string()),
        "Batch assign: MEDIUM task",
    );

    let results = admiral.act(&[action]).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1, "Agent started: fleet-queued-task on vm-1");
    let saved = &admiral.task_queue.tasks[0];
    assert_eq!(saved.status, TaskStatus::Running);
    assert_eq!(saved.assigned_vm.as_deref(), Some("vm-1"));
    assert_eq!(saved.assigned_session.as_deref(), Some("fleet-queued-task"));
    assert!(temp.path().join("logs/admiral_log.json").exists());
}

#[test]
fn run_observe_returns_exit_error_when_vm_missing() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
exit 1
"#,
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };
    unsafe { env::set_var("HOME", home.path()) };

    let error = run_observe("missing-vm").unwrap_err();

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert_eq!(command_error::exit_code(&error), Some(1));
}

#[test]
fn run_observe_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "list" ] && [ "$2" = "--json" ]; then
  printf '%s\n' '[{"name":"vm-1","status":"Running","region":"westus2","session_name":"vm-1"}]'
  exit 0
fi
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  case "$5" in
*"tmux list-sessions"*)
  printf '%s\n' "claude-1|||1|||0"
  exit 0
  ;;
*"tmux capture-pane -t 'claude-1'"*)
  printf '%s\n' "Step 5: Implementing auth" "Reading file auth.py" "Running tests"
  exit 0
  ;;
  esac
fi
exit 1
"#,
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_home = env::var_os("HOME");
    unsafe { env::set_var("AZLIN_PATH", &azlin) };
    unsafe { env::set_var("HOME", home.path()) };

    let result = run_observe("vm-1");

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_watch_succeeds_with_stubbed_azlin() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        "#!/bin/sh\nif [ \"$1\" = connect ]; then\n  printf \"agent output line 1\\nagent output line 2\\n\";\nelse\n  exit 1\nfi\n",
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::set_var("AZLIN_PATH", &azlin);
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
    }

    let result = run_watch("test-vm", "session-1", 30);

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_tui_refresh_detail_capture_reads_fresh_tmux_output() {
    let dir = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        "#!/bin/sh\nif [ \"$1\" = connect ]; then\n  printf \"fresh detail line 1\\nfresh detail line 2\\n\";\nelse\n  exit 1\nfi\n",
    );
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "claude-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::WaitingInput,
                last_output: "summary line".to_string(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: azlin.clone(),
        exclude_vms: Vec::new(),
    };
    let mut ui_state = FleetTuiUiState {
        tab: FleetTuiTab::Detail,
        ..Default::default()
    };
    ui_state.sync_to_state(&state);

    run_tui_refresh_detail_capture(&azlin, &state, &mut ui_state, 10).unwrap();

    let capture = ui_state
        .detail_capture
        .as_ref()
        .expect("expected refreshed detail capture");
    assert_eq!(capture.vm_name, "vm-1");
    assert_eq!(capture.session_name, "claude-1");
    assert!(capture.output.contains("fresh detail line 1"));
    assert!(capture.output.contains("fresh detail line 2"));
}

#[test]
fn run_watch_failure_is_nonfatal() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        "#!/bin/sh\nprintf 'connection refused' >&2\nexit 1\n",
    );

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::set_var("AZLIN_PATH", &azlin);
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
    }

    let result = run_watch("test-vm", "session-1", 30);

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_watch_timeout_is_nonfatal() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(&azlin, "#!/bin/sh\nsleep 1\n");

    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    let previous_home = env::var_os("HOME");
    unsafe {
        env::set_var("AZLIN_PATH", &azlin);
        env::set_var("PATH", dir.path());
        env::set_var("HOME", home.path());
    }

    let result = run_watch_with_timeout("test-vm", "session-1", 30, Duration::from_secs(0));

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }
    match previous_home {
        Some(value) => unsafe { env::set_var("HOME", value) },
        None => unsafe { env::remove_var("HOME") },
    }

    assert!(result.is_ok());
}

#[test]
fn run_watch_rejects_invalid_vm_name() {
    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let empty_path = tempfile::tempdir().unwrap();
    let previous_azlin = env::var_os("AZLIN_PATH");
    let previous_path = env::var_os("PATH");
    unsafe {
        env::remove_var("AZLIN_PATH");
        env::set_var("PATH", empty_path.path());
    }

    let err = run_watch("bad vm!@#", "session-1", 30).expect_err("invalid VM should fail");

    match previous_azlin {
        Some(value) => unsafe { env::set_var("AZLIN_PATH", value) },
        None => unsafe { env::remove_var("AZLIN_PATH") },
    }
    match previous_path {
        Some(value) => unsafe { env::set_var("PATH", value) },
        None => unsafe { env::remove_var("PATH") },
    }

    assert_eq!(command_error::exit_code(&err), None);
    assert!(err.to_string().contains("Invalid VM name"));
}

// ── T5: Per-session LRU capture cache ─────────────────────────────────

#[test]
fn capture_cache_insert_and_hit() {
    let ui = FleetTuiUiState::default();
    assert!(
        ui.get_capture("vm-1", "sess-1").is_none(),
        "empty cache should miss"
    );
    ui.put_capture("vm-1", "sess-1", "line1\nline2".to_string());
    let got = ui
        .get_capture("vm-1", "sess-1")
        .expect("cache should hit after insert");
    assert!(got.contains("line1"), "cached output should match");
}

#[test]
fn capture_cache_miss_on_different_key() {
    let ui = FleetTuiUiState::default();
    ui.put_capture("vm-1", "sess-1", "output-a".to_string());
    assert!(
        ui.get_capture("vm-1", "sess-2").is_none(),
        "different session should miss"
    );
    assert!(
        ui.get_capture("vm-2", "sess-1").is_none(),
        "different vm should miss"
    );
}

#[test]
fn capture_cache_evicts_lru_when_at_capacity() {
    let ui = FleetTuiUiState::default();
    // Fill cache to capacity (64 entries).
    for i in 0..CAPTURE_CACHE_CAPACITY {
        ui.put_capture("vm", &format!("s{i}"), format!("out{i}"));
    }
    // Touch the first entry so it becomes most recently used.
    assert!(
        ui.get_capture("vm", "s0").is_some(),
        "s0 should still be cached"
    );
    // Insert one more — s1 is now least-recently-used, so it should be evicted.
    ui.put_capture("vm", "s_new", "new".to_string());
    assert!(
        ui.get_capture("vm", "s1").is_none(),
        "s1 should be evicted after capacity overflow"
    );
    assert!(
        ui.get_capture("vm", "s_new").is_some(),
        "new entry should exist"
    );
}

#[test]
fn capture_cache_compat_detail_capture_field_falls_through() {
    // Tests that pre-existing `detail_capture` field still works.
    let ui = FleetTuiUiState {
        detail_capture: Some(FleetDetailCapture {
            vm_name: "vm-1".to_string(),
            session_name: "sess-1".to_string(),
            output: "compat-output".to_string(),
        }),
        ..Default::default()
    };
    let got = ui
        .get_capture("vm-1", "sess-1")
        .expect("compat field should fall through");
    assert_eq!(got, "compat-output");
}

// ── T4: Background channel helpers ────────────────────────────────────

#[test]
fn drain_bg_messages_handles_session_created() {
    let (tx, rx) = mpsc::channel::<BackgroundMessage>();
    let mut ui = FleetTuiUiState {
        bg_rx: Some(Arc::new(Mutex::new(rx))),
        ..Default::default()
    };

    tx.send(BackgroundMessage::SessionCreated {
        message: "Created session 'claude-1234' on vm-1 running claude.".to_string(),
    })
    .unwrap();

    ui.drain_bg_messages();

    assert!(!ui.create_session_pending, "pending flag cleared");
    assert_eq!(
        ui.tab,
        FleetTuiTab::Fleet,
        "tab switched to fleet on created"
    );
    assert!(
        ui.status_message
            .as_deref()
            .unwrap_or("")
            .contains("Created session"),
        "status message set"
    );
}

#[test]
fn drain_bg_messages_handles_slow_capture_update() {
    let (tx, rx) = mpsc::channel::<BackgroundMessage>();
    let mut ui = FleetTuiUiState {
        bg_rx: Some(Arc::new(Mutex::new(rx))),
        ..Default::default()
    };

    tx.send(BackgroundMessage::SlowCaptureUpdate {
        vm_name: "vm-bg".to_string(),
        session_name: "sess-bg".to_string(),
        output: "bg capture line".to_string(),
    })
    .unwrap();

    ui.drain_bg_messages();

    let got = ui
        .get_capture("vm-bg", "sess-bg")
        .expect("cache should be populated");
    assert!(got.contains("bg capture line"));
}

#[test]
fn drain_bg_messages_handles_background_error() {
    let (tx, rx) = mpsc::channel::<BackgroundMessage>();
    let mut ui = FleetTuiUiState {
        bg_rx: Some(Arc::new(Mutex::new(rx))),
        ..Default::default()
    };

    tx.send(BackgroundMessage::Error("disk full".to_string()))
        .unwrap();
    ui.drain_bg_messages();

    assert!(
        ui.status_message
            .as_deref()
            .unwrap_or("")
            .contains("[bg error]"),
        "error message surfaced"
    );
}

#[test]
fn drain_bg_messages_noop_without_channel() {
    let mut ui = FleetTuiUiState::default();
    // Should not panic when bg_rx is None.
    ui.drain_bg_messages();
}

// ── T1: Session creation dispatch ─────────────────────────────────────

#[test]
fn run_tui_create_session_dispatches_to_background_when_channel_available() {
    let (cmd_tx, cmd_rx) = mpsc::channel::<BackgroundCommand>();
    let (_msg_tx, msg_rx) = mpsc::channel::<BackgroundMessage>();
    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: Vec::new(),
        }],
        timestamp: None,
        azlin_path: PathBuf::from("azlin"),
        exclude_vms: Vec::new(),
    };
    let mut ui = FleetTuiUiState {
        tab: FleetTuiTab::NewSession,
        bg_tx: Some(cmd_tx),
        bg_rx: Some(Arc::new(Mutex::new(msg_rx))),
        ..Default::default()
    };
    ui.sync_to_state(&state);
    // Ensure a VM is selected.
    ui.new_session_vm = Some("vm-1".to_string());

    run_tui_create_session(&PathBuf::from("azlin"), &mut ui).unwrap();

    // Should be pending.
    assert!(ui.create_session_pending, "pending flag set");
    assert!(
        ui.status_message
            .as_deref()
            .unwrap_or("")
            .contains("background"),
        "status mentions background"
    );

    // Background command should have been sent.
    let cmd = cmd_rx
        .try_recv()
        .expect("CreateSession command should have been sent");
    assert!(
        matches!(cmd, BackgroundCommand::CreateSession { .. }),
        "expected CreateSession command"
    );
}

#[test]
fn run_tui_create_session_falls_back_to_sync_without_channel() {
    let dir = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    // Azlin that fails (no connect support).
    write_executable(&azlin, "#!/bin/sh\nprintf 'no connection' >&2\nexit 1\n");
    let mut ui = FleetTuiUiState {
        tab: FleetTuiTab::NewSession,
        ..Default::default()
    };
    ui.new_session_vm = Some("vm-1".to_string());

    run_tui_create_session(&azlin, &mut ui).unwrap();

    // Should not be pending (synchronous fallback).
    assert!(
        !ui.create_session_pending,
        "pending flag not set in sync mode"
    );
    assert_eq!(
        ui.tab,
        FleetTuiTab::Fleet,
        "tab switched to fleet on completion"
    );
}

// ── T2: Adoption refresh trigger ──────────────────────────────────────

#[test]
fn run_tui_adopt_triggers_force_refresh_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let azlin = dir.path().join("azlin");
    write_executable(
        &azlin,
        r#"#!/bin/sh
if [ "$1" = "connect" ] && [ "$2" = "vm-1" ]; then
  printf '%s\n' \
"===SESSION:work-1===" \
"CWD:/workspace/repo" \
"CMD:claude" \
"REPO:https://github.com/org/repo.git" \
"BRANCH:feat/login" \
"LAST_MSG:Resume work on auth" \
"===DONE==="
  exit 0
fi
exit 1
"#,
    );

    let (cmd_tx, cmd_rx) = mpsc::channel::<BackgroundCommand>();
    let (_msg_tx, msg_rx) = mpsc::channel::<BackgroundMessage>();

    let state = FleetState {
        vms: vec![VmInfo {
            name: "vm-1".to_string(),
            session_name: "vm-1".to_string(),
            os: "linux".to_string(),
            status: "Running".to_string(),
            ip: String::new(),
            region: "westus2".to_string(),
            tmux_sessions: vec![TmuxSessionInfo {
                session_name: "work-1".to_string(),
                vm_name: "vm-1".to_string(),
                windows: 1,
                attached: false,
                agent_status: AgentStatus::Running,
                last_output: String::new(),
                working_directory: String::new(),
                repo_url: String::new(),
                git_branch: String::new(),
                pr_url: String::new(),
                task_summary: String::new(),
            }],
        }],
        timestamp: None,
        azlin_path: azlin.clone(),
        exclude_vms: Vec::new(),
    };

    let _guard = home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev_home = env::var_os("HOME");
    let prev_queue = env::var_os("AMPLIHACK_QUEUE_PATH");
    unsafe {
        env::set_var("HOME", home.path());
        env::remove_var("AMPLIHACK_QUEUE_PATH");
    }

    let mut ui = FleetTuiUiState {
        fleet_subview: FleetSubview::AllSessions,
        bg_tx: Some(cmd_tx),
        bg_rx: Some(Arc::new(Mutex::new(msg_rx))),
        ..Default::default()
    };
    ui.sync_to_state(&state);

    let result = run_tui_adopt_selected_session(&azlin, &state, &mut ui);

    unsafe {
        match prev_home {
            Some(v) => env::set_var("HOME", v),
            None => env::remove_var("HOME"),
        }
        match prev_queue {
            Some(v) => env::set_var("AMPLIHACK_QUEUE_PATH", v),
            None => env::remove_var("AMPLIHACK_QUEUE_PATH"),
        }
    }

    result.unwrap();

    // Verify ForceStatusRefresh was sent.
    let cmd = cmd_rx
        .try_recv()
        .expect("ForceStatusRefresh should have been sent after adopt");
    assert!(
        matches!(cmd, BackgroundCommand::ForceStatusRefresh),
        "expected ForceStatusRefresh, got {cmd:?}"
    );
}

// ── T6: Project management mode ───────────────────────────────────────

#[test]
fn project_mode_default_is_list() {
    let ui = FleetTuiUiState::default();
    assert_eq!(ui.project_mode, ProjectManagementMode::List);
}

#[test]
fn enter_project_remove_mode_requires_selection() {
    let mut ui = FleetTuiUiState::default();
    // No selected project — should not enter Remove mode.
    ui.enter_project_remove_mode();
    assert_eq!(ui.project_mode, ProjectManagementMode::List);
    assert!(ui.status_message.is_some(), "error message set");
}

#[test]
fn enter_project_remove_mode_transitions_when_selected() {
    let mut ui = FleetTuiUiState {
        selected_project_repo: Some("https://github.com/org/repo".to_string()),
        ..Default::default()
    };
    ui.enter_project_remove_mode();
    assert_eq!(ui.project_mode, ProjectManagementMode::Remove);
}

#[test]
fn cancel_project_mode_returns_to_list() {
    let mut ui = FleetTuiUiState {
        project_mode: ProjectManagementMode::Remove,
        selected_project_repo: Some("https://github.com/org/repo".to_string()),
        ..Default::default()
    };
    ui.cancel_project_mode();
    assert_eq!(ui.project_mode, ProjectManagementMode::List);
}

#[test]
fn confirm_project_remove_returns_to_list() {
    let mut ui = FleetTuiUiState {
        project_mode: ProjectManagementMode::Remove,
        selected_project_repo: Some("https://github.com/org/repo".to_string()),
        ..Default::default()
    };
    ui.confirm_project_remove();
    assert_eq!(ui.project_mode, ProjectManagementMode::List);
}

// ── T3: Multiline editor ──────────────────────────────────────────────

#[test]
fn enter_multiline_editor_populates_lines() {
    let mut ui = FleetTuiUiState::default();
    assert!(!ui.editor_active);
    ui.enter_multiline_editor("line one\nline two");
    assert!(ui.editor_active, "editor should be active after entry");
    assert_eq!(ui.editor_lines, vec!["line one", "line two"]);
    assert_eq!(ui.editor_cursor_row, 1, "cursor on last row");
}

#[test]
fn enter_multiline_editor_with_empty_string_creates_one_line() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("");
    assert!(ui.editor_active);
    assert_eq!(ui.editor_lines.len(), 1);
    assert_eq!(ui.editor_lines[0], "");
}

#[test]
fn editor_insert_char_appends_to_current_line() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("hel");
    ui.editor_insert_char('l');
    ui.editor_insert_char('o');
    assert_eq!(ui.editor_content(), "hello");
}

#[test]
fn editor_insert_newline_adds_line() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("line1");
    ui.editor_insert_char('\n');
    assert_eq!(ui.editor_lines.len(), 2);
    assert_eq!(ui.editor_cursor_row, 1);
}

#[test]
fn editor_backspace_removes_char() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("hello");
    ui.editor_backspace();
    assert_eq!(ui.editor_content(), "hell");
}

#[test]
fn editor_backspace_merges_empty_line_with_previous() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("line1\n");
    // cursor is on line 1 (empty), backspace should merge with line 0.
    assert_eq!(ui.editor_cursor_row, 1);
    ui.editor_backspace();
    assert_eq!(ui.editor_lines.len(), 1, "lines merged");
    assert_eq!(ui.editor_cursor_row, 0);
}

#[test]
fn editor_cursor_navigation() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("line1\nline2\nline3");
    assert_eq!(ui.editor_cursor_row, 2);
    ui.editor_move_up();
    assert_eq!(ui.editor_cursor_row, 1);
    ui.editor_move_up();
    assert_eq!(ui.editor_cursor_row, 0);
    // Cannot go above row 0.
    ui.editor_move_up();
    assert_eq!(ui.editor_cursor_row, 0);
    ui.editor_move_down();
    assert_eq!(ui.editor_cursor_row, 1);
}

#[test]
fn editor_save_extracts_content() {
    let mut ui = FleetTuiUiState {
        tab: FleetTuiTab::Editor,
        ..Default::default()
    };
    // Set up a decision so apply_inline_editor_input has something to update.
    ui.editor_decision = Some(SessionDecision {
        session_name: "sess-1".to_string(),
        vm_name: "vm-1".to_string(),
        action: SessionAction::SendInput,
        input_text: "old text".to_string(),
        reasoning: "test".to_string(),
        confidence: 0.9,
    });
    ui.enter_multiline_editor("new\ncontent");
    ui.editor_save();
    assert!(!ui.editor_active, "editor deactivated after save");
    assert!(ui.editor_lines.is_empty(), "lines cleared after save");
    // The saved content should have been applied to editor_decision.
    let saved_input = ui.editor_decision.as_ref().map(|d| d.input_text.as_str());
    assert_eq!(saved_input, Some("new\ncontent"), "content saved");
}

#[test]
fn editor_discard_clears_without_saving() {
    let mut ui = FleetTuiUiState::default();
    ui.enter_multiline_editor("discard me");
    ui.editor_discard();
    assert!(!ui.editor_active, "editor deactivated after discard");
    assert!(ui.editor_lines.is_empty(), "lines cleared after discard");
}
