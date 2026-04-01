use super::*;

pub(super) fn now_isoformat() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.f").to_string()
}

pub(super) fn render_copilot_status(lock_dir: &Path) -> Result<String> {
    let lock_file = lock_dir.join(".lock_active");
    let goal_file = lock_dir.join(".lock_goal");

    if !lock_file.exists() {
        return Ok("Copilot: not active".to_string());
    }

    if goal_file.exists() {
        let goal_text = fs::read_to_string(&goal_file)
            .with_context(|| format!("failed to read {}", goal_file.display()))?;
        return Ok(format!("Copilot: active\nGoal: {}", goal_text.trim()));
    }

    Ok("Copilot: active (no goal)".to_string())
}

#[derive(Debug, Clone)]
pub(super) struct CopilotLogReport {
    pub(super) rendered: String,
    pub(super) malformed_entries: usize,
}

pub(super) fn read_copilot_log(log_dir: &Path, tail: usize) -> Result<CopilotLogReport> {
    let decisions_file = log_dir.join("decisions.jsonl");
    if !decisions_file.exists() {
        return Ok(CopilotLogReport {
            rendered: "No decisions recorded.".to_string(),
            malformed_entries: 0,
        });
    }

    let text = fs::read_to_string(&decisions_file)
        .with_context(|| format!("failed to read {}", decisions_file.display()))?;
    if text.trim().is_empty() {
        return Ok(CopilotLogReport {
            rendered: "No decisions recorded.".to_string(),
            malformed_entries: 0,
        });
    }

    let mut malformed_entries = 0usize;
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => entries.push(value),
            Err(_) => malformed_entries += 1,
        }
    }

    if entries.is_empty() {
        return Ok(CopilotLogReport {
            rendered: "No decisions recorded.".to_string(),
            malformed_entries,
        });
    }

    let start = if tail > 0 && entries.len() > tail {
        entries.len() - tail
    } else {
        0
    };

    let mut lines = Vec::new();
    for entry in &entries[start..] {
        let ts = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let action = entry.get("action").and_then(Value::as_str).unwrap_or("?");
        let confidence = value_to_inline_string(entry.get("confidence"));
        lines.push(format!("[{ts}] {action} (confidence={confidence})"));
        let reasoning = entry.get("reasoning").and_then(Value::as_str).unwrap_or("");
        if !reasoning.is_empty() {
            lines.push(format!("  {reasoning}"));
        }
    }

    Ok(CopilotLogReport {
        rendered: lines.join("\n"),
        malformed_entries,
    })
}

pub(super) fn value_to_inline_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::Null) | None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string(),
    }
}

pub(super) fn render_snapshot(state: &FleetState, observer: &mut FleetObserver) -> Result<String> {
    let managed = state.managed_vms();
    let mut lines = vec![
        format!("Fleet Snapshot ({} managed VMs)", managed.len()),
        "=".repeat(60),
    ];

    for vm in managed.into_iter().filter(|vm| vm.is_running()) {
        lines.push(String::new());
        lines.push(format!("[{}] ({})", vm.name, vm.region));
        if vm.tmux_sessions.is_empty() {
            lines.push("  No sessions".to_string());
            continue;
        }

        for session in &vm.tmux_sessions {
            let observation = observer.observe_session(&vm.name, &session.session_name)?;
            lines.push(format!(
                "  [{}] {}",
                observation.status.as_str(),
                session.session_name
            ));
            for line in observation
                .last_output_lines
                .iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                lines.push(format!("    | {}", truncate_chars(line, 100)));
            }
        }
    }

    Ok(lines.join("\n"))
}

pub(super) fn render_observe(vm: &VmInfo, observer: &FleetObserver) -> Result<String> {
    let results = observer.observe_all(&vm.tmux_sessions)?;
    let mut lines = Vec::new();
    for observation in results {
        lines.push(String::new());
        lines.push(format!("  Session: {}", observation.session_name));
        lines.push(format!(
            "  Status: {} (confidence: {:.0}%)",
            observation.status.as_str(),
            observation.confidence * 100.0
        ));
        if !observation.matched_pattern.is_empty() {
            lines.push(format!("  Pattern: {}", observation.matched_pattern));
        }
        if !observation.last_output_lines.is_empty() {
            lines.push("  Last output:".to_string());
            for line in observation
                .last_output_lines
                .iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                lines.push(format!("    | {}", truncate_chars(line, 120)));
            }
        }
    }

    Ok(lines.join("\n"))
}

pub(super) fn perceive_fleet_state(azlin_path: PathBuf) -> Result<FleetState> {
    collect_observed_fleet_state(&azlin_path, DEFAULT_CAPTURE_LINES)
}

pub(super) fn render_report(state: &FleetState, queue: &TaskQueue) -> String {
    [
        "=".repeat(60),
        "Fleet Admiral Report — Cycle 0".to_string(),
        "=".repeat(60),
        String::new(),
        state.summary(),
        String::new(),
        queue.summary(),
        String::new(),
        "Admiral log: 0 actions recorded".to_string(),
        String::new(),
        "Stats: 0 actions, 0 successes, 0 failures".to_string(),
    ]
    .join("\n")
}
