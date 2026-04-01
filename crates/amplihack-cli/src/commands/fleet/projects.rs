use super::*;

#[derive(Debug)]
pub(super) struct FleetGraphSummary {
    pub(super) node_types: Vec<String>,
    pub(super) edge_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectInfo {
    pub(super) repo_url: String,
    pub(super) name: String,
    pub(super) github_identity: String,
    pub(super) priority: String,
    pub(super) notes: String,
    pub(super) vms: Vec<String>,
    pub(super) tasks_total: usize,
    pub(super) tasks_completed: usize,
    pub(super) tasks_failed: usize,
    pub(super) tasks_in_progress: usize,
    pub(super) prs_created: Vec<String>,
    pub(super) estimated_cost_usd: f64,
    pub(super) started_at: Option<String>,
    pub(super) last_activity: Option<String>,
}

impl ProjectInfo {
    pub(super) fn new(repo_url: &str, github_identity: &str, name: &str, priority: &str) -> Self {
        let inferred_name = if name.is_empty() {
            repo_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
        } else {
            name
        };

        Self {
            repo_url: repo_url.to_string(),
            name: inferred_name.to_string(),
            github_identity: github_identity.to_string(),
            priority: priority.to_string(),
            notes: String::new(),
            vms: Vec::new(),
            tasks_total: 0,
            tasks_completed: 0,
            tasks_failed: 0,
            tasks_in_progress: 0,
            prs_created: Vec::new(),
            estimated_cost_usd: 0.0,
            started_at: Some(now_isoformat()),
            last_activity: None,
        }
    }

    pub(super) fn completion_rate(&self) -> f64 {
        if self.tasks_total == 0 {
            0.0
        } else {
            self.tasks_completed as f64 / self.tasks_total as f64
        }
    }

    pub(super) fn to_json_value(&self) -> Value {
        serde_json::json!({
            "repo_url": self.repo_url,
            "name": self.name,
            "github_identity": self.github_identity,
            "priority": self.priority,
            "notes": self.notes,
            "vms": self.vms,
            "tasks_total": self.tasks_total,
            "tasks_completed": self.tasks_completed,
            "tasks_failed": self.tasks_failed,
            "tasks_in_progress": self.tasks_in_progress,
            "prs_created": self.prs_created,
            "estimated_cost_usd": self.estimated_cost_usd,
            "started_at": self.started_at,
            "last_activity": self.last_activity,
        })
    }

    pub(super) fn from_json_value(value: &Value) -> Option<Self> {
        Some(Self {
            repo_url: value.get("repo_url")?.as_str()?.to_string(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            github_identity: value
                .get("github_identity")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            priority: value
                .get("priority")
                .and_then(Value::as_str)
                .unwrap_or("medium")
                .to_string(),
            notes: value
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            vms: value
                .get("vms")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            tasks_total: value
                .get("tasks_total")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            tasks_completed: value
                .get("tasks_completed")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            tasks_failed: value
                .get("tasks_failed")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            tasks_in_progress: value
                .get("tasks_in_progress")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
            prs_created: value
                .get("prs_created")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            estimated_cost_usd: value
                .get("estimated_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            started_at: value
                .get("started_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            last_activity: value
                .get("last_activity")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

