use super::*;

pub(super) struct FleetAdmiral {
    pub(super) task_queue: TaskQueue,
    pub(super) azlin_path: PathBuf,
    pub(super) poll_interval_seconds: u64,
    pub(super) max_agents_per_vm: usize,
    pub(super) fleet_state: FleetState,
    pub(super) observer: FleetObserver,
    pub(super) auth: AuthPropagator,
    pub(super) log: DirectorLog,
    pub(super) exclude_vms: Vec<String>,
    pub(super) cycle_count: usize,
    pub(super) missing_session_counts: BTreeMap<String, usize>,
    pub(super) stats: AdmiralStats,
    pub(super) coordination_dir: PathBuf,
}

impl FleetAdmiral {
    pub(super) fn new(
        azlin_path: PathBuf,
        task_queue: TaskQueue,
        log_dir: Option<PathBuf>,
    ) -> Result<Self> {
        if let Some(dir) = &log_dir {
            fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        let log = DirectorLog {
            persist_path: log_dir.map(|dir| dir.join("admiral_log.json")),
            ..DirectorLog::default()
        };
        Ok(Self {
            task_queue,
            fleet_state: FleetState::new(azlin_path.clone()),
            observer: FleetObserver::new(azlin_path.clone()),
            auth: AuthPropagator::new(azlin_path.clone()),
            azlin_path,
            poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
            max_agents_per_vm: DEFAULT_MAX_AGENTS_PER_VM,
            log,
            exclude_vms: Vec::new(),
            cycle_count: 0,
            missing_session_counts: BTreeMap::new(),
            stats: AdmiralStats::default(),
            coordination_dir: default_coordination_dir(),
        })
    }

    pub(super) fn exclude_vms(&mut self, vm_names: &[&str]) {
        self.exclude_vms
            .extend(vm_names.iter().map(|name| (*name).to_string()));
        self.fleet_state.exclude_vms(vm_names);
    }

    pub(super) fn run_once(&mut self) -> Result<Vec<DirectorAction>> {
        self.cycle_count += 1;
        self.perceive()?;
        let actions = self.reason()?;
        let results = self.act(&actions)?;
        self.learn(&results);
        Ok(actions)
    }

    pub(super) fn run_loop(&mut self, max_cycles: u32) -> Result<()> {
        let mut cycle = 0u32;
        let mut consecutive_failures = 0usize;

        loop {
            cycle += 1;
            if max_cycles > 0 && cycle > max_cycles {
                break;
            }

            match self.run_once() {
                Ok(_) => consecutive_failures = 0,
                Err(error) => {
                    consecutive_failures += 1;
                    eprintln!(
                        "Admiral cycle error ({}/5): {}",
                        consecutive_failures, error
                    );
                    if consecutive_failures >= 5 {
                        eprintln!("CIRCUIT BREAKER: 5 consecutive failures. Stopping admiral.");
                        break;
                    }
                }
            }

            if self.task_queue.next_task().is_none() && self.task_queue.active_tasks().is_empty() {
                break;
            }

            thread::sleep(Duration::from_secs(self.poll_interval_seconds));
        }

        Ok(())
    }

    pub(super) fn perceive(&mut self) -> Result<()> {
        self.fleet_state.refresh();
        let excluded = self.exclude_vms.clone();
        for vm in &mut self.fleet_state.vms {
            if !vm.is_running() || excluded.iter().any(|name| name == &vm.name) {
                continue;
            }
            for session in &mut vm.tmux_sessions {
                let observation = self
                    .observer
                    .observe_session(&vm.name, &session.session_name)?;
                session.agent_status = observation.status;
                session.last_output = observation.last_output_lines.join("\n");
            }
        }
        Ok(())
    }

    pub(super) fn reason(&mut self) -> Result<Vec<DirectorAction>> {
        self.write_coordination_files()?;

        let mut actions = Vec::new();
        actions.extend(self.lifecycle_actions());
        actions.extend(self.preemption_actions());
        actions.extend(self.batch_assign_actions(&actions));
        self.task_queue.save()?;
        Ok(actions)
    }

    pub(super) fn lifecycle_actions(&mut self) -> Vec<DirectorAction> {
        let active_keys = self
            .task_queue
            .active_tasks()
            .iter()
            .filter_map(|task| {
                Some(format!(
                    "{}:{}",
                    task.assigned_vm.as_deref()?,
                    task.assigned_session.as_deref()?
                ))
            })
            .collect::<HashSet<_>>();
        self.missing_session_counts
            .retain(|key, _| active_keys.contains(key));

        let mut actions = Vec::new();
        let active_tasks = self
            .task_queue
            .active_tasks()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        for task in active_tasks {
            let (Some(vm_name), Some(session_name)) =
                (task.assigned_vm.clone(), task.assigned_session.clone())
            else {
                continue;
            };

            let Some(vm) = self.fleet_state.get_vm(&vm_name) else {
                continue;
            };
            let session = vm
                .tmux_sessions
                .iter()
                .find(|candidate| candidate.session_name == session_name);
            if let Some(session) = session {
                let key = format!("{vm_name}:{session_name}");
                self.missing_session_counts.remove(&key);
                match session.agent_status {
                    AgentStatus::Completed => actions.push(DirectorAction::new(
                        ActionType::MarkComplete,
                        Some(task.clone()),
                        Some(vm_name),
                        Some(session_name),
                        "Agent completed successfully",
                    )),
                    AgentStatus::Error | AgentStatus::Shell | AgentStatus::NoSession => actions
                        .push(DirectorAction::new(
                            ActionType::MarkFailed,
                            Some(task.clone()),
                            Some(vm_name),
                            Some(session_name),
                            format!("Agent error: {}", truncate_chars(&session.last_output, 200)),
                        )),
                    AgentStatus::Stuck if !task.protected => actions.push(DirectorAction::new(
                        ActionType::ReassignTask,
                        Some(task.clone()),
                        Some(vm_name),
                        Some(session_name),
                        "Agent appears stuck",
                    )),
                    _ => {}
                }
                continue;
            }

            let key = format!("{vm_name}:{session_name}");
            let next_count = self.missing_session_counts.get(&key).copied().unwrap_or(0) + 1;
            if next_count >= 2 {
                self.missing_session_counts.remove(&key);
                actions.push(DirectorAction::new(
                    ActionType::MarkFailed,
                    Some(task),
                    Some(vm_name),
                    Some(session_name),
                    "Session no longer exists (missing 2+ cycles)",
                ));
            } else {
                self.missing_session_counts.insert(key, next_count);
            }
        }

        actions
    }

    pub(super) fn preemption_actions(&self) -> Vec<DirectorAction> {
        let critical_queued = self
            .task_queue
            .tasks
            .iter()
            .filter(|task| {
                task.status == TaskStatus::Queued && task.priority == TaskPriority::Critical
            })
            .cloned()
            .collect::<Vec<_>>();
        if critical_queued.is_empty() || !self.fleet_state.idle_vms().is_empty() {
            return Vec::new();
        }

        let mut running = self
            .task_queue
            .active_tasks()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        running.sort_by_key(|t| std::cmp::Reverse(t.priority.rank()));

        let mut actions = Vec::new();
        for critical_task in critical_queued {
            if running.is_empty() {
                break;
            }
            let victim = running.remove(0);
            if victim.priority.rank() <= critical_task.priority.rank() {
                break;
            }
            if victim.protected {
                continue;
            }
            actions.push(DirectorAction::new(
                ActionType::ReassignTask,
                Some(victim.clone()),
                victim.assigned_vm.clone(),
                victim.assigned_session.clone(),
                format!("Preempted for CRITICAL task {}", critical_task.id),
            ));
        }

        actions
    }

    pub(super) fn batch_assign_actions(
        &self,
        prior_actions: &[DirectorAction],
    ) -> Vec<DirectorAction> {
        let mut queued = self
            .task_queue
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Queued)
            .cloned()
            .collect::<Vec<_>>();
        queued.sort_by(|left, right| {
            left.priority
                .rank()
                .cmp(&right.priority.rank())
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
        if queued.is_empty() {
            return Vec::new();
        }

        let mut capacity = BTreeMap::<String, usize>::new();
        for vm in self.fleet_state.managed_vms() {
            if !vm.is_running() {
                continue;
            }
            let mut used = vm.active_agents();
            used += prior_actions
                .iter()
                .filter(|action| {
                    action.action_type == ActionType::StartAgent
                        && action.vm_name.as_deref() == Some(vm.name.as_str())
                })
                .count();
            if self.max_agents_per_vm > used {
                capacity.insert(vm.name.clone(), self.max_agents_per_vm - used);
            }
        }
        if capacity.is_empty() {
            return Vec::new();
        }

        let mut actions = Vec::new();
        for task in queued {
            let Some((best_vm, remaining)) = capacity
                .iter()
                .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
                .map(|(name, remaining)| (name.clone(), *remaining))
            else {
                break;
            };

            actions.push(DirectorAction::new(
                ActionType::StartAgent,
                Some(task.clone()),
                Some(best_vm.clone()),
                Some(format!("fleet-{}", task.id)),
                format!("Batch assign: {} task", task.priority.as_name()),
            ));

            if remaining <= 1 {
                capacity.remove(&best_vm);
            } else {
                capacity.insert(best_vm, remaining - 1);
            }
        }

        actions
    }

    pub(super) fn act(
        &mut self,
        actions: &[DirectorAction],
    ) -> Result<Vec<(DirectorAction, String)>> {
        let mut results = Vec::new();
        for action in actions {
            let outcome = match self.execute_action(action) {
                Ok(outcome) => outcome,
                Err(error) => format!("ERROR: {error:#}"),
            };
            self.log.record(action, &outcome)?;
            results.push((action.clone(), outcome));
        }
        Ok(results)
    }
}
