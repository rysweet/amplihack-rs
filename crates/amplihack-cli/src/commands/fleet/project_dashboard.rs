use super::*;

#[derive(Debug, Clone)]
pub(super) struct FleetDashboardSummary {
    pub(super) projects: Vec<ProjectInfo>,
    pub(super) persist_path: Option<PathBuf>,
}

impl FleetDashboardSummary {
    pub(super) fn load_default() -> Result<Self> {
        Self::load(Some(default_dashboard_path()))
    }

    pub(super) fn load(persist_path: Option<PathBuf>) -> Result<Self> {
        let mut dashboard = Self {
            projects: Vec::new(),
            persist_path,
        };

        let Some(path) = dashboard.persist_path.clone() else {
            return Ok(dashboard);
        };
        if !path.exists() {
            return Ok(dashboard);
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Array(items)) => {
                dashboard.projects = items
                    .iter()
                    .filter_map(ProjectInfo::from_json_value)
                    .collect();
            }
            Ok(_) => {}
            Err(_) => {
                let backup = path.with_extension("json.bak");
                let _ = fs::copy(&path, &backup);
                bail!(
                    "failed to parse {} as fleet dashboard JSON; copied corrupt file to {}",
                    path.display(),
                    backup.display()
                );
            }
        }

        Ok(dashboard)
    }

    pub(super) fn add_project(
        &mut self,
        repo_url: &str,
        github_identity: &str,
        name: &str,
        priority: &str,
    ) -> usize {
        if let Some(index) = self
            .projects
            .iter()
            .position(|project| project.repo_url == repo_url || project.name == name)
        {
            return index;
        }

        self.projects
            .push(ProjectInfo::new(repo_url, github_identity, name, priority));
        self.projects.len() - 1
    }

    pub(super) fn add_project_and_save(
        &mut self,
        repo_url: &str,
        github_identity: &str,
        name: &str,
        priority: &str,
    ) -> Result<usize> {
        let index = self.add_project(repo_url, github_identity, name, priority);
        self.save()?;
        Ok(index)
    }

    pub(super) fn get_project(&self, name_or_url: &str) -> Option<&ProjectInfo> {
        self.projects
            .iter()
            .find(|project| project.name == name_or_url || project.repo_url == name_or_url)
    }

    pub(super) fn remove_project(&mut self, name_or_url: &str) -> bool {
        let Some(index) = self
            .projects
            .iter()
            .position(|project| project.name == name_or_url || project.repo_url == name_or_url)
        else {
            return false;
        };

        self.projects.remove(index);
        true
    }

    pub(super) fn remove_project_and_save(&mut self, name_or_url: &str) -> Result<bool> {
        let removed = self.remove_project(name_or_url);
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub(super) fn update_from_queue(&mut self, queue: &TaskQueue) -> Result<()> {
        let mut grouped = std::collections::BTreeMap::<String, Vec<&FleetTask>>::new();
        for task in &queue.tasks {
            let key = if task.repo_url.is_empty() {
                "unassigned".to_string()
            } else {
                task.repo_url.clone()
            };
            grouped.entry(key).or_default().push(task);
        }

        for (repo_url, tasks) in grouped {
            if repo_url == "unassigned" {
                continue;
            }
            let index = self.add_project(&repo_url, "", "", "medium");
            let project = &mut self.projects[index];
            project.tasks_total = tasks.len();
            project.tasks_completed = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Completed)
                .count();
            project.tasks_failed = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Failed)
                .count();
            project.tasks_in_progress = tasks
                .iter()
                .filter(|task| matches!(task.status, TaskStatus::Assigned | TaskStatus::Running))
                .count();
            project.prs_created = tasks
                .iter()
                .filter_map(|task| task.pr_url.clone())
                .collect();
            project.vms = tasks
                .iter()
                .filter_map(|task| task.assigned_vm.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            project.last_activity = Some(now_isoformat());
        }

        self.save()
    }

    pub(super) fn summary(&self) -> String {
        let total_tasks = self
            .projects
            .iter()
            .map(|project| project.tasks_total)
            .sum::<usize>();
        let total_completed = self
            .projects
            .iter()
            .map(|project| project.tasks_completed)
            .sum::<usize>();
        let total_prs = self
            .projects
            .iter()
            .map(|project| project.prs_created.len())
            .sum::<usize>();
        let total_cost = self
            .projects
            .iter()
            .map(|project| project.estimated_cost_usd)
            .sum::<f64>();
        let total_vms = self
            .projects
            .iter()
            .flat_map(|project| project.vms.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .len();

        let mut lines = vec![
            "=".repeat(60),
            "FLEET DASHBOARD".to_string(),
            "=".repeat(60),
            format!("  Projects: {}", self.projects.len()),
            format!("  VMs in use: {total_vms}"),
            format!("  Tasks: {total_completed}/{total_tasks} completed"),
            format!("  PRs created: {total_prs}"),
            format!("  Estimated cost: ${total_cost:.2}"),
            String::new(),
        ];

        for project in &self.projects {
            let identity = if project.github_identity.is_empty() {
                String::new()
            } else {
                format!(" ({})", project.github_identity)
            };
            lines.push(format!("  [{}]{}", project.name, identity));
            lines.push(format!(
                "    {} {}/{} tasks",
                Self::progress_bar(project.completion_rate(), 20),
                project.tasks_completed,
                project.tasks_total
            ));
            lines.push(format!(
                "    VMs: {} | PRs: {} | Cost: ${:.2}",
                if project.vms.is_empty() {
                    "none".to_string()
                } else {
                    project.vms.join(", ")
                },
                project.prs_created.len(),
                project.estimated_cost_usd
            ));
            if project.tasks_failed > 0 {
                lines.push(format!("    !! {} failed tasks", project.tasks_failed));
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    pub(super) fn progress_bar(ratio: f64, width: usize) -> String {
        let filled = (width as f64 * ratio).floor() as usize;
        let bar = "#".repeat(filled) + &"-".repeat(width.saturating_sub(filled));
        let pct = (ratio * 100.0).floor() as usize;
        format!("[{bar}] {pct}%")
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
        let payload = Value::Array(
            self.projects
                .iter()
                .map(ProjectInfo::to_json_value)
                .collect(),
        );
        let bytes =
            serde_json::to_vec_pretty(&payload).context("failed to serialize fleet dashboard")?;
        temp.write_all(&bytes)
            .with_context(|| format!("failed to write {}", path.display()))?;
        // SEC-PERM: tempfile guarantees 0o600 on Unix (O_CREAT with mode 0600, unaffected by umask > 0o177)
        temp.persist(path)
            .map_err(|err| err.error)
            .with_context(|| format!("failed to persist {}", path.display()))?;
        Ok(())
    }

    pub(super) fn project_repo_refs_default() -> Result<Vec<String>> {
        Self::load_default().map(|dashboard| {
            dashboard
                .projects
                .into_iter()
                .map(|project| project.repo_url)
                .collect()
        })
    }
}

