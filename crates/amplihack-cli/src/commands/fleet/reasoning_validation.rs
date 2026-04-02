use super::*;

pub(super) fn parse_reasoner_response(
    response_text: &str,
    context: &SessionContext,
) -> Option<SessionDecision> {
    let json_start = response_text.find('{')?;
    let json_end = response_text.rfind('}')?;
    if json_end <= json_start {
        return None;
    }
    let value: Value = serde_json::from_str(&response_text[json_start..=json_end]).ok()?;
    let action = match value.get("action").and_then(Value::as_str) {
        Some("send_input") => SessionAction::SendInput,
        Some("wait") => SessionAction::Wait,
        Some("escalate") => SessionAction::Escalate,
        Some("mark_complete") => SessionAction::MarkComplete,
        Some("restart") => SessionAction::Restart,
        _ => SessionAction::Wait,
    };
    let confidence = value
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    Some(SessionDecision {
        session_name: context.session_name.clone(),
        vm_name: context.vm_name.clone(),
        action,
        input_text: value
            .get("input_text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        reasoning: value
            .get("reasoning")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        confidence,
    })
}

pub(super) fn heuristic_decision(context: &SessionContext) -> SessionDecision {
    let (action, reasoning, confidence) = match context.agent_status {
        AgentStatus::Completed => (
            SessionAction::MarkComplete,
            "Session output indicates completion".to_string(),
            CONFIDENCE_COMPLETION,
        ),
        AgentStatus::Error | AgentStatus::Shell | AgentStatus::Stuck => (
            SessionAction::Escalate,
            "Session needs human attention or restart review".to_string(),
            CONFIDENCE_ERROR,
        ),
        AgentStatus::WaitingInput => (
            SessionAction::Wait,
            "Session is waiting for input, but no native reasoner backend was available"
                .to_string(),
            CONFIDENCE_IDLE,
        ),
        AgentStatus::Thinking | AgentStatus::Running => (
            SessionAction::Wait,
            "Session appears active; no intervention needed".to_string(),
            CONFIDENCE_RUNNING,
        ),
        AgentStatus::Idle => (
            SessionAction::Wait,
            "Session is idle at the prompt".to_string(),
            CONFIDENCE_IDLE,
        ),
        AgentStatus::NoSession | AgentStatus::Unreachable | AgentStatus::Unknown => (
            SessionAction::Wait,
            "Session is empty or unavailable".to_string(),
            CONFIDENCE_UNKNOWN,
        ),
    };
    SessionDecision {
        session_name: context.session_name.clone(),
        vm_name: context.vm_name.clone(),
        action,
        input_text: String::new(),
        reasoning,
        confidence,
    }
}

pub(super) fn is_dangerous_input(text: &str) -> bool {
    if Regex::new(r"[;|&`]|\$\(")
        .ok()
        .is_some_and(|regex| regex.is_match(text))
    {
        return true;
    }
    if SAFE_INPUT_PATTERNS
        .iter()
        .filter_map(|pattern| {
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        })
        .any(|regex| regex.is_match(text))
    {
        return false;
    }
    DANGEROUS_INPUT_PATTERNS
        .iter()
        .filter_map(|pattern| {
            RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        })
        .any(|regex| regex.is_match(text))
}

pub(super) fn remote_parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_else(|| ".".to_string())
}

pub(super) fn validate_chmod_mode(mode: &str) -> Result<()> {
    if mode.len() < 3 || mode.len() > 4 || !mode.chars().all(|ch| ('0'..='7').contains(&ch)) {
        bail!("Invalid chmod mode: {mode:?}");
    }
    Ok(())
}

pub(super) fn validate_vm_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid VM name: {name:?}");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("Invalid VM name: {name:?}");
    }
    if name.len() > 64 || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
        bail!("Invalid VM name: {name:?}");
    }
    Ok(())
}

pub(super) fn validate_session_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("Invalid session name: {name:?}");
    };
    if !first.is_ascii_alphanumeric() {
        bail!("Invalid session name: {name:?}");
    }
    if name.len() > 128
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'))
    {
        bail!("Invalid session name: {name:?}");
    }
    Ok(())
}

pub(super) fn get_azlin_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("AZLIN_PATH") {
        return Ok(PathBuf::from(path));
    }

    if let Some(path) = find_binary("azlin") {
        return Ok(path);
    }

    if let Some(home) = env::var_os("HOME") {
        let dev_path = PathBuf::from(home).join("src/azlin/.venv/bin/azlin");
        if is_executable_file(&dev_path) {
            return Ok(dev_path);
        }
    }

    bail!(
        "azlin not found. Set AZLIN_PATH to the binary location.\nSee: https://github.com/rysweet/azlin"
    )
}

pub(super) fn find_binary(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(name);
        is_executable_file(&candidate).then_some(candidate)
    })
}
