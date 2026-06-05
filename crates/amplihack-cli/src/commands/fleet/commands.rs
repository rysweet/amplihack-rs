use super::*;

pub fn run_fleet(args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        return run_tui(DEFAULT_DASHBOARD_REFRESH_SECONDS, DEFAULT_CAPTURE_LINES);
    }

    match parse_native_fleet_command(&args) {
        Some(NativeFleetCommand::Setup) => run_setup(),
        Some(NativeFleetCommand::Status) => run_status(),
        Some(NativeFleetCommand::Snapshot) => run_snapshot(),
        Some(NativeFleetCommand::Tui {
            interval,
            capture_lines,
        }) => run_tui(interval, capture_lines),
        Some(NativeFleetCommand::DryRun {
            vm,
            priorities,
            backend,
        }) => run_dry_run(&vm, &priorities, &backend),
        Some(NativeFleetCommand::Scout {
            vm,
            session_target,
            skip_adopt,
            incremental,
            save_path,
        }) => run_scout(
            vm.as_deref(),
            session_target.as_deref(),
            skip_adopt,
            incremental,
            save_path.as_deref(),
        ),
        Some(NativeFleetCommand::Advance {
            vm,
            session_target,
            force,
            save_path,
        }) => run_advance(
            vm.as_deref(),
            session_target.as_deref(),
            force,
            save_path.as_deref(),
        ),
        Some(NativeFleetCommand::Start {
            max_cycles,
            interval,
            adopt,
            stuck_threshold,
            max_agents_per_vm,
            capture_lines,
        }) => run_start(
            max_cycles,
            interval,
            adopt,
            stuck_threshold,
            max_agents_per_vm,
            capture_lines,
        ),
        Some(NativeFleetCommand::RunOnce) => run_run_once(),
        Some(NativeFleetCommand::Auth { vm_name, services }) => run_auth(&vm_name, &services),
        Some(NativeFleetCommand::Adopt { vm_name, sessions }) => run_adopt(&vm_name, &sessions),
        Some(NativeFleetCommand::Observe { vm_name }) => run_observe(&vm_name),
        Some(NativeFleetCommand::Report) => run_report(),
        Some(NativeFleetCommand::Queue) => run_queue(),
        Some(NativeFleetCommand::Dashboard) => run_dashboard(),
        Some(NativeFleetCommand::Graph) => run_graph(),
        Some(NativeFleetCommand::CopilotStatus) => run_copilot_status(),
        Some(NativeFleetCommand::CopilotLog { tail }) => run_copilot_log(tail),
        Some(NativeFleetCommand::Project { command }) => run_project(command),
        Some(NativeFleetCommand::Watch {
            vm_name,
            session_name,
            lines,
        }) => run_watch(&vm_name, &session_name, lines),
        Some(NativeFleetCommand::AddTask {
            prompt,
            repo,
            priority,
            agent,
            mode,
            max_turns,
            protected,
        }) => run_add_task(&prompt, &repo, priority, agent, mode, max_turns, protected),
        Some(NativeFleetCommand::VerifyVersion {
            hosts,
            host_file,
            expected_version,
            format,
        }) => run_verify_version(
            &hosts,
            host_file.as_deref(),
            expected_version.as_deref(),
            format,
        ),
        Some(NativeFleetCommand::Update {
            hosts,
            host_file,
            version,
            dry_run,
            verify,
            format,
        }) => run_update_hosts(
            &hosts,
            host_file.as_deref(),
            version.as_deref(),
            dry_run,
            verify,
            format,
        ),
        None if args.iter().any(|arg| arg == "--help" || arg == "-h") => {
            let mut command = NativeFleetCli::command();
            command.print_help()?;
            println!();
            Ok(())
        }
        None => bail!("unsupported or invalid `amplihack fleet` subcommand"),
    }
}

pub(super) fn parse_native_fleet_command(args: &[String]) -> Option<NativeFleetCommand> {
    let argv = iter::once("fleet").chain(args.iter().map(String::as_str));
    NativeFleetCli::try_parse_from(argv)
        .ok()
        .map(|cli| cli.command)
}

pub(super) fn run_setup() -> Result<()> {
    println!("Fleet setup — checking prerequisites...");
    let mut all_ok = true;

    let azlin_path = match get_azlin_path() {
        Ok(path) => {
            println!("  azlin: {}", path.display());
            Some(path)
        }
        Err(_) => {
            eprintln!("  azlin: NOT FOUND");
            eprintln!("    Install azlin and set AZLIN_PATH, or add it to PATH.");
            eprintln!("    See: https://github.com/rysweet/azlin");
            all_ok = false;
            None
        }
    };

    if let Some(path) = azlin_path {
        let mut version_cmd = Command::new(&path);
        version_cmd.arg("--version");
        match run_output_with_timeout(version_cmd, AZLIN_VERSION_TIMEOUT) {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                let version = version.trim();
                let version = if version.is_empty() {
                    "unknown"
                } else {
                    version
                };
                println!("  azlin version: {version}");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("  azlin: found but --version failed ({})", stderr.trim());
            }
            Err(err) => {
                eprintln!("  azlin: found but verification failed — {err}");
            }
        }
    }

    if let Some(path) = find_binary("az") {
        println!("  az CLI: {}", path.display());
    } else {
        println!("  az CLI: not found (optional — needed for VM provisioning)");
    }

    if all_ok {
        println!();
        println!("All prerequisites found.");
        Ok(())
    } else {
        eprintln!();
        eprintln!("Missing prerequisites — see errors above.");
        Err(exit_error(1))
    }
}

pub(super) fn run_status() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let mut state = FleetState::new(azlin);
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();
    println!("{}", state.summary());
    Ok(())
}

pub(super) fn run_snapshot() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let mut state = FleetState::new(azlin.clone());
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();

    let mut observer = FleetObserver::new(azlin);
    println!("{}", render_snapshot(&state, &mut observer)?);
    Ok(())
}

pub(super) fn run_dry_run(vm_names: &[String], priorities: &str, backend: &str) -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let sessions = discover_dry_run_sessions(&azlin, vm_names)?;
    if sessions.is_empty() {
        return Ok(());
    }

    let backend = NativeReasonerBackend::detect(backend)?;
    let mut reasoner = FleetSessionReasoner::new(azlin, backend);
    println!();
    println!("Fleet Admiral Dry Run -- {} sessions", sessions.len());
    println!("Backend: {}", reasoner.backend_label());
    println!(
        "Priorities: {}",
        if priorities.is_empty() {
            "(none specified)"
        } else {
            priorities
        }
    );
    println!();

    for session in sessions {
        let _ = reasoner.reason_about_session(
            &session.vm_name,
            &session.session_name,
            "",
            priorities,
            None,
        )?;
    }

    println!("\n{}", reasoner.dry_run_report());
    Ok(())
}

#[derive(Debug, Serialize)]
struct FleetVersionResult {
    host: String,
    status: String,
    version: Option<String>,
    expected: Option<String>,
    message: String,
}

fn resolve_hosts(hosts: &[String], host_file: Option<&Path>) -> Result<Vec<String>> {
    let mut resolved = Vec::new();
    for host in hosts {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            resolved.push(trimmed.to_string());
        }
    }
    if let Some(path) = host_file {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read host file {}", path.display()))?;
        for line in content.lines() {
            let host = line.split('#').next().unwrap_or("").trim();
            if !host.is_empty() {
                resolved.push(host.to_string());
            }
        }
    }
    resolved.sort();
    resolved.dedup();
    if resolved.is_empty() {
        bail!("fleet command requires at least one explicit host via --hosts or --host-file");
    }
    for host in &resolved {
        validate_fleet_host(host)?;
    }
    Ok(resolved)
}

fn validate_fleet_host(host: &str) -> Result<()> {
    let valid = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '@' | ':'));
    if valid {
        Ok(())
    } else {
        bail!("invalid fleet host target '{host}'")
    }
}

fn extract_amplihack_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|token| token.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .map(|token| token.trim_start_matches('v').to_string())
}

fn verify_host_version(host: &str, expected: Option<&str>) -> FleetVersionResult {
    let output = Command::new("ssh")
        .arg(host)
        .arg("amplihack")
        .arg("--version")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let version = extract_amplihack_version(&stdout);
            let matches_expected = expected
                .zip(version.as_deref())
                .is_none_or(|(expected, actual)| actual == expected.trim_start_matches('v'));
            let status = if matches_expected { "ok" } else { "mismatch" };
            let message = match (expected, version.as_deref()) {
                (Some(expected), Some(actual)) if actual != expected.trim_start_matches('v') => {
                    format!("expected {expected}, found {actual}")
                }
                (_, Some(actual)) => format!("amplihack {actual}"),
                _ => "amplihack version output did not contain a parseable version".to_string(),
            };
            FleetVersionResult {
                host: host.to_string(),
                status: status.to_string(),
                version,
                expected: expected.map(str::to_string),
                message,
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            FleetVersionResult {
                host: host.to_string(),
                status: "unreachable".to_string(),
                version: None,
                expected: expected.map(str::to_string),
                message: stderr
                    .lines()
                    .next()
                    .unwrap_or("ssh command failed")
                    .to_string(),
            }
        }
        Err(err) => FleetVersionResult {
            host: host.to_string(),
            status: "error".to_string(),
            version: None,
            expected: expected.map(str::to_string),
            message: err.to_string(),
        },
    }
}

fn render_fleet_results(
    results: &[FleetVersionResult],
    format: NativeFleetOutputFormat,
) -> Result<()> {
    match format {
        NativeFleetOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(results)?);
        }
        NativeFleetOutputFormat::Table => {
            println!("{:<32} {:<12} {:<12} MESSAGE", "HOST", "STATUS", "VERSION");
            for result in results {
                println!(
                    "{:<32} {:<12} {:<12} {}",
                    result.host,
                    result.status,
                    result.version.as_deref().unwrap_or("-"),
                    result.message
                );
            }
        }
    }
    Ok(())
}

pub(super) fn run_verify_version(
    hosts: &[String],
    host_file: Option<&Path>,
    expected: Option<&str>,
    format: NativeFleetOutputFormat,
) -> Result<()> {
    let hosts = resolve_hosts(hosts, host_file)?;
    let results: Vec<_> = hosts
        .iter()
        .map(|host| verify_host_version(host, expected))
        .collect();
    let failed = results.iter().any(|result| result.status != "ok");
    render_fleet_results(&results, format)?;
    if failed { Err(exit_error(1)) } else { Ok(()) }
}

pub(super) fn run_update_hosts(
    hosts: &[String],
    host_file: Option<&Path>,
    version: Option<&str>,
    dry_run: bool,
    verify: bool,
    format: NativeFleetOutputFormat,
) -> Result<()> {
    let hosts = resolve_hosts(hosts, host_file)?;
    let expected = version.map(str::trim).filter(|value| !value.is_empty());
    let mut results = Vec::new();
    for host in hosts {
        if dry_run {
            results.push(FleetVersionResult {
                host,
                status: "planned".to_string(),
                version: None,
                expected: expected.map(str::to_string),
                message: "would run `amplihack update` on explicit host target".to_string(),
            });
            continue;
        }

        let mut command = Command::new("ssh");
        command.arg(&host).arg("amplihack").arg("update");
        if expected.is_some() {
            command.arg("--skip-install");
        }
        match command.output() {
            Ok(out) if out.status.success() => {
                if verify {
                    results.push(verify_host_version(&host, expected));
                } else {
                    results.push(FleetVersionResult {
                        host,
                        status: "updated".to_string(),
                        version: None,
                        expected: expected.map(str::to_string),
                        message: "amplihack update completed".to_string(),
                    });
                }
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                results.push(FleetVersionResult {
                    host,
                    status: "failed".to_string(),
                    version: None,
                    expected: expected.map(str::to_string),
                    message: stderr
                        .lines()
                        .next()
                        .unwrap_or("amplihack update failed")
                        .to_string(),
                });
            }
            Err(err) => results.push(FleetVersionResult {
                host,
                status: "error".to_string(),
                version: None,
                expected: expected.map(str::to_string),
                message: err.to_string(),
            }),
        }
    }
    let failed = results.iter().any(|result| {
        matches!(
            result.status.as_str(),
            "failed" | "error" | "mismatch" | "unreachable"
        )
    });
    render_fleet_results(&results, format)?;
    if failed { Err(exit_error(1)) } else { Ok(()) }
}
