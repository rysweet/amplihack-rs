use crate::known_agents::is_amplihack_agent;
use regex::Regex;
use std::sync::OnceLock;

const AGENT_REFERENCE_PATTERNS: &[&str] = &[
    r"@\.claude/agents/amplihack/[^/]+/([^/]+)\.md",
    r"@\.claude/agents/([^/]+)\.md",
    r"Include\s+@\.claude/agents/[^/]+/([^/]+)\.md",
    r"Use\s+([a-z-]+)\.md\s+agent",
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

/// Names of real amplihack agents referenced in `prompt`: by an agent
/// definition reference (`@.claude/agents/…/builder.md`), or by a slash word
/// (`/analyzer`, `/reflect` → `reflection`).
///
/// An agent definition reference names its agent explicitly, so any agent
/// counts, bundled or project-defined (`@.claude/agents/my-reviewer.md`).
///
/// A slash word is a whitespace-separated token, ignoring surrounding
/// punctuation (`(/analyze`, `/reflect.`), that is `/` followed only by
/// lower-case letters and hyphens, anywhere in the prompt (at its end too).
/// A path segment is not one: in `docs/security` or `~/.amplihack/bin` the
/// `/` is inside a token. A slash word only counts when it names a bundled
/// agent definition or a slash-command agent, so `/skills` or `/bin` on
/// their own are not agents either (issue #1483).
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
    for token in prompt.split_whitespace() {
        let token = token
            .trim_start_matches(['(', '[', '"', '\'', '`'])
            .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '`']);
        let Some(word) = token.strip_prefix('/') else {
            continue;
        };
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            continue;
        }
        let agent_name = normalize_agent_name(word);
        if (is_amplihack_agent(&agent_name) || is_slash_command_agent(&agent_name))
            && !agents.iter().any(|existing| existing == &agent_name)
        {
            agents.push(agent_name);
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

fn is_slash_command_agent(agent_name: &str) -> bool {
    SLASH_COMMAND_AGENTS
        .iter()
        .any(|(_, agent)| *agent == agent_name)
}

/// Slash commands that invoke `agent_name` (`analyzer` → `analyze`).
pub(crate) fn slash_commands_for(agent_name: &str) -> impl Iterator<Item = &'static str> + '_ {
    SLASH_COMMAND_AGENTS
        .iter()
        .filter(move |(_, agent)| *agent == agent_name)
        .map(|(command, _)| *command)
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
