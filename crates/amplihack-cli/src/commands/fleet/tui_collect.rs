use super::*;

/// T1: Background helper to create a tmux session.  Returns a human-readable
/// status message (success or error) rather than a Result so it can be sent
/// through an mpsc channel.
pub(super) fn background_create_session(azlin_path: &Path, vm_name: &str, agent: &str) -> String {
    let session_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 10_000;
    let session_name = format!("{agent}-{session_suffix:04}");
    let remote_cmd = format!(
        "tmux new-session -d -s {} {}",
        shell_single_quote(&session_name),
        shell_single_quote(&format!("amplihack {agent}"))
    );
    let mut cmd = Command::new(azlin_path);
    cmd.args(["connect", vm_name, "--no-tmux", "--", &remote_cmd]);
    match run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
        Ok(output) if output.status.success() => {
            format!("Created session '{session_name}' on {vm_name} running {agent}.")
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.trim();
            if detail.is_empty() {
                format!("Failed to create {agent} session on {vm_name}.")
            } else {
                format!("Failed to create {agent} session on {vm_name}: {detail}")
            }
        }
        Err(e) => format!("Failed to create {agent} session on {vm_name}: {e}"),
    }
}

pub(super) fn render_tui_once(azlin_path: &Path, interval: u64, capture_lines: usize) -> Result<String> {
    let state = collect_observed_fleet_state(azlin_path, capture_lines)?;
    let mut ui_state = FleetTuiUiState::default();
    ui_state.sync_to_state(&state);
    render_tui_frame(&state, interval, &ui_state)
}

/// Return the terminal width, capped at 100 columns for readability.
pub(super) fn collect_observed_fleet_state(azlin_path: &Path, capture_lines: usize) -> Result<FleetState> {
    collect_observed_fleet_state_with_progress(azlin_path, capture_lines, |_, _| Ok(()))
}

pub(super) fn collect_observed_fleet_state_with_progress<F>(
    azlin_path: &Path,
    capture_lines: usize,
    mut on_update: F,
) -> Result<FleetState>
where
    F: FnMut(&FleetState, FleetRefreshProgress) -> Result<()>,
{
    let mut state = FleetState::new(azlin_path.to_path_buf());
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh_inventory();

    let running_indices = state
        .vms
        .iter()
        .enumerate()
        .filter(|(_, vm)| vm.is_running())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let total_running = running_indices.len();
    on_update(
        &state,
        FleetRefreshProgress {
            completed_vms: 0,
            total_vms: total_running,
            current_vm: running_indices
                .first()
                .and_then(|index| state.vms.get(*index))
                .map(|vm| vm.name.clone()),
        },
    )?;

    let mut observer = FleetObserver::new(azlin_path.to_path_buf());
    observer.capture_lines = capture_lines.clamp(1, MAX_CAPTURE_LINES);
    let adopter = SessionAdopter::new(azlin_path.to_path_buf());
    for (position, vm_index) in running_indices.iter().copied().enumerate() {
        let vm_name = state
            .vms
            .get(vm_index)
            .map(|vm| vm.name.clone())
            .unwrap_or_default();
        let should_poll_tmux = state.is_managed_vm(&vm_name)
            || existing_vms.iter().any(|existing| existing == &vm_name);

        if let Some(vm) = state.vms.get_mut(vm_index) {
            if should_poll_tmux {
                vm.tmux_sessions = FleetState::poll_tmux_sessions_with_path(azlin_path, &vm.name);
            }

            let discovered = adopter.discover_sessions(&vm.name);
            for session in &mut vm.tmux_sessions {
                if let Some(metadata) = discovered
                    .iter()
                    .find(|candidate| candidate.session_name == session.session_name)
                {
                    session.working_directory = metadata.working_directory.clone();
                    session.repo_url = metadata.inferred_repo.clone();
                    session.git_branch = metadata.inferred_branch.clone();
                    session.pr_url = metadata.inferred_pr.clone();
                    session.task_summary = metadata.inferred_task.clone();
                }
            }
            for session in &mut vm.tmux_sessions {
                let observation = observer.observe_session(&vm.name, &session.session_name)?;
                session.agent_status = observation.status;
                session.last_output = observation.last_output_lines.join("\n");
            }
        }

        on_update(
            &state,
            FleetRefreshProgress {
                completed_vms: position + 1,
                total_vms: total_running,
                current_vm: running_indices
                    .get(position + 1)
                    .and_then(|index| state.vms.get(*index))
                    .map(|vm| vm.name.clone()),
            },
        )?;
    }

    Ok(state)
}



