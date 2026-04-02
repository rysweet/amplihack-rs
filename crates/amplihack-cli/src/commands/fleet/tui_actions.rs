use super::*;

pub(super) fn run_tui_dry_run(
    azlin_path: &Path,
    state: &FleetState,
    ui_state: &mut FleetTuiUiState,
) -> Result<()> {
    let Some((vm, session)) = ui_state.selected_session(state) else {
        ui_state.status_message = Some("No session selected for dry-run.".to_string());
        return Ok(());
    };

    let backend = NativeReasonerBackend::detect("auto")?;
    let mut reasoner = FleetSessionReasoner::new(azlin_path.to_path_buf(), backend);
    let analysis = reasoner.reason_about_session(
        &vm.name,
        &session.session_name,
        "",
        "",
        Some(&session.last_output),
    )?;
    let summary = format!(
        "Prepared proposal for {}/{}: {} ({:.0}%)",
        analysis.decision.vm_name,
        analysis.decision.session_name,
        analysis.decision.action.as_str(),
        analysis.decision.confidence * 100.0
    );
    ui_state.proposal_notice = analysis
        .diagnostic
        .as_ref()
        .map(|diagnostic| FleetProposalNotice {
            vm_name: analysis.decision.vm_name.clone(),
            session_name: analysis.decision.session_name.clone(),
            title: "Reasoner status".to_string(),
            message: diagnostic.clone(),
        });
    ui_state.last_decision = Some(analysis.decision);
    ui_state.status_message = Some(summary);
    ui_state.tab = FleetTuiTab::Detail;
    Ok(())
}

pub(super) fn run_tui_refresh_detail_capture(
    azlin_path: &Path,
    state: &FleetState,
    ui_state: &mut FleetTuiUiState,
    capture_lines: u32,
) -> Result<()> {
    let Some((vm, session)) = ui_state.selected_session(state) else {
        // Nothing selected; leave cache as-is (stale entries are harmless).
        return Ok(());
    };
    let output = capture_tmux_output_with_timeout(
        azlin_path,
        &vm.name,
        &session.session_name,
        capture_lines,
        CLI_WATCH_TIMEOUT,
    )?;
    // T5: Store into the LRU cache (and keep compat field in sync).
    ui_state.put_capture(&vm.name, &session.session_name, output.clone());
    ui_state.detail_capture = Some(FleetDetailCapture {
        vm_name: vm.name.clone(),
        session_name: session.session_name.clone(),
        output,
    });
    Ok(())
}

pub(super) fn run_tui_apply(azlin_path: &Path, ui_state: &mut FleetTuiUiState) -> Result<()> {
    let Some(selected) = ui_state.selected.as_ref() else {
        ui_state.status_message = Some("No session selected to apply.".to_string());
        return Ok(());
    };
    let Some(decision) = ui_state.last_decision.clone().filter(|decision| {
        decision.vm_name == selected.vm_name && decision.session_name == selected.session_name
    }) else {
        ui_state.status_message = Some("No prepared proposal to apply.".to_string());
        return Ok(());
    };

    let reasoner = FleetSessionReasoner::new(azlin_path.to_path_buf(), NativeReasonerBackend::None);
    match reasoner.execute_decision(&decision) {
        Ok(()) => {
            let message = format!(
                "Applied {} to {}/{}.",
                decision.action.as_str(),
                decision.vm_name,
                decision.session_name
            );
            ui_state.set_proposal_notice_for_session(
                &decision.vm_name,
                &decision.session_name,
                "Apply status",
                message.clone(),
            );
            ui_state.status_message = Some(message);
            Ok(())
        }
        Err(error) => {
            ui_state.set_proposal_notice_for_session(
                &decision.vm_name,
                &decision.session_name,
                "Apply status",
                format!("Apply failed: {error}"),
            );
            ui_state.status_message = Some(format!("Apply failed: {error}"));
            Ok(())
        }
    }
}

pub(super) fn run_tui_edit(ui_state: &mut FleetTuiUiState) {
    ui_state.load_selected_proposal_into_editor();
    // T3: Also populate the multiline editor buffer with the existing proposal input.
    let initial = ui_state
        .editor_decision
        .as_ref()
        .map(|d| d.input_text.as_str())
        .unwrap_or("")
        .to_string();
    ui_state.enter_multiline_editor(&initial);
    ui_state.editor_active = false;
}

pub(super) fn handle_tui_inline_input_key(
    ui_state: &mut FleetTuiUiState,
    key: DashboardKey,
) -> Result<()> {
    match key {
        DashboardKey::Char('\n') | DashboardKey::Char('\r') => {
            if let Some((mode, value)) = ui_state.finish_inline_input() {
                match mode {
                    FleetTuiInlineInputMode::AddProjectRepo => {
                        add_project_from_repo_input(ui_state, &value)?
                    }
                    FleetTuiInlineInputMode::SearchSessions => {
                        ui_state.apply_inline_session_search(&value)
                    }
                }
            }
        }
        DashboardKey::Char('\u{1b}') => ui_state.cancel_inline_input(),
        DashboardKey::Char('\u{8}') | DashboardKey::Char('\u{7f}') => {
            ui_state.pop_inline_input_char()
        }
        DashboardKey::Char(ch) if !ch.is_control() => ui_state.push_inline_input_char(ch),
        _ => {}
    }
    Ok(())
}

pub(super) fn handle_tui_editor_active_key(
    azlin_path: &Path,
    ui_state: &mut FleetTuiUiState,
    key: DashboardKey,
) -> Result<()> {
    match key {
        DashboardKey::Char('\u{1b}') => {
            ui_state.editor_discard();
            ui_state.tab = FleetTuiTab::Detail;
            ui_state.status_message = Some("Editor changes discarded.".to_string());
        }
        DashboardKey::Char('\x13') => ui_state.editor_save(),
        DashboardKey::Char('\n') | DashboardKey::Char('\r') => ui_state.editor_insert_char('\n'),
        DashboardKey::Up => ui_state.editor_move_up(),
        DashboardKey::Down => ui_state.editor_move_down(),
        DashboardKey::Char('\u{8}') | DashboardKey::Char('\u{7f}') => ui_state.editor_backspace(),
        DashboardKey::Char('A') => run_tui_apply_edited(azlin_path, ui_state)?,
        DashboardKey::Char(ch) if !ch.is_control() => ui_state.editor_insert_char(ch),
        _ => {}
    }
    Ok(())
}

pub(super) fn run_tui_edit_input(ui_state: &mut FleetTuiUiState) -> Result<()> {
    let Some((vm_name, session_name, input_text)) =
        ui_state.editor_decision.as_ref().map(|decision| {
            (
                decision.vm_name.clone(),
                decision.session_name.clone(),
                decision.input_text.clone(),
            )
        })
    else {
        ui_state.status_message = Some("No editor proposal loaded. Press 'e' first.".to_string());
        return Ok(());
    };
    if ui_state.editor_lines.is_empty() {
        ui_state.enter_multiline_editor(&input_text);
    }
    ui_state.editor_active = true;
    ui_state.status_message = Some(format!(
        "Editing input for {}/{}. Enter adds lines, Ctrl-S saves, Esc cancels.",
        vm_name, session_name
    ));
    Ok(())
}

pub(super) fn run_tui_apply_edited(
    azlin_path: &Path,
    ui_state: &mut FleetTuiUiState,
) -> Result<()> {
    let Some(mut decision) = ui_state.editor_decision.clone() else {
        ui_state.status_message =
            Some("No edited proposal to apply. Press 'e' to open the editor.".to_string());
        return Ok(());
    };
    if !ui_state.editor_lines.is_empty() {
        decision.input_text = ui_state.editor_content();
    }

    let reasoner = FleetSessionReasoner::new(azlin_path.to_path_buf(), NativeReasonerBackend::None);
    match reasoner.execute_decision(&decision) {
        Ok(()) => {
            let message = format!(
                "Applied edited {} to {}/{}.",
                decision.action.as_str(),
                decision.vm_name,
                decision.session_name
            );
            ui_state.set_proposal_notice_for_session(
                &decision.vm_name,
                &decision.session_name,
                "Apply status",
                message.clone(),
            );
            ui_state.editor_active = false;
            ui_state.last_decision = Some(decision.clone());
            ui_state.editor_decision = Some(decision.clone());
            ui_state.tab = FleetTuiTab::Detail;
            ui_state.status_message = Some(message);
            Ok(())
        }
        Err(error) => {
            ui_state.editor_active = false;
            ui_state.set_proposal_notice_for_session(
                &decision.vm_name,
                &decision.session_name,
                "Apply status",
                format!("Edited apply failed: {error}"),
            );
            ui_state.status_message = Some(format!("Edited apply failed: {error}"));
            Ok(())
        }
    }
}

pub(super) fn run_tui_adopt_selected_session(
    azlin_path: &Path,
    state: &FleetState,
    ui_state: &mut FleetTuiUiState,
) -> Result<()> {
    let Some((vm, session)) = ui_state.selected_session(state) else {
        ui_state.status_message = Some("No session selected to adopt.".to_string());
        return Ok(());
    };

    let mut queue = TaskQueue::load_default()?;
    if queue.has_active_assignment(&vm.name, &session.session_name) {
        ui_state.status_message = Some(format!(
            "{}/{} is already adopted into the active fleet queue.",
            vm.name, session.session_name
        ));
        return Ok(());
    }

    let adopter = SessionAdopter::new(azlin_path.to_path_buf());
    let adopted = adopter.adopt_sessions(
        &vm.name,
        &mut queue,
        Some(std::slice::from_ref(&session.session_name)),
    )?;
    if adopted.is_empty() {
        ui_state.status_message = Some(format!(
            "No adoptable session found for {}/{}.",
            vm.name, session.session_name
        ));
        return Ok(());
    }

    ui_state.status_message = Some(format!(
        "Adopted {}/{} into the fleet queue.",
        vm.name, session.session_name
    ));

    // T2: After adoption, trigger a fast-status refresh.
    ui_state.send_bg_cmd(BackgroundCommand::ForceStatusRefresh);

    Ok(())
}

/// T1: Dispatch session creation to the background worker thread.
/// When background threads are not available (tests, non-interactive mode),
/// falls back to a synchronous blocking call.
pub(super) fn run_tui_create_session(
    azlin_path: &Path,
    ui_state: &mut FleetTuiUiState,
) -> Result<()> {
    let Some(vm_name) = ui_state.new_session_vm.as_deref() else {
        ui_state.status_message = Some("No running VM selected for session creation.".to_string());
        return Ok(());
    };
    validate_vm_name(vm_name)?;

    let agent = ui_state.new_session_agent.as_str().to_string();
    let vm_name_owned = vm_name.to_string();

    // Prefer background dispatch (T1 goal: non-blocking).
    if ui_state.bg_tx.is_some() {
        ui_state.send_bg_cmd(BackgroundCommand::CreateSession {
            azlin_path: azlin_path.to_path_buf(),
            vm_name: vm_name_owned.clone(),
            agent: agent.clone(),
        });
        ui_state.create_session_pending = true;
        ui_state.status_message = Some(format!(
            "Creating {agent} session on {vm_name_owned}... (background)"
        ));
    } else {
        // Synchronous fallback (tests / non-interactive).
        let msg = background_create_session(azlin_path, &vm_name_owned, &agent);
        ui_state.status_message = Some(msg);
        ui_state.tab = FleetTuiTab::Fleet;
    }
    Ok(())
}

/// T6: Wrapper that enters project Add sub-mode.
pub(super) fn run_tui_add_project(ui_state: &mut FleetTuiUiState) -> Result<()> {
    ui_state.enter_project_add_mode();
    Ok(())
}

pub(super) fn add_project_from_repo_input(
    ui_state: &mut FleetTuiUiState,
    repo_url: &str,
) -> Result<()> {
    let repo_url = repo_url.trim();
    if repo_url.is_empty() {
        ui_state.status_message = Some("Project add cancelled.".to_string());
        return Ok(());
    }

    let mut dashboard = FleetDashboardSummary::load_default()?;
    if let Some(existing) = dashboard.get_project(repo_url) {
        ui_state.selected_project_repo = Some(existing.repo_url.clone());
        ui_state.status_message = Some(format!(
            "Project '{}' already exists in the dashboard.",
            existing.name
        ));
        return Ok(());
    }

    let index = dashboard.add_project_and_save(repo_url, "", "", "medium")?;
    let project = &dashboard.projects[index];
    ui_state.selected_project_repo = Some(project.repo_url.clone());
    ui_state.status_message = Some(format!(
        "Added project '{}' to the dashboard.",
        project.name
    ));
    Ok(())
}

pub(super) fn run_tui_remove_project(ui_state: &mut FleetTuiUiState) -> Result<()> {
    let Some(selected_repo) = ui_state.selected_project_repo.clone() else {
        ui_state.status_message = Some("No project selected to remove.".to_string());
        return Ok(());
    };

    let mut dashboard = FleetDashboardSummary::load_default()?;
    let removed_name = dashboard
        .get_project(&selected_repo)
        .map(|project| project.name.clone())
        .unwrap_or_else(|| selected_repo.clone());
    if !dashboard.remove_project_and_save(&selected_repo)? {
        ui_state.status_message = Some(format!(
            "Selected project '{removed_name}' is no longer present."
        ));
        ui_state.sync_project_selection();
        return Ok(());
    }
    ui_state.sync_project_selection();
    ui_state.status_message = Some(format!(
        "Removed project '{removed_name}' from the dashboard."
    ));
    Ok(())
}
