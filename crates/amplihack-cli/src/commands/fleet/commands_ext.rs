use super::*;

pub(super) fn run_graph() -> Result<()> {
    let graph = FleetGraphSummary::load_default()?;
    println!("{}", graph.summary());
    Ok(())
}

pub(super) fn run_dashboard() -> Result<()> {
    let queue = TaskQueue::load_default()?;
    let mut dashboard = FleetDashboardSummary::load_default()?;
    dashboard.update_from_queue(&queue)?;
    println!("{}", dashboard.summary());
    Ok(())
}

pub(super) fn run_project(command: NativeFleetProjectCommand) -> Result<()> {
    match command {
        NativeFleetProjectCommand::Add {
            repo_url,
            identity,
            priority,
            name,
        } => run_project_add(&repo_url, &identity, priority.as_str(), &name),
        NativeFleetProjectCommand::List => run_project_list(),
        NativeFleetProjectCommand::Remove { name } => run_project_remove(&name),
    }
}

pub(super) fn run_project_add(
    repo_url: &str,
    identity: &str,
    priority: &str,
    name: &str,
) -> Result<()> {
    let mut dashboard = FleetDashboardSummary::load_default()?;
    let existing = dashboard.get_project(repo_url).or_else(|| {
        (!name.is_empty())
            .then(|| dashboard.get_project(name))
            .flatten()
    });
    if let Some(existing) = existing {
        println!(
            "Project already registered: {} ({})",
            existing.name, existing.repo_url
        );
        return Ok(());
    }

    let index = dashboard.add_project_and_save(repo_url, identity, name, priority)?;
    let project = dashboard.projects[index].clone();

    let _ = ensure_default_project_registry_entry(
        &project.name,
        ProjectRegistryEntry {
            repo_url: repo_url.to_string(),
            identity: identity.to_string(),
            priority: priority.to_string(),
            objectives: Vec::new(),
        },
    )?;

    println!("Added project: {}", project.name);
    println!("  Repo: {}", project.repo_url);
    if !identity.is_empty() {
        println!("  Identity: {identity}");
    }
    println!("  Priority: {priority}");
    Ok(())
}

pub(super) fn run_project_list() -> Result<()> {
    let dashboard = FleetDashboardSummary::load_default()?;
    println!("{}", render_project_list(&dashboard));
    Ok(())
}

pub(super) fn run_project_remove(name: &str) -> Result<()> {
    let mut dashboard = FleetDashboardSummary::load_default()?;
    if dashboard.remove_project_and_save(name)? {
        println!("Removed project: {name}");
    } else {
        println!("Project not found: {name}");
    }
    Ok(())
}

pub(super) fn run_copilot_status() -> Result<()> {
    println!("{}", render_copilot_status(&default_copilot_lock_dir())?);
    Ok(())
}

pub(super) fn run_copilot_log(tail: usize) -> Result<()> {
    let report = read_copilot_log(&default_copilot_log_dir(), tail)?;
    for _ in 0..report.malformed_entries {
        eprintln!("  (skipped malformed entry)");
    }
    println!("{}", report.rendered);
    Ok(())
}

pub(super) fn run_watch(vm_name: &str, session_name: &str, lines: u32) -> Result<()> {
    run_watch_with_timeout(vm_name, session_name, lines, CLI_WATCH_TIMEOUT)
}

pub(super) fn run_watch_with_timeout(
    vm_name: &str,
    session_name: &str,
    lines: u32,
    timeout: Duration,
) -> Result<()> {
    validate_vm_name(vm_name)?;
    validate_session_name(session_name)?;
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };
    let output = capture_tmux_output_with_timeout(&azlin, vm_name, session_name, lines, timeout)?;
    println!("--- {vm_name}/{session_name} ---");
    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }
    println!("--- end ---");

    Ok(())
}

pub(super) fn capture_tmux_output_with_timeout(
    azlin_path: &Path,
    vm_name: &str,
    session_name: &str,
    lines: u32,
    timeout: Duration,
) -> Result<String> {
    validate_vm_name(vm_name)?;
    validate_session_name(session_name)?;
    let lines = lines.clamp(1, 10_000);
    let command = format!("tmux capture-pane -t {session_name} -p -S -{lines}");
    let mut cmd = Command::new(azlin_path);
    cmd.args(["connect", vm_name, "--no-tmux", "--", &command]);

    match run_output_with_timeout(cmd, timeout) {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout).into()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(format!(
                "Failed to capture: {}",
                sanitize_external_error_detail(stderr.trim(), 200)
            ))
        }
        Err(error) if error.to_string().contains("timed out after") => {
            Ok("Timeout connecting to VM".to_string())
        }
        Err(error) => Err(error),
    }
}
