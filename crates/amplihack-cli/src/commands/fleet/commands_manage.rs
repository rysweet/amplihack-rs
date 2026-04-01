use super::*;

pub(super) fn run_adopt(vm_name: &str, sessions: &[String]) -> Result<()> {
    validate_vm_name(vm_name)?;
    for session in sessions {
        validate_session_name(session)?;
    }
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let adopter = SessionAdopter::new(azlin);
    let mut queue = TaskQueue::load_default()?;

    println!("Discovering sessions on {vm_name}...");
    let discovered = adopter.discover_sessions(vm_name);
    if discovered.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("Found {} sessions:", discovered.len());
    for session in &discovered {
        println!("  {}", session.session_name);
        if !session.inferred_repo.is_empty() {
            println!("    Repo: {}", session.inferred_repo);
        }
        if !session.inferred_branch.is_empty() {
            println!("    Branch: {}", session.inferred_branch);
        }
        if !session.agent_type.is_empty() {
            println!("    Agent: {}", session.agent_type);
        }
    }

    let session_filter = (!sessions.is_empty()).then(|| sessions.to_vec());
    let adopted = adopter.adopt_sessions(vm_name, &mut queue, session_filter.as_deref())?;

    println!("\nAdopted {} sessions:", adopted.len());
    for session in &adopted {
        if let Some(task_id) = &session.task_id {
            println!("  {} -> task {}", session.session_name, task_id);
        }
    }

    Ok(())
}

pub(super) fn run_report() -> Result<()> {
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let queue = TaskQueue::load_default()?;
    let state = perceive_fleet_state(azlin)?;
    println!("{}", render_report(&state, &queue));
    Ok(())
}

pub(super) fn run_observe(vm_name: &str) -> Result<()> {
    validate_vm_name(vm_name)?;
    let azlin = match get_azlin_path() {
        Ok(path) => path,
        Err(_) => {
            eprintln!("Error: azlin not found. Install azlin or set AZLIN_PATH.");
            return Err(exit_error(1));
        }
    };

    let mut state = FleetState::new(azlin.clone());
    state.refresh();

    let Some(vm) = state.get_vm(vm_name) else {
        println!("VM not found: {vm_name}");
        return Err(exit_error(1));
    };

    if vm.tmux_sessions.is_empty() {
        println!("No tmux sessions on {vm_name}");
        return Ok(());
    }

    let observer = FleetObserver::new(azlin);
    println!("{}", render_observe(vm, &observer)?);
    Ok(())
}

pub(super) fn run_queue() -> Result<()> {
    let queue = TaskQueue::load_default()?;
    println!("{}", queue.summary());
    Ok(())
}

pub(super) fn run_add_task(
    prompt: &str,
    repo: &str,
    priority: NativeTaskPriorityArg,
    agent: NativeAgentArg,
    mode: NativeAgentModeArg,
    max_turns: u32,
    protected: bool,
) -> Result<()> {
    let _ = protected;
    let mut queue = TaskQueue::load_default()?;
    let task = queue.add_task(
        prompt,
        repo,
        priority.into_task_priority(),
        agent.as_str(),
        mode.as_str(),
        max_turns,
    )?;

    println!("Task {} added: {}", task.id, truncate_chars(prompt, 80));
    println!(
        "Priority: {}, Agent: {}, Mode: {}",
        priority.as_str(),
        agent.as_str(),
        mode.as_str()
    );
    Ok(())
}

