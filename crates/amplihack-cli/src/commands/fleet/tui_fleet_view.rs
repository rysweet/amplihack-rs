use super::*;

pub(super) fn cockpit_render_fleet_view(
    state: &FleetState,
    ui_state: &FleetTuiUiState,
    lines: &mut Vec<String>,
    inner: usize,
) {
    let mut rows: Vec<FleetTuiRow<'_>> = Vec::new();
    for vm in FleetTuiUiState::fleet_vms(state, ui_state.fleet_subview)
        .into_iter()
        .filter(|vm| vm.is_running())
    {
        if vm.tmux_sessions.is_empty() {
            if ui_state.status_filter.is_none() && ui_state.matches_vm_search(&vm.name) {
                rows.push(FleetTuiRow::Placeholder(vm));
            }
            continue;
        }
        rows.extend(
            vm.tmux_sessions
                .iter()
                .filter(|session| {
                    ui_state
                        .status_filter
                        .is_none_or(|f| f.matches(session.agent_status))
                        && ui_state.matches_session_search(&vm.name, &session.session_name)
                })
                .map(|session| FleetTuiRow::Session(vm, session)),
        );
    }
    rows.sort_by_key(|row| match row {
        FleetTuiRow::Session(vm, session) => (
            status_sort_priority(session.agent_status),
            vm.name.as_str(),
            session.session_name.as_str(),
        ),
        FleetTuiRow::Placeholder(vm) => (
            status_sort_priority(AgentStatus::NoSession),
            vm.name.as_str(),
            "",
        ),
    });

    let filter_label = ui_state
        .status_filter
        .map(|f| format!(" [filter: {}]", f.label()))
        .unwrap_or_default();
    let subviews = [FleetSubview::Managed, FleetSubview::AllSessions]
        .iter()
        .map(|subview| {
            let label = subview.label();
            if *subview == ui_state.fleet_subview {
                format!("{ANSI_BOLD}{ANSI_CYAN}[{label}]{ANSI_RESET}")
            } else {
                format!(" {label} ")
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let subviews_raw = [FleetSubview::Managed, FleetSubview::AllSessions]
        .iter()
        .map(|subview| subview.label().len() + 2)
        .sum::<usize>()
        + 3;
    lines.push(cockpit_boxline(&subviews, subviews_raw, inner));
    let heading = format!("{}{}", ui_state.fleet_subview.title(), filter_label);
    lines.push(cockpit_boxline(&heading, heading.len(), inner));
    if let Some(input) = ui_state
        .inline_input
        .as_ref()
        .filter(|input| input.mode == FleetTuiInlineInputMode::SearchSessions)
    {
        let prompt = format!(
            "Search sessions > {}_",
            truncate_chars(&input.buffer, inner.saturating_sub(24))
        );
        lines.push(cockpit_boxline(&prompt, prompt.len(), inner));
        let controls = "Enter apply | Esc cancel | Backspace delete";
        lines.push(cockpit_boxline(controls, controls.len(), inner));
    } else if let Some(search) = ui_state.session_search.as_deref() {
        let active = format!("Search: {search} (press / to edit, Esc to clear)");
        let active = truncate_chars(&active, inner);
        lines.push(cockpit_boxline(&active, active.len(), inner));
    }
    lines.push(cockpit_boxline("", 0, inner));

    if rows.is_empty() {
        let msg = if ui_state.status_filter.is_some() && ui_state.session_search.is_some() {
            "No sessions match the current filter/search. Press Esc or '*' to clear."
        } else if ui_state.session_search.is_some() {
            "No sessions match the current search. Press Esc to clear."
        } else if ui_state.status_filter.is_some() {
            "No sessions match the current filter.  Press '*' to clear."
        } else {
            "No running tmux session output available."
        };
        lines.push(cockpit_boxline(msg, msg.len(), inner));
        return;
    }

    if let Some((vm, session)) = ui_state.selected_session(state) {
        let selected_heading = format!(
            "Selected session: {}/{} ({})",
            vm.name,
            session.session_name,
            session.agent_status.as_str()
        );
        lines.push(cockpit_boxline(
            &selected_heading,
            selected_heading.len(),
            inner,
        ));
        for metadata in session_metadata_lines(session, "  ") {
            let line = truncate_chars(&metadata, inner.saturating_sub(2));
            lines.push(cockpit_boxline(&line, line.len(), inner));
        }
        let preview: Vec<&str> = session
            .last_output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if preview.is_empty() {
            let none = "  no output captured";
            lines.push(cockpit_boxline(none, none.len(), inner));
        } else {
            for line in &preview[preview.len().saturating_sub(3)..] {
                let line = truncate_chars(line, inner.saturating_sub(4));
                let raw = 2 + line.len();
                lines.push(cockpit_boxline(&format!("  {line}"), raw, inner));
            }
        }
        lines.push(cockpit_boxline("", 0, inner));
    }

    let mut current_vm: Option<&str> = None;
    for row in &rows {
        match row {
            FleetTuiRow::Session(vm, session) => {
                // VM section header (emitted once per VM block).
                if current_vm != Some(vm.name.as_str()) {
                    current_vm = Some(vm.name.as_str());
                    let management_label = if ui_state.fleet_subview == FleetSubview::AllSessions {
                        if state.is_managed_vm(&vm.name) {
                            " managed"
                        } else {
                            " unmanaged"
                        }
                    } else {
                        ""
                    };
                    let dash_len = inner.saturating_sub(vm.name.len() + management_label.len() + 4);
                    let dashes: String = std::iter::repeat_n(BOX_DASH, dash_len).collect();
                    let vm_hdr_raw = 2 + vm.name.len() + management_label.len() + 1 + dash_len;
                    let vm_hdr = format!(
                        "  {ANSI_BOLD}[{name}]{ANSI_RESET}{management_label} {ANSI_DIM}{dashes}{ANSI_RESET}",
                        name = vm.name,
                    );
                    lines.push(cockpit_boxline(&vm_hdr, vm_hdr_raw, inner));
                }

                let selected = ui_state.selection_matches(&vm.name, &session.session_name);
                let (color, icon) = status_color_and_icon(session.agent_status);
                let marker = if selected { ">" } else { " " };
                let status_label = session.agent_status.as_str().to_uppercase();
                let name = if session.session_name.len() > 18 {
                    format!("{}...", &session.session_name[..15])
                } else {
                    session.session_name.clone()
                };
                let sess_raw = 4 + 1 + 1 + name.len() + 2 + status_label.len();
                let sess_line = format!(
                    "  {marker} {color}{icon}{ANSI_RESET} {ANSI_BOLD}{name}{ANSI_RESET}  \
                     {ANSI_DIM}{status_label}{ANSI_RESET}"
                );
                lines.push(cockpit_boxline(&sess_line, sess_raw, inner));

                // Last-output preview (up to 2 lines).
                let preview: Vec<&str> = session
                    .last_output
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                if preview.is_empty() {
                    let no_out = "    | no output captured";
                    lines.push(cockpit_boxline(no_out, no_out.len(), inner));
                } else {
                    for line in preview
                        .iter()
                        .rev()
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        let t = truncate_chars(line, inner.saturating_sub(6));
                        let pl_raw = 6 + t.len();
                        let pl = format!("    {ANSI_DIM}| {t}{ANSI_RESET}");
                        lines.push(cockpit_boxline(&pl, pl_raw, inner));
                    }
                }
                lines.push(cockpit_boxline("", 0, inner));
            }
            FleetTuiRow::Placeholder(vm) => {
                let management_label = if ui_state.fleet_subview == FleetSubview::AllSessions {
                    if state.is_managed_vm(&vm.name) {
                        " managed"
                    } else {
                        " unmanaged"
                    }
                } else {
                    ""
                };
                let (color, icon) = status_color_and_icon(AgentStatus::NoSession);
                let text = format!(
                    "   {color}{icon}{ANSI_RESET} {ANSI_DIM}{name}/(no sessions){management_label} (empty){ANSI_RESET}",
                    name = vm.name,
                );
                let text_raw = 3 + 1 + 1 + vm.name.len() + 14 + management_label.len() + 7;
                lines.push(cockpit_boxline(&text, text_raw, inner));
                lines.push(cockpit_boxline("", 0, inner));
            }
        }
    }
}
