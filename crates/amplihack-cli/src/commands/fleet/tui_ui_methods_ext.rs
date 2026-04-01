use super::*;

impl FleetTuiUiState {
    pub(super) fn start_inline_project_input(&mut self) {
        self.inline_input = Some(FleetTuiInlineInput {
            mode: FleetTuiInlineInputMode::AddProjectRepo,
            buffer: String::new(),
        });
        self.status_message =
            Some("Adding project repo inline. Type URL, Enter adds, Esc cancels.".to_string());
    }

    pub(super) fn start_inline_session_search(&mut self) {
        self.inline_input = Some(FleetTuiInlineInput {
            mode: FleetTuiInlineInputMode::SearchSessions,
            buffer: self.session_search.clone().unwrap_or_default(),
        });
        self.status_message = Some(
            "Searching fleet sessions inline. Type VM/session text, Enter applies, Esc cancels."
                .to_string(),
        );
    }

    pub(super) fn apply_inline_editor_input(&mut self, edited: &str) {
        let Some(decision) = self.editor_decision.as_mut() else {
            self.status_message = Some("No editor proposal loaded. Press 'e' first.".to_string());
            return;
        };
        self.proposal_notice = None;
        decision.input_text = edited.replace("\\n", "\n");
        self.status_message = Some(format!(
            "Updated editor input for {}/{}.",
            decision.vm_name, decision.session_name
        ));
    }

    pub(super) fn apply_inline_session_search(&mut self, search: &str) {
        let search = search.trim();
        if search.is_empty() {
            self.session_search = None;
            self.status_message = Some("Cleared fleet session search.".to_string());
            return;
        }
        self.session_search = Some(search.to_string());
        self.status_message = Some(format!("Searching fleet sessions for '{search}'."));
    }

    pub(super) fn clear_session_search(&mut self) {
        if self.session_search.take().is_some() {
            self.status_message = Some("Cleared fleet session search.".to_string());
        }
    }

    pub(super) fn push_inline_input_char(&mut self, ch: char) {
        if let Some(input) = self.inline_input.as_mut() {
            input.buffer.push(ch);
        }
    }

    pub(super) fn pop_inline_input_char(&mut self) {
        if let Some(input) = self.inline_input.as_mut() {
            input.buffer.pop();
        }
    }

    pub(super) fn finish_inline_input(&mut self) -> Option<(FleetTuiInlineInputMode, String)> {
        self.inline_input
            .take()
            .map(|input| (input.mode, input.buffer))
    }

    pub(super) fn cancel_inline_input(&mut self) {
        let Some(input) = self.inline_input.take() else {
            return;
        };
        self.status_message = Some(match input.mode {
            FleetTuiInlineInputMode::AddProjectRepo => "Project add cancelled.".to_string(),
            FleetTuiInlineInputMode::SearchSessions => "Fleet search cancelled.".to_string(),
        });
    }

    pub(super) fn cycle_editor_action(&mut self) {
        self.proposal_notice = None;
        let Some(decision) = self.editor_decision.as_mut() else {
            self.status_message = Some("No editor proposal loaded. Press 'e' first.".to_string());
            return;
        };
        decision.action = decision.action.next();
        self.status_message = Some(format!(
            "Editor action set to {} for {}/{}.",
            decision.action.as_str(),
            decision.vm_name,
            decision.session_name
        ));
    }

    pub(super) fn cycle_new_session_agent(&mut self) {
        self.new_session_agent = self.new_session_agent.next();
        self.status_message = Some(format!(
            "New session agent set to {}.",
            self.new_session_agent.as_str()
        ));
    }

    pub(super) fn cycle_fleet_subview(&mut self, state: &FleetState) {
        self.fleet_subview = self.fleet_subview.next();
        self.sync_to_state(state);
        self.status_message = Some(format!("Fleet view set to {}.", self.fleet_subview.title()));
    }

    pub(super) fn navigate_back(&mut self) {
        self.tab = match self.tab {
            FleetTuiTab::Editor => FleetTuiTab::Detail,
            FleetTuiTab::NewSession => FleetTuiTab::Fleet,
            FleetTuiTab::Detail | FleetTuiTab::Projects => FleetTuiTab::Fleet,
            FleetTuiTab::Fleet => FleetTuiTab::Fleet,
        };
    }

    // ── T5: Per-session LRU capture cache helpers ──────────────────────────

    /// Store a capture output into the LRU cache.
    pub(super) fn put_capture(&self, vm_name: &str, session_name: &str, output: String) {
        let key = (vm_name.to_string(), session_name.to_string());
        if let Ok(mut cache) = self.capture_cache.lock() {
            cache.put(
                key,
                FleetDetailCapture {
                    vm_name: vm_name.to_string(),
                    session_name: session_name.to_string(),
                    output,
                },
            );
        }
    }

    /// Look up a capture from the LRU cache.  Falls back to the compatibility
    /// `detail_capture` field so that tests that pre-populate it still work.
    pub(super) fn get_capture(&self, vm_name: &str, session_name: &str) -> Option<String> {
        // Check LRU cache first.
        let key = (vm_name.to_string(), session_name.to_string());
        if let Ok(mut cache) = self.capture_cache.lock()
            && let Some(entry) = cache.get(&key)
        {
            return Some(entry.output.clone());
        }
        // Fall back to the compatibility single-entry field.
        self.detail_capture
            .as_ref()
            .filter(|c| c.vm_name == vm_name && c.session_name == session_name)
            .map(|c| c.output.clone())
    }

    /// Remove all cached captures (e.g. on state reset).
    #[allow(dead_code)]
    pub(super) fn clear_capture_cache(&self) {
        if let Ok(mut cache) = self.capture_cache.lock() {
            cache.clear();
        }
    }

    // ── T4: Background channel helpers ─────────────────────────────────────

    /// Send a command to the background worker (best-effort; ignores broken channel).
    pub(super) fn send_bg_cmd(&self, cmd: BackgroundCommand) {
        if let Some(tx) = &self.bg_tx {
            let _ = tx.send(cmd);
        }
    }

    /// Drain all pending background messages and apply them to state.
    pub(super) fn drain_bg_messages(&mut self) {
        let rx_arc = match self.bg_rx.as_ref() {
            Some(arc) => arc.clone(),
            None => return,
        };
        let Ok(rx) = rx_arc.try_lock() else { return };
        loop {
            match rx.try_recv() {
                Ok(BackgroundMessage::FastStatusUpdate(_state)) => {
                    // State update from background; the render loop re-collects
                    // state synchronously on each iteration, so we just note it.
                }
                Ok(BackgroundMessage::SlowCaptureUpdate {
                    vm_name,
                    session_name,
                    output,
                }) => {
                    self.put_capture(&vm_name, &session_name, output);
                }
                Ok(BackgroundMessage::SessionCreated { message }) => {
                    self.status_message = Some(message);
                    self.create_session_pending = false;
                    self.tab = FleetTuiTab::Fleet;
                }
                Ok(BackgroundMessage::Error(msg)) => {
                    self.status_message = Some(format!("[bg error] {msg}"));
                    self.create_session_pending = false;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.bg_rx = None;
                    break;
                }
            }
        }
    }

    // ── T3: Multiline proposal editor helpers ───────────────────────────────

    /// Enter the multiline editor, populating it with `initial_text`.
    pub(super) fn enter_multiline_editor(&mut self, initial_text: &str) {
        self.editor_lines = initial_text
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>();
        if self.editor_lines.is_empty() {
            self.editor_lines.push(String::new());
        }
        self.editor_cursor_row = self.editor_lines.len().saturating_sub(1);
        self.editor_active = true;
    }

    /// Insert a character at the end of the current cursor line.
    pub(super) fn editor_insert_char(&mut self, ch: char) {
        if !self.editor_active {
            return;
        }
        if ch == '\n' {
            let row = self.editor_cursor_row;
            self.editor_lines.insert(row + 1, String::new());
            self.editor_cursor_row += 1;
        } else {
            let row = self.editor_cursor_row;
            if let Some(line) = self.editor_lines.get_mut(row) {
                line.push(ch);
            }
        }
    }

    /// Delete the last character from the current cursor line (Backspace).
    pub(super) fn editor_backspace(&mut self) {
        if !self.editor_active {
            return;
        }
        let row = self.editor_cursor_row;
        if let Some(line) = self.editor_lines.get_mut(row)
            && !line.is_empty()
        {
            line.pop();
            return;
        }
        // Empty line — merge with previous.
        if row > 0 {
            self.editor_lines.remove(row);
            self.editor_cursor_row -= 1;
        }
    }

    /// Move cursor up one line.
    pub(super) fn editor_move_up(&mut self) {
        if self.editor_cursor_row > 0 {
            self.editor_cursor_row -= 1;
        }
    }

    /// Move cursor down one line.
    pub(super) fn editor_move_down(&mut self) {
        if self.editor_cursor_row + 1 < self.editor_lines.len() {
            self.editor_cursor_row += 1;
        }
    }

    /// Extract the editor content as a single string (lines joined with `\n`).
    pub(super) fn editor_content(&self) -> String {
        self.editor_lines.join("\n")
    }

    /// Save the editor content into `inline_input` buffer and exit editor.
    pub(super) fn editor_save(&mut self) {
        let content = self.editor_content();
        self.apply_inline_editor_input(&content);
        self.editor_active = false;
        self.editor_lines.clear();
        self.editor_cursor_row = 0;
    }

    /// Discard the editor content without saving.
    #[allow(dead_code)]
    pub(super) fn editor_discard(&mut self) {
        self.editor_active = false;
        self.editor_lines.clear();
        self.editor_cursor_row = 0;
    }

    // ── T6: Project management mode helpers ────────────────────────────────

    /// Transition project tab to Add sub-mode.
    pub(super) fn enter_project_add_mode(&mut self) {
        self.project_mode = ProjectManagementMode::Add;
        self.start_inline_project_input();
    }

    /// Transition project tab to Remove sub-mode (requires selected project).
    pub(super) fn enter_project_remove_mode(&mut self) {
        if self.selected_project_repo.is_none() {
            self.status_message = Some("No project selected to remove.".to_string());
            return;
        }
        self.project_mode = ProjectManagementMode::Remove;
    }

    /// Confirm project removal and return to List mode.
    pub(super) fn confirm_project_remove(&mut self) {
        // Actual file I/O is handled by run_tui_remove_project; here we just
        // transition the mode back to List so the TUI is consistent.
        self.project_mode = ProjectManagementMode::List;
    }

    /// Cancel project operation and return to List mode.
    pub(super) fn cancel_project_mode(&mut self) {
        self.project_mode = ProjectManagementMode::List;
    }

    pub(super) fn skip_selected_proposal(&mut self) {
        let Some(selected) = self.selected.as_ref() else {
            self.status_message = Some("No session selected to skip.".to_string());
            return;
        };

        let matches_selected = |decision: &SessionDecision| {
            decision.vm_name == selected.vm_name && decision.session_name == selected.session_name
        };

        let had_prepared = self.last_decision.as_ref().is_some_and(matches_selected)
            || self.editor_decision.as_ref().is_some_and(matches_selected);
        if !had_prepared {
            self.status_message = Some("No prepared proposal to skip.".to_string());
            return;
        }

        if self.last_decision.as_ref().is_some_and(matches_selected) {
            self.last_decision = None;
        }
        if self.editor_decision.as_ref().is_some_and(matches_selected) {
            self.editor_decision = None;
        }
        self.set_selected_proposal_notice("Proposal status", "Skipped.");
        self.tab = FleetTuiTab::Detail;
        self.status_message = Some("Skipped.".to_string());
    }
}
