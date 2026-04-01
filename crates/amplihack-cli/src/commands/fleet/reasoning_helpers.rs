use super::*;

pub(super) fn find_reasoner_binary() -> Option<PathBuf> {
    if let Ok(path) = env::var("AMPLIHACK_FLEET_REASONER_BINARY_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    if let Ok(info) = BinaryFinder::find("claude") {
        return Some(info.path);
    }

    if let Ok(path) = env::var("RUSTYCLAWD_PATH") {
        let path = PathBuf::from(path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    find_binary("claude-code")
}

pub(super) fn match_project(repo_url: &str) -> Result<(String, Vec<ProjectObjective>)> {
    let projects = load_default_projects_registry()?;
    for (name, project) in projects {
        if !project.repo_url.is_empty()
            && project.repo_url.trim_end_matches('/') == repo_url.trim_end_matches('/')
        {
            return Ok((name, project.objectives));
        }
    }
    Ok((String::new(), Vec::new()))
}

pub(super) fn gather_session_context(
    azlin_path: &Path,
    vm_name: &str,
    session_name: &str,
    task_prompt: &str,
    project_priorities: &str,
    cached_tmux_capture: Option<&str>,
) -> Result<SessionContext> {
    let mut context = SessionContext::new(vm_name, session_name, task_prompt, project_priorities)?;
    let quoted_session = shell_single_quote(session_name);
    let gather_cmd = format!(
        concat!(
            "echo \"===TMUX===\"; ",
            "tmux capture-pane -t {session} -p -S - 2>/dev/null || echo 'NO_SESSION'; ",
            "echo \"===CWD===\"; ",
            "CWD=$(tmux display-message -t {session} -p \"#{{pane_current_path}}\" 2>/dev/null); ",
            "echo \"$CWD\"; ",
            "echo \"===GIT===\"; ",
            "if [ -n \"$CWD\" ] && [ -d \"$CWD/.git\" ]; then ",
            "cd \"$CWD\"; ",
            "echo \"BRANCH:$(git branch --show-current 2>/dev/null)\"; ",
            "echo \"REMOTE:$(git remote get-url origin 2>/dev/null)\"; ",
            "echo \"MODIFIED:$(git diff --name-only HEAD 2>/dev/null | head -10 | tr '\\n' ',')\"; ",
            "PRURL=$(gh pr list --head \"$(git branch --show-current 2>/dev/null)\" --json url --jq \".[]|.url\" 2>/dev/null | head -1); ",
            "if [ -n \"$PRURL\" ]; then echo \"PR_URL:$PRURL\"; fi; ",
            "fi; ",
            "echo \"===TRANSCRIPT===\"; ",
            "if [ -n \"$CWD\" ]; then ",
            "PKEY=$(echo \"$CWD\" | sed \"s|/|-|g\"); ",
            "JSONL=$(ls -t \"$HOME/.claude/projects/$PKEY/\"*.jsonl 2>/dev/null | head -1); ",
            "if [ -n \"$JSONL\" ]; then ",
            "MSGS=$(grep -E '\"type\":\"(user|assistant)\"' \"$JSONL\" 2>/dev/null | grep -oP '\"text\":\"[^\"]*\"' | sed 's/\"text\":\"//;s/\"$//' | grep -v '^$'); ",
            "TOTAL=$(echo \"$MSGS\" | wc -l); ",
            "echo \"TRANSCRIPT_LINES:$TOTAL\"; ",
            "echo \"---EARLY---\"; ",
            "echo \"$MSGS\" | head -50; ",
            "echo \"---RECENT---\"; ",
            "echo \"$MSGS\" | tail -200; ",
            "fi; fi; ",
            "echo \"===HEALTH===\"; ",
            "MEM=$(free -m 2>/dev/null | grep Mem | awk '{{printf \"%.0f\", $3/$2*100}}'); ",
            "DISK=$(df -h / 2>/dev/null | tail -1 | awk '{{print $5}}' | tr -d \"%\"); ",
            "LOAD=$(cat /proc/loadavg 2>/dev/null | awk '{{print $1}}'); ",
            "echo \"mem=${{MEM:-?}}% disk=${{DISK:-?}}% load=${{LOAD:-?}}\"; ",
            "echo \"===OBJECTIVES===\"; ",
            "if [ -n \"$CWD\" ] && command -v gh >/dev/null 2>&1; then ",
            "REMOTE=$(cd \"$CWD\" 2>/dev/null && git remote get-url origin 2>/dev/null); ",
            "if [ -n \"$REMOTE\" ]; then ",
            "gh issue list --repo \"$REMOTE\" --label fleet-objective --json number,title,state --jq '.[]|[.number,.title,.state]|@tsv' 2>/dev/null; ",
            "fi; fi; ",
            "echo \"===END===\""
        ),
        session = quoted_session
    );

    let mut cmd = Command::new(azlin_path);
    cmd.args(["connect", vm_name, "--no-tmux", "--yes", "--", &gather_cmd]);

    match run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("===TMUX===")
                || stdout.contains("===END===")
                || output.status.success()
            {
                parse_context_output(&stdout, &mut context)?;
            }
        }
        Err(_) => {
            context.agent_status = AgentStatus::Unreachable;
        }
    }

    if let Some(cached_capture) = cached_tmux_capture {
        context.tmux_capture = cached_capture.to_string();
        context.agent_status = infer_agent_status(cached_capture);
    }

    Ok(context)
}

pub(super) fn parse_context_output(output: &str, context: &mut SessionContext) -> Result<()> {
    let sections = output.split("===").collect::<Vec<_>>();
    let mut index = 0usize;
    while index + 1 < sections.len() {
        let label = sections[index].trim();
        if label.is_empty() {
            index += 1;
            continue;
        }
        let body = sections[index + 1].trim();
        match label {
            "TMUX" => {
                if body == "NO_SESSION" {
                    context.agent_status = AgentStatus::NoSession;
                } else {
                    context.tmux_capture = body.to_string();
                    context.agent_status = infer_agent_status(body);
                }
            }
            "CWD" => context.working_directory = body.to_string(),
            "GIT" => {
                for line in body.lines() {
                    if let Some(value) = line.strip_prefix("BRANCH:") {
                        context.git_branch = value.to_string();
                    } else if let Some(value) = line.strip_prefix("REMOTE:") {
                        context.repo_url = value.to_string();
                    } else if let Some(value) = line.strip_prefix("MODIFIED:") {
                        context.files_modified = value
                            .split(',')
                            .filter_map(|entry| {
                                let entry = entry.trim();
                                (!entry.is_empty()).then(|| entry.to_string())
                            })
                            .collect();
                    } else if let Some(value) = line.strip_prefix("PR_URL:") {
                        context.pr_url = value.trim().to_string();
                    }
                }
            }
            "TRANSCRIPT" => {
                let mut early = String::new();
                let mut recent = String::new();
                if let Some(early_start) = body.find("---EARLY---") {
                    if let Some(recent_start) = body.find("---RECENT---") {
                        early = body[early_start + "---EARLY---".len()..recent_start]
                            .trim()
                            .to_string();
                        recent = body[recent_start + "---RECENT---".len()..]
                            .trim()
                            .to_string();
                    }
                } else {
                    recent = body.to_string();
                }
                let mut transcript_parts = Vec::new();
                if !early.is_empty() {
                    transcript_parts.push("=== Session start ===".to_string());
                    transcript_parts.push(early);
                }
                if !recent.is_empty() {
                    if !transcript_parts.is_empty() {
                        transcript_parts.push("\n=== Recent activity ===".to_string());
                    }
                    transcript_parts.push(recent);
                }
                context.transcript_summary = transcript_parts.join("\n");
                if context.pr_url.is_empty() {
                    for line in context.transcript_summary.lines() {
                        if let Some(value) = line.split("PR_CREATED:").nth(1) {
                            context.pr_url = value.trim().to_string();
                            break;
                        }
                    }
                }
            }
            "HEALTH" => context.health_summary = body.to_string(),
            "OBJECTIVES" => {
                for line in body.lines() {
                    let parts = line.split('\t').collect::<Vec<_>>();
                    if parts.len() < 2 {
                        continue;
                    }
                    let number = match parts[0].trim().parse::<i64>() {
                        Ok(number) => number,
                        Err(_) => continue,
                    };
                    let title = parts[1]
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(256)
                        .collect::<String>();
                    let state = parts
                        .get(2)
                        .map(|value| value.trim().to_ascii_lowercase())
                        .filter(|value| value == "open" || value == "closed")
                        .unwrap_or_else(|| "open".to_string());
                    context.project_objectives.push(ProjectObjective {
                        number,
                        title,
                        state,
                        url: String::new(),
                    });
                }
            }
            _ => {}
        }
        index += 2;
    }

    if !context.repo_url.is_empty() {
        let (project_name, mut local_objectives) = match_project(&context.repo_url)?;
        if !project_name.is_empty() {
            context.project_name = project_name;
            let existing = context
                .project_objectives
                .iter()
                .map(|objective| objective.number)
                .collect::<std::collections::BTreeSet<_>>();
            local_objectives.retain(|objective| !existing.contains(&objective.number));
            context.project_objectives.extend(local_objectives);
        }
    }

    Ok(())
}

pub(super) fn infer_agent_status(tmux_text: &str) -> AgentStatus {
    let lines = tmux_text.trim().lines().collect::<Vec<_>>();
    let combined = lines.join("\n");
    let combined_lower = combined.to_ascii_lowercase();
    let last_line = lines.last().map(|line| line.trim()).unwrap_or_default();
    let last_line_lower = last_line.to_ascii_lowercase();

    let mut prompt_line_text = String::new();
    let mut has_prompt = false;
    for line in lines.iter().rev() {
        let stripped = line.trim();
        if stripped.starts_with('\u{276f}') {
            has_prompt = true;
            prompt_line_text = stripped.trim_start_matches('\u{276f}').trim().to_string();
            break;
        }
    }

    if lines
        .iter()
        .any(|line| line.contains("(running)") && line.contains("\u{23f5}\u{23f5}"))
    {
        return AgentStatus::Running;
    }
    if lines
        .iter()
        .any(|line| line.trim_start().starts_with('\u{00b7}'))
    {
        return AgentStatus::Thinking;
    }

    for line in lines.iter().rev() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with('\u{25cf}') && !stripped.starts_with("\u{25cf} Bash(") {
            return AgentStatus::Thinking;
        }
        if stripped.starts_with('\u{23bf}') {
            return AgentStatus::Thinking;
        }
        break;
    }

    let has_finished_indicator = lines.iter().any(|line| line.contains('\u{273b}'));
    if has_finished_indicator && has_prompt {
        return if prompt_line_text.is_empty() {
            AgentStatus::Idle
        } else {
            AgentStatus::Thinking
        };
    }
    if has_finished_indicator {
        return AgentStatus::Thinking;
    }

    if ["thinking...", "running:", "loading"]
        .iter()
        .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::Thinking;
    }
    if combined.contains("\u{25cf} Bash(")
        || combined.contains("\u{25cf} Read(")
        || combined.contains("\u{25cf} Write(")
        || combined.contains("\u{25cf} Edit(")
    {
        if last_line.contains("\u{23f5}\u{23f5}") {
            return AgentStatus::WaitingInput;
        }
        return AgentStatus::Thinking;
    }
    if has_prompt && !prompt_line_text.is_empty() {
        return AgentStatus::Thinking;
    }
    if has_prompt {
        return AgentStatus::Idle;
    }
    if last_line_lower.ends_with("$") || last_line_lower.ends_with("$ ") {
        return AgentStatus::Shell;
    }
    if ["y/n]", "yes/no", "[y/n", "(yes/no)"]
        .iter()
        .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::WaitingInput;
    }
    if combined.contains("\u{23f5}\u{23f5}")
        && (combined_lower.contains("bypass") || combined_lower.contains("allow"))
    {
        return AgentStatus::WaitingInput;
    }
    if last_line_lower.ends_with('?') {
        return AgentStatus::WaitingInput;
    }
    if ["error:", "traceback", "fatal:", "panic:"]
        .iter()
        .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::Error;
    }
    if combined.contains("GOAL_STATUS: ACHIEVED") || combined.contains("Workflow Complete") {
        return AgentStatus::Completed;
    }
    if (combined.contains("gh pr create")
        || combined.contains("PR #")
        || combined_lower.contains("pull request"))
        && ["created", "opened", "merged"]
            .iter()
            .any(|needle| combined_lower.contains(needle))
    {
        return AgentStatus::Completed;
    }
    if combined.trim().len() > MIN_SUBSTANTIAL_OUTPUT_LEN {
        return AgentStatus::Running;
    }
    AgentStatus::Unknown
}

