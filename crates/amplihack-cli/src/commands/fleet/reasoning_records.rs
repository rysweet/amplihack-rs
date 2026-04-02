use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SessionDecisionRecord {
    pub(super) vm: String,
    pub(super) session: String,
    pub(super) status: String,
    #[serde(default)]
    pub(super) branch: String,
    #[serde(default)]
    pub(super) pr: String,
    pub(super) action: String,
    pub(super) confidence: f64,
    pub(super) reasoning: String,
    #[serde(default)]
    pub(super) input_text: String,
    #[serde(default)]
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(super) project: String,
    #[serde(default)]
    pub(super) objectives: Vec<ProjectObjective>,
}

impl SessionDecisionRecord {
    pub(super) fn from_analysis(analysis: &SessionAnalysis) -> Self {
        Self {
            vm: analysis.context.vm_name.clone(),
            session: analysis.context.session_name.clone(),
            status: analysis.context.agent_status.as_str().to_string(),
            branch: analysis.context.git_branch.clone(),
            pr: analysis.context.pr_url.clone(),
            action: analysis.decision.action.as_str().to_string(),
            confidence: analysis.decision.confidence,
            reasoning: analysis.decision.reasoning.clone(),
            input_text: analysis.decision.input_text.clone(),
            error: None,
            project: analysis.context.project_name.clone(),
            objectives: analysis.context.project_objectives.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct SessionExecutionRecord {
    pub(super) vm: String,
    pub(super) session: String,
    pub(super) action: String,
    pub(super) executed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

impl SessionExecutionRecord {
    pub(super) fn executed(record: &SessionDecisionRecord) -> Self {
        Self {
            vm: record.vm.clone(),
            session: record.session.clone(),
            action: record.action.clone(),
            executed: true,
            error: None,
        }
    }

    pub(super) fn skipped(record: &SessionDecisionRecord, error: Option<String>) -> Self {
        Self {
            vm: record.vm.clone(),
            session: record.session.clone(),
            action: record.action.clone(),
            executed: false,
            error,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct LastScoutSnapshot {
    pub(super) timestamp: String,
    pub(super) running_vms: usize,
    pub(super) total_sessions: usize,
    pub(super) adopted_count: usize,
    pub(super) skip_adopt: bool,
    pub(super) decisions: Vec<SessionDecisionRecord>,
    pub(super) session_statuses: BTreeMap<String, String>,
}

impl LastScoutSnapshot {
    pub(super) fn new(
        running_vms: usize,
        total_sessions: usize,
        adopted_count: usize,
        skip_adopt: bool,
        decisions: Vec<SessionDecisionRecord>,
        sessions: &[DiscoveredSession],
    ) -> Self {
        let session_statuses = sessions
            .iter()
            .map(|session| {
                (
                    format!("{}/{}", session.vm_name, session.session_name),
                    session.status.as_str().to_string(),
                )
            })
            .collect();
        Self {
            timestamp: now_isoformat(),
            running_vms,
            total_sessions,
            adopted_count,
            skip_adopt,
            decisions,
            session_statuses,
        }
    }

    pub(super) fn save(&self, path: &Path) -> Result<()> {
        let payload = serde_json::to_value(self).context("failed to serialize scout snapshot")?;
        write_json_file(path, &payload)
    }

    pub(super) fn save_default(&self) -> Result<()> {
        self.save(&default_last_scout_path())
    }
}

pub(super) fn discover_dry_run_sessions(
    azlin: &Path,
    vm_names: &[String],
) -> Result<Vec<DryRunSession>> {
    let mut state = FleetState::new(azlin.to_path_buf());
    let existing_vms = configured_existing_vms();
    let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
    state.exclude_vms(&existing_refs);
    state.refresh();

    let target_vms = if vm_names.is_empty() {
        state
            .managed_vms()
            .into_iter()
            .filter(|vm| vm.is_running())
            .map(|vm| vm.name.clone())
            .collect::<Vec<_>>()
    } else {
        vm_names.to_vec()
    };

    if target_vms.is_empty() {
        println!("No managed VMs found. Use 'fleet adopt' to bring VMs under management.");
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for vm in &state.vms {
        if target_vms.iter().any(|name| name == &vm.name) {
            for session in &vm.tmux_sessions {
                sessions.push(DryRunSession {
                    vm_name: vm.name.clone(),
                    session_name: session.session_name.clone(),
                });
            }
        }
    }

    if sessions.is_empty() {
        for vm_name in &target_vms {
            println!("Scanning {vm_name} for sessions...");
            for session in state.poll_tmux_sessions(vm_name) {
                sessions.push(DryRunSession {
                    vm_name: vm_name.clone(),
                    session_name: session.session_name,
                });
            }
        }
    }

    if sessions.is_empty() {
        println!("No sessions found on target VMs.");
    }

    Ok(sessions)
}

pub(super) fn discover_scout_sessions(
    azlin: &Path,
    vm: Option<&str>,
    session_target: Option<&str>,
    exclude: bool,
) -> Result<Option<ScoutDiscovery>> {
    let mut state = FleetState::new(azlin.to_path_buf());
    if exclude {
        let existing_vms = configured_existing_vms();
        let existing_refs: Vec<&str> = existing_vms.iter().map(String::as_str).collect();
        state.exclude_vms(&existing_refs);
    }
    state.refresh();

    let mut target_vm = vm.map(str::to_string);
    let mut session_filter = None::<String>;
    if let Some(target) = session_target {
        let (vm_name, session_name) = parse_session_target(target);
        if vm_name.is_some() {
            target_vm = vm_name;
        }
        session_filter = Some(session_name);
    }

    let mut all_vms = state.vms.clone();
    if let Some(target) = target_vm.as_deref() {
        all_vms.retain(|candidate| candidate.name == target);
        if all_vms.is_empty() {
            println!("VM not found: {target}");
            return Ok(None);
        }
    }

    let mut running_vms = all_vms
        .iter()
        .filter(|candidate| candidate.is_running() && !candidate.tmux_sessions.is_empty())
        .cloned()
        .collect::<Vec<_>>();

    if let Some(filter) = session_filter.as_deref() {
        for vm_info in &mut running_vms {
            vm_info
                .tmux_sessions
                .retain(|session| session.session_name == filter);
        }
        running_vms.retain(|vm_info| !vm_info.tmux_sessions.is_empty());
        if running_vms.is_empty() {
            println!("Session not found: {}", session_target.unwrap_or(filter));
            return Ok(None);
        }
    }

    let mut observer = FleetObserver::new(azlin.to_path_buf());
    let mut sessions = Vec::<DiscoveredSession>::new();
    for vm_info in &mut running_vms {
        for session in &mut vm_info.tmux_sessions {
            let observation = observer.observe_session(&vm_info.name, &session.session_name)?;
            session.agent_status = observation.status;
            session.last_output = observation.last_output_lines.join("\n");
            sessions.push(DiscoveredSession {
                vm_name: vm_info.name.clone(),
                session_name: session.session_name.clone(),
                status: observation.status,
                cached_tmux_capture: session.last_output.clone(),
            });
        }
    }

    println!(
        "Found {} VMs, {} sessions on {} running VMs",
        all_vms.len(),
        sessions.len(),
        running_vms.len()
    );

    if sessions.is_empty() {
        println!("No running VMs with sessions found.");
        return Ok(None);
    }

    Ok(Some(ScoutDiscovery {
        all_vm_count: all_vms.len(),
        running_vm_count: running_vms.len(),
        sessions,
    }))
}

pub(super) fn generate_task_id(seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
            .to_string(),
    );
    hasher.update(std::process::id().to_string());
    hasher.update(seed.as_bytes());
    let digest = hasher.finalize();
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
