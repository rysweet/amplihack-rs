use super::*;

#[derive(Debug, Clone)]
pub(super) struct TaskQueue {
    pub(super) tasks: Vec<FleetTask>,
    pub(super) persist_path: Option<PathBuf>,
}

pub(super) trait FleetAdoptionBackend {
    fn record_adopted_session(&mut self, vm_name: &str, session: &mut AdoptedSession)
    -> Result<()>;
}

impl TaskQueue {
    pub(super) fn load_default() -> Result<Self> {
        Self::load(Some(default_queue_path()))
    }

    pub(super) fn load(persist_path: Option<PathBuf>) -> Result<Self> {
        let mut queue = Self {
            tasks: Vec::new(),
            persist_path,
        };

        let Some(path) = queue.persist_path.clone() else {
            return Ok(queue);
        };
        if !path.exists() {
            return Ok(queue);
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Array(items)) => {
                queue.tasks = items
                    .iter()
                    .filter_map(FleetTask::from_json_value)
                    .collect();
            }
            Ok(_) => {}
            Err(_) => {
                let backup = path.with_extension("json.bak");
                let _ = fs::copy(&path, &backup);
                bail!(
                    "failed to parse {} as fleet task queue JSON; copied corrupt file to {}",
                    path.display(),
                    backup.display()
                );
            }
        }

        Ok(queue)
    }

    pub(super) fn add_task(
        &mut self,
        prompt: &str,
        repo_url: &str,
        priority: TaskPriority,
        agent_command: &str,
        agent_mode: &str,
        max_turns: u32,
    ) -> Result<FleetTask> {
        let task = FleetTask::new(
            prompt,
            repo_url,
            priority,
            agent_command,
            agent_mode,
            max_turns,
        );
        self.tasks.push(task.clone());
        self.save()?;
        Ok(task)
    }

    pub(super) fn task_by_id_mut(&mut self, task_id: &str) -> Option<&mut FleetTask> {
        self.tasks
            .iter_mut()
            .find(|candidate| candidate.id == task_id)
    }

    pub(super) fn adopt_discovered_session(
        &mut self,
        vm_name: &str,
        session: &mut AdoptedSession,
    ) -> Result<()> {
        let prompt = if session.inferred_task.is_empty() {
            format!("Adopted session: {}", session.session_name)
        } else {
            session.inferred_task.clone()
        };
        let task = self.add_task(
            &prompt,
            &session.inferred_repo,
            TaskPriority::Medium,
            if session.agent_type.is_empty() {
                "claude"
            } else {
                &session.agent_type
            },
            "auto",
            DEFAULT_MAX_TURNS,
        )?;
        if let Some(saved_task) = self
            .tasks
            .iter_mut()
            .find(|candidate| candidate.id == task.id)
        {
            saved_task.assigned_vm = Some(vm_name.to_string());
            saved_task.assigned_session = Some(session.session_name.clone());
            saved_task.assigned_at = Some(now_isoformat());
            saved_task.started_at = Some(now_isoformat());
            saved_task.status = TaskStatus::Running;
        }
        self.save()?;

        session.task_id = Some(task.id);
        session.adopted_at = Some(now_isoformat());
        Ok(())
    }

    pub(super) fn mark_task_running(
        &mut self,
        task_id: &str,
        vm_name: &str,
        session_name: &str,
    ) -> Result<()> {
        if let Some(saved_task) = self.task_by_id_mut(task_id) {
            saved_task.assign(vm_name, session_name);
            saved_task.start();
        }
        self.save()
    }

    pub(super) fn mark_task_complete(&mut self, task_id: &str, result: &str) -> Result<()> {
        if let Some(saved_task) = self.task_by_id_mut(task_id) {
            saved_task.complete(result, None);
        }
        self.save()
    }

    pub(super) fn mark_task_failed(&mut self, task_id: &str, reason: &str) -> Result<()> {
        if let Some(saved_task) = self.task_by_id_mut(task_id) {
            saved_task.fail(reason);
        }
        self.save()
    }

    pub(super) fn requeue_task(&mut self, task_id: &str) -> Result<()> {
        if let Some(saved_task) = self.task_by_id_mut(task_id) {
            saved_task.requeue();
        }
        self.save()
    }

    pub(super) fn next_task(&self) -> Option<&FleetTask> {
        self.tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Queued)
            .min_by(|left, right| {
                left.priority
                    .rank()
                    .cmp(&right.priority.rank())
                    .then_with(|| left.created_at.cmp(&right.created_at))
            })
    }

    pub(super) fn active_tasks(&self) -> Vec<&FleetTask> {
        self.tasks
            .iter()
            .filter(|task| matches!(task.status, TaskStatus::Assigned | TaskStatus::Running))
            .collect()
    }

    pub(super) fn has_active_assignment(&self, vm_name: &str, session_name: &str) -> bool {
        self.tasks.iter().any(|task| {
            matches!(task.status, TaskStatus::Assigned | TaskStatus::Running)
                && task.assigned_vm.as_deref() == Some(vm_name)
                && task.assigned_session.as_deref() == Some(session_name)
        })
    }

    pub(super) fn summary(&self) -> String {
        let mut lines = vec![format!("Task Queue ({} tasks)", self.tasks.len())];
        for status in [
            TaskStatus::Queued,
            TaskStatus::Assigned,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
        ] {
            let tasks = self
                .tasks
                .iter()
                .filter(|task| task.status == status)
                .collect::<Vec<_>>();
            if tasks.is_empty() {
                continue;
            }

            lines.push(String::new());
            lines.push(format!("  {} ({}):", status.heading(), tasks.len()));
            for task in tasks {
                let vm = task
                    .assigned_vm
                    .as_deref()
                    .map(|vm_name| format!(" -> {vm_name}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "    [{}] {}: {}{}",
                    task.priority.short_label(),
                    task.id,
                    truncate_chars(&task.prompt, 60),
                    vm
                ));
            }
        }

        lines.join("\n")
    }

    pub(super) fn save(&self) -> Result<()> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                .with_context(|| format!("failed to create temp file for {}", path.display()))?;
        let payload = Value::Array(self.tasks.iter().map(FleetTask::to_json_value).collect());
        let bytes =
            serde_json::to_vec_pretty(&payload).context("failed to serialize task queue")?;
        temp.write_all(&bytes)
            .with_context(|| format!("failed to write {}", path.display()))?;
        // SEC-PERM: tempfile guarantees 0o600 on Unix (O_CREAT with mode 0600, unaffected by umask > 0o177)
        temp.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist {}", path.display()))?;
        Ok(())
    }
}

impl FleetAdoptionBackend for TaskQueue {
    fn record_adopted_session(
        &mut self,
        vm_name: &str,
        session: &mut AdoptedSession,
    ) -> Result<()> {
        self.adopt_discovered_session(vm_name, session)
    }
}

