use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectRegistryDoc {
    #[serde(default)]
    pub(super) project: BTreeMap<String, ProjectRegistryEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectRegistryEntry {
    #[serde(default)]
    pub(super) repo_url: String,
    #[serde(default)]
    pub(super) identity: String,
    #[serde(default = "default_project_priority")]
    pub(super) priority: String,
    #[serde(default)]
    pub(super) objectives: Vec<ProjectObjective>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ProjectObjective {
    pub(super) number: i64,
    pub(super) title: String,
    #[serde(default = "default_objective_state")]
    pub(super) state: String,
    #[serde(default)]
    pub(super) url: String,
}

pub(super) fn default_project_priority() -> String {
    "medium".to_string()
}

pub(super) fn default_objective_state() -> String {
    "open".to_string()
}

pub(super) fn load_projects_registry(
    path: &Path,
) -> Result<BTreeMap<String, ProjectRegistryEntry>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let doc = toml::from_str::<ProjectRegistryDoc>(&raw).map_err(|error| {
        let backup = path.with_extension("toml.bak");
        let _ = fs::copy(path, &backup);
        anyhow!(
            "failed to parse {} as fleet projects registry TOML; copied corrupt file to {}: {error}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(doc.project)
}

pub(super) fn load_default_projects_registry() -> Result<BTreeMap<String, ProjectRegistryEntry>> {
    load_projects_registry(&default_projects_path())
}

pub(super) fn save_projects_registry(
    projects: &BTreeMap<String, ProjectRegistryEntry>,
    path: &Path,
) -> Result<()> {
    for name in projects.keys() {
        validate_project_name(name)?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let doc = ProjectRegistryDoc {
        project: projects.clone(),
    };
    let rendered = toml::to_string(&doc).context("failed to serialize project registry")?;
    fs::write(path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub(super) fn save_default_projects_registry(
    projects: &BTreeMap<String, ProjectRegistryEntry>,
) -> Result<()> {
    save_projects_registry(projects, &default_projects_path())
}

pub(super) fn ensure_default_project_registry_entry(
    name: &str,
    entry: ProjectRegistryEntry,
) -> Result<bool> {
    let mut projects = load_default_projects_registry()?;
    if projects.contains_key(name) {
        return Ok(false);
    }
    projects.insert(name.to_string(), entry);
    save_default_projects_registry(&projects)?;
    Ok(true)
}

pub(super) fn validate_project_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid project name {name:?}: must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$");
    };
    if !first.is_ascii_alphanumeric()
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        bail!("Invalid project name {name:?}: must match ^[a-zA-Z0-9][a-zA-Z0-9_-]*$");
    }
    Ok(())
}

pub(super) fn render_project_list(dashboard: &FleetDashboardSummary) -> String {
    if dashboard.projects.is_empty() {
        return "No projects registered. Use 'fleet project add <repo_url>' to add one."
            .to_string();
    }

    let mut lines = vec![
        format!("Fleet Projects ({})", dashboard.projects.len()),
        "=".repeat(60),
    ];
    for project in &dashboard.projects {
        let prio_label = match project.priority.as_str() {
            "high" => "!!!",
            "low" => "!",
            _ => "!!",
        };
        lines.push(format!("  [{prio_label}] {}", project.name));
        lines.push(format!("      Repo: {}", project.repo_url));
        if !project.github_identity.is_empty() {
            lines.push(format!("      Identity: {}", project.github_identity));
        }
        lines.push(format!("      Priority: {}", project.priority));
        lines.push(format!(
            "      VMs: {} | Tasks: {}/{} | PRs: {}",
            project.vms.len(),
            project.tasks_completed,
            project.tasks_total,
            project.prs_created.len()
        ));
        if !project.notes.is_empty() {
            lines.push(format!("      Notes: {}", project.notes));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

impl FleetGraphSummary {
    pub(super) fn load_default() -> Result<Self> {
        Self::load(Some(default_graph_path()))
    }

    pub(super) fn load(persist_path: Option<PathBuf>) -> Result<Self> {
        let Some(path) = persist_path else {
            return Ok(Self {
                node_types: Vec::new(),
                edge_types: Vec::new(),
            });
        };
        if !path.exists() {
            return Ok(Self {
                node_types: Vec::new(),
                edge_types: Vec::new(),
            });
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let value = match serde_json::from_str::<Value>(&raw) {
            Ok(value) => value,
            Err(_) => {
                let backup = path.with_extension("json.bak");
                let _ = fs::copy(&path, &backup);
                bail!(
                    "failed to parse {} as fleet graph JSON; copied corrupt file to {}",
                    path.display(),
                    backup.display()
                );
            }
        };

        let node_types = value
            .get("nodes")
            .and_then(Value::as_object)
            .map(|nodes| {
                nodes
                    .values()
                    .filter_map(|node| node.get("type").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let edge_types = value
            .get("edges")
            .and_then(Value::as_array)
            .map(|edges| {
                edges
                    .iter()
                    .filter_map(|edge| edge.get("type").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Self {
            node_types,
            edge_types,
        })
    }

    pub(super) fn summary(&self) -> String {
        let mut node_counts = std::collections::BTreeMap::<String, usize>::new();
        for node_type in &self.node_types {
            *node_counts.entry(node_type.clone()).or_insert(0) += 1;
        }
        let mut edge_counts = std::collections::BTreeMap::<String, usize>::new();
        for edge_type in &self.edge_types {
            *edge_counts.entry(edge_type.clone()).or_insert(0) += 1;
        }

        let mut lines = vec![
            format!(
                "Fleet Graph: {} nodes, {} edges",
                self.node_types.len(),
                self.edge_types.len()
            ),
            format!(
                "  Nodes: {}",
                node_counts
                    .iter()
                    .map(|(kind, count)| format!("{kind}={count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!(
                "  Edges: {}",
                edge_counts
                    .iter()
                    .map(|(kind, count)| format!("{kind}={count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ];

        let conflicts = edge_counts.get("conflicts").copied().unwrap_or(0);
        if conflicts > 0 {
            lines.push(format!("  !! {conflicts} conflicts detected"));
        }

        lines.join("\n")
    }
}
