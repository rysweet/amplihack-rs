use super::*;

pub(super) fn default_queue_path() -> PathBuf {
    fleet_home_dir().join("task_queue.json")
}

pub(super) fn default_dashboard_path() -> PathBuf {
    fleet_home_dir().join("dashboard.json")
}

pub(super) fn default_log_dir() -> PathBuf {
    fleet_home_dir().join("logs")
}

pub(super) fn default_coordination_dir() -> PathBuf {
    fleet_home_dir().join("coordination")
}

pub(super) fn default_projects_path() -> PathBuf {
    fleet_home_dir().join("projects.toml")
}

pub(super) fn default_graph_path() -> PathBuf {
    fleet_home_dir().join("graph.json")
}

pub(super) fn default_copilot_lock_dir() -> PathBuf {
    claude_project_dir()
        .join(".claude")
        .join("runtime")
        .join("locks")
}

pub(super) fn default_copilot_log_dir() -> PathBuf {
    claude_project_dir()
        .join(".claude")
        .join("runtime")
        .join("copilot-decisions")
}

pub(super) fn fleet_home_dir() -> PathBuf {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".amplihack").join("fleet")
}

pub(super) fn claude_project_dir() -> PathBuf {
    env::var_os("CLAUDE_PROJECT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

pub(super) fn shell_single_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub(super) fn first_matching_pattern(patterns: &[&str], text: &str, multiline: bool) -> Option<String> {
    patterns.iter().find_map(|pattern| {
        RegexBuilder::new(pattern)
            .case_insensitive(true)
            .multi_line(multiline)
            .build()
            .ok()
            .filter(|regex: &Regex| regex.is_match(text))
            .map(|_| (*pattern).to_string())
    })
}

pub(super) fn auth_files_for_service(
    service: &str,
) -> Option<&'static [(&'static str, &'static str, &'static str)]> {
    match service {
        "github" => Some(AUTH_GITHUB_FILES),
        "azure" => Some(AUTH_AZURE_FILES),
        "claude" => Some(AUTH_CLAUDE_FILES),
        _ => None,
    }
}

pub(super) fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest),
        None => PathBuf::from(path),
    }
}

pub(super) fn parse_session_target(session_target: &str) -> (Option<String>, String) {
    if let Some((vm_name, session_name)) = session_target.split_once(':') {
        let vm_name = vm_name.trim();
        let session_name = session_name.trim().to_string();
        return (
            (!vm_name.is_empty()).then(|| vm_name.to_string()),
            session_name,
        );
    }
    (None, session_target.trim().to_string())
}

pub(super) fn load_previous_scout(
    path: &Path,
) -> Result<(BTreeMap<String, String>, Vec<SessionDecisionRecord>)> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let statuses = value
        .get("session_statuses")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let decisions = value
        .get("decisions")
        .cloned()
        .map(serde_json::from_value::<Vec<SessionDecisionRecord>>)
        .transpose()
        .context("failed to decode previous scout decisions")?
        .unwrap_or_default();
    Ok((statuses, decisions))
}

pub(super) fn load_default_previous_scout() -> Result<(BTreeMap<String, String>, Vec<SessionDecisionRecord>)> {
    load_previous_scout(&default_last_scout_path())
}

pub(super) fn write_json_file(path: &Path, payload: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let rendered = serde_json::to_vec_pretty(payload).context("failed to encode json payload")?;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
        .with_context(|| format!("failed to create temp file for {}", path.display()))?;
    temp.write_all(&rendered)
        .with_context(|| format!("failed to write {}", path.display()))?;
    // SEC-PERM: tempfile guarantees 0o600 on Unix (O_CREAT with mode 0600, unaffected by umask > 0o177)
    temp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("failed to persist {}", path.display()))?;
    Ok(())
}

pub(super) fn render_scout_report(
    decisions: &[SessionDecisionRecord],
    all_vm_count: usize,
    running_vm_count: usize,
    adopted_count: usize,
    skip_adopt: bool,
) -> String {
    let mut action_counts = BTreeMap::<String, usize>::new();
    for decision in decisions {
        *action_counts.entry(decision.action.clone()).or_insert(0) += 1;
    }

    let mut lines = vec![
        "=".repeat(60),
        "FLEET SCOUT REPORT".to_string(),
        "=".repeat(60),
        format!("VMs discovered: {all_vm_count}"),
        format!("Running VMs: {running_vm_count}"),
        format!("Sessions analyzed: {}", decisions.len()),
        if skip_adopt {
            "Adoption: skipped".to_string()
        } else {
            format!("Adopted sessions: {adopted_count}")
        },
    ];

    if !action_counts.is_empty() {
        lines.push("Actions:".to_string());
        for (action, count) in action_counts {
            lines.push(format!("  {action}: {count}"));
        }
    }
    lines.push(String::new());

    for decision in decisions {
        let status_suffix = if decision.status.is_empty() {
            String::new()
        } else {
            format!(" [{}]", decision.status)
        };
        lines.push(format!(
            "  {}/{}{} -> {} ({:.0}%)",
            decision.vm,
            decision.session,
            status_suffix,
            decision.action,
            decision.confidence * 100.0
        ));
        if !decision.branch.is_empty() {
            lines.push(format!("    Branch: {}", decision.branch));
        }
        if !decision.pr.is_empty() {
            lines.push(format!("    PR: {}", decision.pr));
        }
        if !decision.project.is_empty() {
            lines.push(format!("    Project: {}", decision.project));
        }
        if let Some(error) = &decision.error {
            lines.push(format!("    ERROR: {error}"));
        } else {
            lines.push(format!("    Reason: {}", decision.reasoning));
            if !decision.input_text.is_empty() {
                lines.push(format!(
                    "    Input: {}",
                    truncate_chars(&decision.input_text.replace('\n', "\\n"), 120)
                ));
            }
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

pub(super) fn render_advance_report(
    decisions: &[SessionDecisionRecord],
    executed: &[SessionExecutionRecord],
) -> String {
    let mut action_counts = BTreeMap::<String, usize>::new();
    for decision in decisions {
        *action_counts.entry(decision.action.clone()).or_insert(0) += 1;
    }

    let mut lines = vec![
        "=".repeat(60),
        "FLEET ADVANCE REPORT".to_string(),
        "=".repeat(60),
        format!("Sessions analyzed: {}", decisions.len()),
    ];
    for (action, count) in action_counts {
        lines.push(format!("  {action}: {count}"));
    }
    lines.push(String::new());

    for execution in executed {
        let label = if let Some(error) = &execution.error {
            format!("[ERROR] {error}")
        } else if execution.executed {
            "[OK]".to_string()
        } else {
            "[SKIPPED]".to_string()
        };
        lines.push(format!(
            "  {label} {}/{} -> {}",
            execution.vm, execution.session, execution.action
        ));
    }

    lines.join("\n")
}


