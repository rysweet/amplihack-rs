use regex::Regex;
use std::sync::OnceLock;

const AGENT_REFERENCE_PATTERNS: &[&str] = &[
    r"@\.claude/agents/amplihack/[^/]+/([^/]+)\.md",
    r"@\.claude/agents/([^/]+)\.md",
    r"Include\s+@\.claude/agents/[^/]+/([^/]+)\.md",
    r"Use\s+([a-z-]+)\.md\s+agent",
    r"/([a-z-]+)\s",
];

const SLASH_COMMAND_AGENTS: &[(&str, &str)] = &[
    ("ultrathink", "orchestrator"),
    ("fix", "fix-agent"),
    ("analyze", "analyzer"),
    ("improve", "reviewer"),
    ("socratic", "ambiguity"),
    ("debate", "multi-agent-debate"),
    ("reflect", "reflection"),
    ("xpia", "xpia-defense"),
];

pub(crate) fn detect_agent_references(prompt: &str) -> Vec<String> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        AGENT_REFERENCE_PATTERNS
            .iter()
            .map(|pattern| Regex::new(pattern).expect("valid agent reference regex"))
            .collect()
    });

    let mut agents = Vec::new();
    for pattern in patterns {
        for captures in pattern.captures_iter(prompt) {
            let Some(agent_name) = captures
                .get(1)
                .map(|capture| normalize_agent_name(capture.as_str()))
            else {
                continue;
            };
            if !agents.iter().any(|existing| existing == &agent_name) {
                agents.push(agent_name);
            }
        }
    }
    agents
}

pub(crate) fn detect_slash_command_agent(prompt: &str) -> Option<&'static str> {
    static SLASH_PATTERN: OnceLock<Regex> = OnceLock::new();
    let prompt = prompt.trim();
    if !prompt.starts_with('/') {
        return None;
    }

    let regex = SLASH_PATTERN
        .get_or_init(|| Regex::new(r"^/([a-z-]+)").expect("valid slash command regex"));
    let command = regex.captures(prompt)?.get(1)?.as_str();
    SLASH_COMMAND_AGENTS
        .iter()
        .find_map(|(name, agent)| (*name == command).then_some(*agent))
}

pub(crate) fn normalize_agent_name(agent_name: &str) -> String {
    match agent_name.to_lowercase().replace('_', "-").as_str() {
        "ultrathink" => "orchestrator".to_string(),
        "fix" => "fix-agent".to_string(),
        "analyze" => "analyzer".to_string(),
        "improve" => "reviewer".to_string(),
        "socratic" => "ambiguity".to_string(),
        "debate" => "multi-agent-debate".to_string(),
        "reflect" => "reflection".to_string(),
        "xpia" => "xpia-defense".to_string(),
        other => other.to_string(),
    }
}
