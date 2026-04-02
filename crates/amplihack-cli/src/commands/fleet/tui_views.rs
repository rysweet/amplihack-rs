use super::*;

pub(super) fn cockpit_render_detail_view(
    state: &FleetState,
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let push = |lines: &mut Vec<String>, text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };

    let Some((vm, session)) = ui_state.selected_session(state) else {
        push(lines, "No session selected.");
        return;
    };

    let hdr = format!("Session Detail — {}/{}", vm.name, session.session_name);
    push(lines, &hdr);
    push(lines, "");
    push(
        lines,
        &format!("Status:   {}", session.agent_status.as_str()),
    );
    push(lines, &format!("Windows:  {}", session.windows));
    push(
        lines,
        &format!("Attached: {}", if session.attached { "yes" } else { "no" }),
    );
    for metadata in session_metadata_lines(session, "") {
        push(lines, &metadata);
    }
    push(lines, "");
    push(lines, "Captured output");

    // T5: Prefer the LRU cache entry; fall back to single-entry compat field, then
    // fall back to the session's last_output if neither is available.
    let cached_output = ui_state.get_capture(&vm.name, &session.session_name);
    let detail_output: &str = cached_output
        .as_deref()
        .unwrap_or(session.last_output.as_str());

    if detail_output.trim().is_empty() {
        push(lines, "(no output captured)");
    } else {
        for line in detail_output.lines() {
            let t = truncate_chars(line, inner.saturating_sub(2));
            lines.push(cockpit_boxline(&t, t.len(), inner));
        }
    }

    if let Some(decision) = &ui_state.last_decision
        && decision.vm_name == vm.name
        && decision.session_name == session.session_name
    {
        push(lines, "");
        push(lines, "Prepared proposal");
        for line in decision.summary().lines() {
            push(lines, line);
        }
    }

    if let Some(notice) = &ui_state.proposal_notice
        && notice.vm_name == vm.name
        && notice.session_name == session.session_name
    {
        push(lines, "");
        push(lines, &notice.title);
        push(lines, &notice.message);
    }

    push(lines, "");
    push(lines, "Detail actions");
    let detail_controls = if ui_state.last_decision.as_ref().is_some_and(|decision| {
        decision.vm_name == vm.name && decision.session_name == session.session_name
    }) {
        "d rerun proposal | e edit | a apply | x skip"
    } else {
        "d prepare proposal"
    };
    push(lines, detail_controls);
}

pub(super) fn session_metadata_lines(session: &TmuxSessionInfo, prefix: &str) -> Vec<String> {
    [
        (!session.git_branch.is_empty()).then(|| format!("{prefix}branch: {}", session.git_branch)),
        (!session.repo_url.is_empty()).then(|| format!("{prefix}repo: {}", session.repo_url)),
        (!session.working_directory.is_empty())
            .then(|| format!("{prefix}cwd: {}", session.working_directory)),
        (!session.pr_url.is_empty()).then(|| format!("{prefix}pr: {}", session.pr_url)),
        (!session.task_summary.is_empty())
            .then(|| format!("{prefix}task: {}", session.task_summary)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(super) fn cockpit_render_projects_view(
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) -> Result<()> {
    let dashboard = FleetDashboardSummary::load_default()?;
    if dashboard.projects.is_empty() {
        let text = render_project_list(&dashboard);
        for line in text.lines() {
            lines.push(cockpit_boxline(line, line.len(), inner));
        }
        lines.push(cockpit_boxline("", 0, inner));
        if let Some(input) = ui_state
            .inline_input
            .as_ref()
            .filter(|input| input.mode == FleetTuiInlineInputMode::AddProjectRepo)
        {
            let prompt = format!(
                "Add project repo > {}_",
                truncate_chars(&input.buffer, inner.saturating_sub(20))
            );
            lines.push(cockpit_boxline(&prompt, prompt.len(), inner));
            let controls = "Enter add | Esc cancel | Backspace delete";
            lines.push(cockpit_boxline(controls, controls.len(), inner));
            return Ok(());
        }
        let controls = "i add project repo";
        lines.push(cockpit_boxline(controls, controls.len(), inner));
        return Ok(());
    }

    let heading = format!("Fleet Projects ({})", dashboard.projects.len());
    lines.push(cockpit_boxline(&heading, heading.len(), inner));
    lines.push(cockpit_boxline("", 0, inner));
    for project in &dashboard.projects {
        let marker = if ui_state.selected_project_repo.as_deref() == Some(project.repo_url.as_str())
        {
            ">"
        } else {
            " "
        };
        let prio_label = match project.priority.as_str() {
            "high" => "!!!",
            "low" => "!",
            _ => "!!",
        };
        let summary = format!("  {marker} [{prio_label}] {}", project.name);
        lines.push(cockpit_boxline(&summary, summary.len(), inner));
        let repo = format!("      Repo: {}", project.repo_url);
        lines.push(cockpit_boxline(&repo, repo.len(), inner));
        if !project.github_identity.is_empty() {
            let identity = format!("      Identity: {}", project.github_identity);
            lines.push(cockpit_boxline(&identity, identity.len(), inner));
        }
        let stats = format!(
            "      Priority: {} | VMs: {} | Tasks: {}/{} | PRs: {}",
            project.priority,
            project.vms.len(),
            project.tasks_completed,
            project.tasks_total,
            project.prs_created.len()
        );
        lines.push(cockpit_boxline(&stats, stats.len(), inner));
        if !project.notes.is_empty() {
            let notes = format!("      Notes: {}", project.notes);
            lines.push(cockpit_boxline(&notes, notes.len(), inner));
        }
        lines.push(cockpit_boxline("", 0, inner));
    }
    if let Some(input) = ui_state
        .inline_input
        .as_ref()
        .filter(|input| input.mode == FleetTuiInlineInputMode::AddProjectRepo)
    {
        let prompt = format!(
            "Add project repo > {}_",
            truncate_chars(&input.buffer, inner.saturating_sub(20))
        );
        lines.push(cockpit_boxline(&prompt, prompt.len(), inner));
        let controls = "Enter add | Esc cancel | Backspace delete";
        lines.push(cockpit_boxline(controls, controls.len(), inner));
        return Ok(());
    }
    let controls = "j/k choose project | i add project repo | x remove selected";
    lines.push(cockpit_boxline(controls, controls.len(), inner));
    Ok(())
}

pub(super) fn cockpit_render_editor_view(
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let push = |lines: &mut Vec<String>, text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };

    push(lines, "Action Editor");
    push(lines, "");

    let Some(decision) = &ui_state.editor_decision else {
        push(lines, "No proposal loaded into the editor.");
        push(
            lines,
            "Press 'e' after preparing a proposal for the selected session.",
        );
        return;
    };

    push(
        lines,
        &format!("Target: {}/{}", decision.vm_name, decision.session_name),
    );
    push(lines, &format!("Action: {}", decision.action.as_str()));
    push(lines, "Action choices");
    for action in SessionAction::all() {
        let marker = if action == decision.action { ">" } else { " " };
        push(lines, &format!("  {marker} {}", action.as_str()));
    }
    push(
        lines,
        &format!("Confidence: {:.0}%", decision.confidence * 100.0),
    );
    push(lines, &format!("Reasoning: {}", decision.reasoning));
    push(lines, "");
    push(lines, "Edited input");
    let editor_lines = if ui_state.editor_lines.is_empty() {
        decision
            .input_text
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        ui_state.editor_lines.clone()
    };
    if editor_lines.is_empty() || (editor_lines.len() == 1 && editor_lines[0].is_empty()) {
        push(lines, "(empty)");
    } else {
        for (index, line) in editor_lines.iter().enumerate() {
            let rendered = if ui_state.editor_active && index == ui_state.editor_cursor_row {
                format!("{line}_")
            } else {
                line.clone()
            };
            let t = truncate_chars(&rendered, inner.saturating_sub(2));
            lines.push(cockpit_boxline(&t, t.len(), inner));
        }
    }
    push(lines, "");
    if let Some(notice) = &ui_state.proposal_notice
        && notice.vm_name == decision.vm_name
        && notice.session_name == decision.session_name
    {
        push(lines, &notice.title);
        push(lines, &notice.message);
        push(lines, "");
    }
    if ui_state.editor_active {
        push(lines, "Typing mode");
        push(
            lines,
            "Enter newline  Ctrl-S save  Esc cancel  Up/Down move  A apply edited",
        );
    } else {
        push(
            lines,
            "e reload  i focus editor  t cycle action  A apply edited",
        );
    }
}

pub(super) fn cockpit_render_new_session_view(
    state: &FleetState,
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let push = |lines: &mut Vec<String>, text: &str| {
        lines.push(cockpit_boxline(text, text.len(), inner));
    };

    push(lines, "New Session");
    push(lines, "");
    push(
        lines,
        &format!("Agent type: {}", ui_state.new_session_agent.as_str()),
    );
    push(lines, "");
    push(lines, "Running VMs");

    let running_vms = FleetTuiUiState::new_session_vm_refs(state);
    if running_vms.is_empty() {
        push(lines, "No running VMs available.");
    } else {
        for vm_name in &running_vms {
            let marker = if ui_state.new_session_vm.as_deref() == Some(vm_name.as_str()) {
                ">"
            } else {
                " "
            };
            let row = format!("  {marker} {vm_name}");
            lines.push(cockpit_boxline(&row, row.len(), inner));
        }
    }
    push(lines, "");
    push(
        lines,
        "n jump here | j/k choose VM | t cycle agent | Enter create",
    );
}
