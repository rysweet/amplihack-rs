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

/// Strength of a completion signal scraped from terminal text.
///
/// issue #868: terminal-text classification is advisory. Only `Structured`
/// evidence (machine-emitted, unambiguous markers such as `GOAL_STATUS: ACHIEVED`
/// or a concrete `PR #<n>`) may drive an irreversible `MarkComplete`. `Textual`
/// evidence is a prose heuristic (e.g. "pull request created") that an agent
/// could merely be quoting, so it is advisory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionEvidence {
    /// A machine-emitted, unambiguous completion marker is present.
    Structured,
    /// Only an advisory prose completion heuristic is present.
    Textual,
    /// No completion marker is present.
    None,
}

/// Classify the strongest completion evidence present in `text`.
///
/// Structured markers take precedence over textual ones when both match (for
/// example, `PR #123 created` matches the structured `PR #\d+` and the textual
/// `PR.*created`; the structured signal wins).
pub(super) fn completion_evidence(text: &str) -> CompletionEvidence {
    if first_matching_pattern(STRUCTURED_COMPLETION_PATTERNS, text, false).is_some() {
        CompletionEvidence::Structured
    } else if first_matching_pattern(TEXTUAL_COMPLETION_PATTERNS, text, false).is_some() {
        CompletionEvidence::Textual
    } else {
        CompletionEvidence::None
    }
}

/// Whether a captured PR URL or terminal text is an *authoritative* completion
/// signal.
///
/// issue #868: an advisory `AgentStatus::Completed` (inferred from terminal text
/// that could merely be quoting a marker) must never on its own drive an
/// irreversible `MarkComplete`. Authoritative completion requires structured
/// corroboration: a concrete PR URL captured from session/git state, or a
/// STRUCTURED (machine-emitted) marker in the terminal capture. Shared by the
/// reasoning engine (via [`SessionContext`]) and the admiral director (via
/// `TmuxSessionInfo`) so every consumer enforces one completion contract.
pub(super) fn is_authoritative_completion(pr_url: &str, capture: &str) -> bool {
    !pr_url.trim().is_empty() || completion_evidence(capture) == CompletionEvidence::Structured
}

/// Whether a session context has an *authoritative* completion signal.
///
/// Thin [`SessionContext`] adapter over [`is_authoritative_completion`].
pub(super) fn has_authoritative_completion(context: &SessionContext) -> bool {
    is_authoritative_completion(&context.pr_url, &context.tmux_capture)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // infer_agent_status
    // -----------------------------------------------------------------------

    #[test]
    fn status_running_indicator() {
        let text = "some output\n(running) \u{23f5}\u{23f5}\nmore stuff";
        assert_eq!(infer_agent_status(text).as_str(), "running");
    }

    #[test]
    fn status_thinking_dot_prefix() {
        let text = "processing\n\u{00b7} thinking about it";
        assert_eq!(infer_agent_status(text).as_str(), "thinking");
    }

    #[test]
    fn status_thinking_bullet() {
        // The bullet must be the last non-empty line (reverse scan breaks on first)
        let text = "some output\n\u{25cf} SomeTask";
        assert_eq!(infer_agent_status(text).as_str(), "thinking");
    }

    #[test]
    fn status_thinking_bash_tool() {
        // Bash tool invocations are detected as thinking (tool in progress)
        let text = "some output\n\u{25cf} Bash(echo hello)";
        assert_eq!(infer_agent_status(text).as_str(), "thinking");
    }

    #[test]
    fn status_idle_prompt_empty() {
        let text = "\u{273b} done\n\u{276f} ";
        assert_eq!(infer_agent_status(text).as_str(), "idle");
    }

    #[test]
    fn status_thinking_prompt_with_text() {
        let text = "\u{273b} done\n\u{276f} typing something";
        assert_eq!(infer_agent_status(text).as_str(), "thinking");
    }

    #[test]
    fn status_shell_dollar_prompt() {
        let text = "user@host:~$ ";
        assert_eq!(infer_agent_status(text).as_str(), "shell");
    }

    #[test]
    fn status_waiting_input_yn() {
        let text = "Continue? [y/n]";
        assert_eq!(infer_agent_status(text).as_str(), "waiting_input");
    }

    #[test]
    fn status_waiting_input_question() {
        let text = "What should I do?";
        assert_eq!(infer_agent_status(text).as_str(), "waiting_input");
    }

    #[test]
    fn status_error() {
        let text = "error: compilation failed";
        assert_eq!(infer_agent_status(text).as_str(), "error");
    }

    #[test]
    fn status_error_traceback() {
        let text = "Traceback (most recent call last):\n  File...";
        assert_eq!(infer_agent_status(text).as_str(), "error");
    }

    #[test]
    fn status_completed_goal_achieved() {
        let text = "GOAL_STATUS: ACHIEVED\nAll done";
        assert_eq!(infer_agent_status(text).as_str(), "completed");
    }

    #[test]
    fn status_completed_workflow() {
        let text = "Workflow Complete";
        assert_eq!(infer_agent_status(text).as_str(), "completed");
    }

    #[test]
    fn status_completed_pr_created() {
        let text = "gh pr create --title 'feat'\nPR #42 created successfully";
        assert_eq!(infer_agent_status(text).as_str(), "completed");
    }

    #[test]
    fn status_unknown_minimal_output() {
        let text = "hi";
        assert_eq!(infer_agent_status(text).as_str(), "unknown");
    }

    #[test]
    fn status_running_substantial_output() {
        let text = "x".repeat(MIN_SUBSTANTIAL_OUTPUT_LEN + 10);
        assert_eq!(infer_agent_status(&text).as_str(), "running");
    }

    #[test]
    fn status_no_session() {
        // This is tested via parse_context_output, not infer directly
        let text = "NO_SESSION";
        // NO_SESSION is handled by parse_context_output, not infer
        // infer just sees text
        let status = infer_agent_status(text);
        assert!(matches!(
            status,
            AgentStatus::Unknown | AgentStatus::Running
        ));
    }

    // -----------------------------------------------------------------------
    // parse_context_output — CWD / GIT / HEALTH sections
    // -----------------------------------------------------------------------

    #[test]
    fn parse_cwd_section() {
        let output = "===TMUX===\nNO_SESSION\n===CWD===\n/home/user/project\n===END===";
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        parse_context_output(output, &mut ctx).unwrap();
        assert_eq!(ctx.working_directory, "/home/user/project");
        assert_eq!(ctx.agent_status.as_str(), "no_session");
    }

    #[test]
    fn parse_git_section() {
        let output = "===TMUX===\nNO_SESSION\n===GIT===\nBRANCH:main\nREMOTE:https://github.com/org/repo\nMODIFIED:file1.rs,file2.rs,\n===END===";
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        parse_context_output(output, &mut ctx).unwrap();
        assert_eq!(ctx.git_branch, "main");
        assert_eq!(ctx.files_modified, vec!["file1.rs", "file2.rs"]);
    }

    #[test]
    fn parse_health_section() {
        let output = "===TMUX===\nNO_SESSION\n===HEALTH===\nmem=42% disk=10% load=1.5\n===END===";
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        parse_context_output(output, &mut ctx).unwrap();
        assert!(ctx.health_summary.contains("mem=42%"));
    }

    #[test]
    fn parse_pr_url_from_git() {
        let output = "===TMUX===\nNO_SESSION\n===GIT===\nPR_URL:https://github.com/org/repo/pull/5\n===END===";
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        parse_context_output(output, &mut ctx).unwrap();
        assert_eq!(ctx.pr_url, "https://github.com/org/repo/pull/5");
    }

    #[test]
    fn parse_objectives_section() {
        let output =
            "===TMUX===\nNO_SESSION\n===OBJECTIVES===\n42\tBuild feature X\topen\n===END===";
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        parse_context_output(output, &mut ctx).unwrap();
        assert_eq!(ctx.project_objectives.len(), 1);
        assert_eq!(ctx.project_objectives[0].number, 42);
        assert_eq!(ctx.project_objectives[0].title, "Build feature X");
    }

    #[test]
    fn parse_transcript_with_early_and_recent() {
        let output = "===TMUX===\nNO_SESSION\n===TRANSCRIPT===\nTRANSCRIPT_LINES:5\n---EARLY---\nfirst message\n---RECENT---\nlast message\n===END===";
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        parse_context_output(output, &mut ctx).unwrap();
        assert!(ctx.transcript_summary.contains("first message"));
        assert!(ctx.transcript_summary.contains("last message"));
    }

    // -----------------------------------------------------------------------
    // issue #868: completion patterns are split into a trustworthy structured
    // set and an advisory textual set; `completion_evidence` keys off which set
    // matched, and only STRUCTURED evidence may drive an irreversible
    // MarkComplete.
    // -----------------------------------------------------------------------

    fn completed_ctx(capture: &str) -> SessionContext {
        let mut ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        ctx.agent_status = AgentStatus::Completed;
        ctx.tmux_capture = capture.to_string();
        ctx
    }

    fn none_reasoner() -> FleetSessionReasoner {
        FleetSessionReasoner::new(
            PathBuf::from("/nonexistent/azlin"),
            NativeReasonerBackend::None,
        )
    }

    #[test]
    fn completion_pattern_sets_present_and_disjoint() {
        assert!(
            !STRUCTURED_COMPLETION_PATTERNS.is_empty(),
            "structured completion patterns must exist"
        );
        assert!(
            !TEXTUAL_COMPLETION_PATTERNS.is_empty(),
            "textual completion patterns must exist"
        );
        assert!(
            STRUCTURED_COMPLETION_PATTERNS
                .iter()
                .any(|p| p.contains("GOAL_STATUS")),
            "GOAL_STATUS is a structured (machine-emitted) marker"
        );
        assert!(
            TEXTUAL_COMPLETION_PATTERNS
                .iter()
                .any(|p| p.contains("created")),
            "prose 'created' heuristics belong to the textual set"
        );
        for pattern in STRUCTURED_COMPLETION_PATTERNS {
            assert!(
                !TEXTUAL_COMPLETION_PATTERNS.contains(pattern),
                "pattern {pattern:?} must not appear in both sets"
            );
        }
    }

    #[test]
    fn completion_evidence_structured_goal_status() {
        assert_eq!(
            completion_evidence("GOAL_STATUS: ACHIEVED"),
            CompletionEvidence::Structured
        );
    }

    #[test]
    fn completion_evidence_structured_workflow_complete() {
        assert_eq!(
            completion_evidence("Workflow Complete"),
            CompletionEvidence::Structured
        );
    }

    #[test]
    fn completion_evidence_structured_pr_number() {
        assert_eq!(
            completion_evidence("Opened PR #4217 for review"),
            CompletionEvidence::Structured
        );
    }

    #[test]
    fn completion_evidence_textual_pull_request_created() {
        assert_eq!(
            completion_evidence("the pull request created is ready"),
            CompletionEvidence::Textual
        );
    }

    #[test]
    fn completion_evidence_textual_pushed_branch() {
        assert_eq!(
            completion_evidence("Changes pushed to the feature branch"),
            CompletionEvidence::Textual
        );
    }

    #[test]
    fn completion_evidence_none_for_plain_output() {
        assert_eq!(
            completion_evidence("Reading files and analyzing the code"),
            CompletionEvidence::None
        );
    }

    #[test]
    fn completion_evidence_structured_beats_textual_on_overlap() {
        // "PR #123 created" matches both structured `PR #\d+` and textual
        // `PR.*created`; the structured match must win.
        assert_eq!(
            completion_evidence("PR #123 created successfully"),
            CompletionEvidence::Structured
        );
    }

    #[test]
    fn completion_evidence_quoted_prose_marker_is_not_structured() {
        // An agent merely quoting the instruction must not produce a trusted
        // (Structured) completion signal — this is the core #868 false-positive.
        let capture = "Reminder: when done, print 'pull request created' to signal completion.";
        assert_eq!(completion_evidence(capture), CompletionEvidence::Textual);
    }

    #[test]
    fn status_completed_textual_marker_is_advisory_completed() {
        // Shared pattern source: a textual completion marker is still classified
        // as "completed" for the *advisory* AgentStatus; authority lives in the
        // downstream completion guard, not in the status classifier.
        let text = "the pull request created for this task is open";
        assert_eq!(infer_agent_status(text).as_str(), "completed");
    }

    #[test]
    fn heuristic_decision_structured_completion_marks_complete() {
        let ctx = completed_ctx("GOAL_STATUS: ACHIEVED\nAll done");
        assert_eq!(heuristic_decision(&ctx).action, SessionAction::MarkComplete);
    }

    #[test]
    fn heuristic_decision_textual_completion_does_not_mark_complete() {
        // issue #868: textual-only completion must NOT auto-complete; it
        // downgrades to wait/escalate.
        let ctx = completed_ctx("Looks like the pull request created earlier is ready");
        let decision = heuristic_decision(&ctx);
        assert_ne!(
            decision.action,
            SessionAction::MarkComplete,
            "textual-only completion must not auto-complete"
        );
        assert!(matches!(
            decision.action,
            SessionAction::Wait | SessionAction::Escalate
        ));
    }

    #[test]
    fn reason_structured_completion_marks_complete() {
        let reasoner = none_reasoner();
        let ctx = completed_ctx("PR #123\nGOAL_STATUS: ACHIEVED");
        let (decision, _) = reasoner.reason(&ctx);
        assert_eq!(decision.action, SessionAction::MarkComplete);
    }

    #[test]
    fn reason_textual_completion_not_marked_complete() {
        // issue #868: the reasoning-engine short-circuit must require STRUCTURED
        // evidence before MarkComplete; textual-only falls through to reasoning
        // (here the None backend yields a guarded heuristic decision).
        let reasoner = none_reasoner();
        let ctx = completed_ctx("the pull request created by the bot is under review");
        let (decision, _) = reasoner.reason(&ctx);
        assert_ne!(decision.action, SessionAction::MarkComplete);
    }

    #[test]
    fn is_authoritative_completion_requires_structured_corroboration() {
        // issue #868: the shared completion contract enforced by both the
        // reasoning engine and the admiral director.
        // A captured PR URL is authoritative regardless of the capture text.
        assert!(is_authoritative_completion(
            "https://github.com/org/repo/pull/1",
            "the pull request created earlier"
        ));
        // A machine-emitted (STRUCTURED) marker is authoritative without a URL.
        assert!(is_authoritative_completion("", "GOAL_STATUS: ACHIEVED"));
        assert!(is_authoritative_completion(
            "",
            "Opened PR #4217 for review"
        ));
        // A textual-only marker with no URL is NOT authoritative (the core bug).
        assert!(!is_authoritative_completion(
            "",
            "the pull request created is ready"
        ));
        // A whitespace-only URL is treated as absent.
        assert!(!is_authoritative_completion("   ", "just working on it"));
    }

    #[test]
    fn parse_reasoner_response_extracts_json_before_trailing_prose_braces() {
        // issue #868: the naive first-'{' .. last-'}' slice breaks when prose
        // after the JSON contains braces. The shared extractor recovers it.
        let ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        let raw = "{\"action\": \"mark_complete\", \"confidence\": 0.9} note: avoid {curly} text";
        let decision =
            parse_reasoner_response(raw, &ctx).expect("decision JSON should be recovered");
        assert_eq!(decision.action, SessionAction::MarkComplete);
        assert!((decision.confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn parse_reasoner_response_missing_json_returns_none() {
        let ctx = SessionContext::new("vm1", "sess1", "task", "prio").unwrap();
        assert!(parse_reasoner_response("I am not sure what to do here.", &ctx).is_none());
    }
}
