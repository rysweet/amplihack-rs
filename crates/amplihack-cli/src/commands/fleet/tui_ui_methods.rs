use super::*;

impl FleetTuiUiState {
    pub(super) fn sync_to_state(&mut self, state: &FleetState) {
        let sessions = self.session_refs(state);
        if sessions.is_empty() {
            self.selected = None;
        } else {
            let selected_exists = self
                .selected
                .as_ref()
                .is_some_and(|selected| sessions.iter().any(|candidate| candidate == selected));
            if !selected_exists {
                self.selected = sessions.into_iter().next();
            }
        }
        if self.detail_capture.as_ref().is_some_and(|capture| {
            self.selected.as_ref().is_none_or(|selected| {
                capture.vm_name != selected.vm_name || capture.session_name != selected.session_name
            })
        }) {
            self.detail_capture = None;
        }

        let running_vms = Self::new_session_vm_refs(state);
        if running_vms.is_empty() {
            self.new_session_vm = None;
        } else {
            let selected_exists = self
                .new_session_vm
                .as_ref()
                .is_some_and(|selected| running_vms.iter().any(|candidate| candidate == selected));
            if !selected_exists {
                self.new_session_vm = running_vms.into_iter().next();
            }
        }

        self.sync_project_selection();
    }

    pub(super) fn move_selection(&mut self, state: &FleetState, delta: isize) {
        if self.tab == FleetTuiTab::NewSession {
            self.move_new_session_target(state, delta);
            return;
        }
        if self.tab == FleetTuiTab::Projects {
            self.move_project_selection(delta);
            return;
        }

        let sessions = self.session_refs(state);
        if sessions.is_empty() {
            self.selected = None;
            return;
        }

        let current_index = self
            .selected
            .as_ref()
            .and_then(|selected| sessions.iter().position(|candidate| candidate == selected))
            .unwrap_or(0);
        let len = sessions.len() as isize;
        let next = (current_index as isize + delta).rem_euclid(len) as usize;
        self.selected = sessions.get(next).cloned();
    }

    pub(super) fn move_new_session_target(&mut self, state: &FleetState, delta: isize) {
        let running_vms = Self::new_session_vm_refs(state);
        if running_vms.is_empty() {
            self.new_session_vm = None;
            return;
        }

        let current_index = self
            .new_session_vm
            .as_ref()
            .and_then(|selected| {
                running_vms
                    .iter()
                    .position(|candidate| candidate == selected)
            })
            .unwrap_or(0);
        let len = running_vms.len() as isize;
        let next = (current_index as isize + delta).rem_euclid(len) as usize;
        self.new_session_vm = running_vms.get(next).cloned();
    }

    pub(super) fn sync_project_selection(&mut self) {
        let project_refs = match Self::project_refs() {
            Ok(project_refs) => project_refs,
            Err(error) => {
                self.selected_project_repo = None;
                self.status_message = Some(error.to_string());
                return;
            }
        };
        if project_refs.is_empty() {
            self.selected_project_repo = None;
            return;
        }

        let selected_exists = self
            .selected_project_repo
            .as_ref()
            .is_some_and(|selected| project_refs.iter().any(|candidate| candidate == selected));
        if !selected_exists {
            self.selected_project_repo = project_refs.into_iter().next();
        }
    }

    pub(super) fn move_project_selection(&mut self, delta: isize) {
        let project_refs = match Self::project_refs() {
            Ok(project_refs) => project_refs,
            Err(error) => {
                self.selected_project_repo = None;
                self.status_message = Some(error.to_string());
                return;
            }
        };
        if project_refs.is_empty() {
            self.selected_project_repo = None;
            return;
        }

        let current_index = self
            .selected_project_repo
            .as_ref()
            .and_then(|selected| {
                project_refs
                    .iter()
                    .position(|candidate| candidate == selected)
            })
            .unwrap_or(0);
        let len = project_refs.len() as isize;
        let next = (current_index as isize + delta).rem_euclid(len) as usize;
        self.selected_project_repo = project_refs.get(next).cloned();
    }

    pub(super) fn selection_matches(&self, vm_name: &str, session_name: &str) -> bool {
        self.selected.as_ref().is_some_and(|selected| {
            selected.vm_name == vm_name && selected.session_name == session_name
        })
    }

    pub(super) fn fleet_filter_summary(&self) -> String {
        let mut filters = Vec::new();
        if let Some(filter) = self.status_filter {
            filters.push(format!("filter: {}", filter.label()));
        }
        if let Some(search) = self.session_search.as_deref() {
            filters.push(format!("search: {search}"));
        }
        if filters.is_empty() {
            String::new()
        } else {
            format!("  [{}]", filters.join(", "))
        }
    }

    pub(super) fn matches_vm_search(&self, vm_name: &str) -> bool {
        let Some(search) = self.normalized_session_search() else {
            return true;
        };
        let search = search.to_ascii_lowercase();
        vm_name.to_ascii_lowercase().contains(&search)
    }

    pub(super) fn matches_session_search(&self, vm_name: &str, session_name: &str) -> bool {
        let Some(search) = self.normalized_session_search() else {
            return true;
        };
        let search = search.to_ascii_lowercase();
        vm_name.to_ascii_lowercase().contains(&search)
            || session_name.to_ascii_lowercase().contains(&search)
    }

    pub(super) fn selected_session<'a>(
        &self,
        state: &'a FleetState,
    ) -> Option<(&'a VmInfo, &'a TmuxSessionInfo)> {
        let selected = self.selected.as_ref()?;
        for vm in Self::fleet_vms(state, self.fleet_subview)
            .into_iter()
            .filter(|vm| vm.is_running())
        {
            if vm.name != selected.vm_name {
                continue;
            }
            if let Some(session) = vm
                .tmux_sessions
                .iter()
                .find(|session| session.session_name == selected.session_name)
            {
                return Some((vm, session));
            }
        }
        None
    }

    pub(super) fn session_refs(&self, state: &FleetState) -> Vec<FleetTuiSelection> {
        let mut sessions = Vec::new();
        for vm in Self::fleet_vms(state, self.fleet_subview)
            .into_iter()
            .filter(|vm| vm.is_running())
        {
            for session in &vm.tmux_sessions {
                if self
                    .status_filter
                    .is_some_and(|filter| !filter.matches(session.agent_status))
                {
                    continue;
                }
                if !self.matches_session_search(&vm.name, &session.session_name) {
                    continue;
                }
                sessions.push(FleetTuiSelection {
                    vm_name: vm.name.clone(),
                    session_name: session.session_name.clone(),
                });
            }
        }
        sessions
    }

    pub(super) fn fleet_vms(state: &FleetState, subview: FleetSubview) -> Vec<&VmInfo> {
        match subview {
            FleetSubview::Managed => state.managed_vms(),
            FleetSubview::AllSessions => state.all_vms(),
        }
    }

    pub(super) fn new_session_vm_refs(state: &FleetState) -> Vec<String> {
        state
            .managed_vms()
            .into_iter()
            .filter(|vm| vm.is_running())
            .map(|vm| vm.name.clone())
            .collect()
    }

    pub(super) fn project_refs() -> Result<Vec<String>> {
        FleetDashboardSummary::project_repo_refs_default()
    }

    /// Advance to the next tab, wrapping around.
    pub(super) fn cycle_tab_forward(&mut self) {
        self.tab = match self.tab {
            FleetTuiTab::Fleet => FleetTuiTab::Detail,
            FleetTuiTab::Detail => FleetTuiTab::Editor,
            FleetTuiTab::Editor => FleetTuiTab::Projects,
            FleetTuiTab::Projects => FleetTuiTab::NewSession,
            FleetTuiTab::NewSession => FleetTuiTab::Fleet,
        };
    }

    /// Retreat to the previous tab, wrapping around.
    pub(super) fn cycle_tab_backward(&mut self) {
        self.tab = match self.tab {
            FleetTuiTab::Fleet => FleetTuiTab::NewSession,
            FleetTuiTab::Detail => FleetTuiTab::Fleet,
            FleetTuiTab::Editor => FleetTuiTab::Detail,
            FleetTuiTab::Projects => FleetTuiTab::Editor,
            FleetTuiTab::NewSession => FleetTuiTab::Projects,
        };
    }

    /// Set or clear a status filter.  Calling with the same filter clears it (toggle).
    pub(super) fn toggle_filter(&mut self, filter: StatusFilter) {
        if self.status_filter == Some(filter) {
            self.status_filter = None;
        } else {
            self.status_filter = Some(filter);
        }
    }

    pub(super) fn normalized_session_search(&self) -> Option<&str> {
        self.session_search
            .as_deref()
            .map(str::trim)
            .filter(|search| !search.is_empty())
    }

    pub(super) fn set_selected_proposal_notice(&mut self, title: &str, message: impl Into<String>) {
        let Some((vm_name, session_name)) = self
            .selected
            .as_ref()
            .map(|selected| (selected.vm_name.clone(), selected.session_name.clone()))
        else {
            return;
        };
        self.set_proposal_notice_for_session(&vm_name, &session_name, title, message);
    }

    pub(super) fn set_proposal_notice_for_session(
        &mut self,
        vm_name: &str,
        session_name: &str,
        title: &str,
        message: impl Into<String>,
    ) {
        self.proposal_notice = Some(FleetProposalNotice {
            vm_name: vm_name.to_string(),
            session_name: session_name.to_string(),
            title: title.to_string(),
            message: message.into(),
        });
    }

    pub(super) fn load_selected_proposal_into_editor(&mut self) {
        self.inline_input = None;
        self.proposal_notice = None;
        let Some(selected) = self.selected.as_ref() else {
            self.status_message = Some("No session selected for editing.".to_string());
            return;
        };
        let Some(decision) = self.last_decision.as_ref().filter(|decision| {
            decision.vm_name == selected.vm_name && decision.session_name == selected.session_name
        }) else {
            self.status_message =
                Some("No prepared proposal for the selected session.".to_string());
            return;
        };
        self.editor_decision = Some(decision.clone());
        self.tab = FleetTuiTab::Editor;
        self.status_message = Some(format!(
            "Loaded proposal into editor for {}/{}.",
            decision.vm_name, decision.session_name
        ));
    }

}
