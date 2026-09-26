use crate::known_agents::is_amplihack_agent;
use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// An agent definition reference: `@` and a path to a `.md` file under a
/// `.claude/agents/` directory, at any depth below it, optionally with a
/// path before it (`@~/.amplihack/.claude/agents/…`). The `Include @…` form
/// is covered too. The name is the file's own stem, one path component.
const DEFINITION_REFERENCE_PATTERN: &str =
    r"@(?:[~A-Za-z0-9_.-]+/)*\.claude/agents/(?:[A-Za-z0-9_-]+/)*([A-Za-z0-9_-]+)\.md";

const AGENT_REFERENCE_PATTERNS: &[&str] = &[
    DEFINITION_REFERENCE_PATTERN,
    // `Use <name>.md agent`, with a capital `U`.
    r"Use\s+([a-z-]+)\.md\s+agent",
];

fn definition_reference_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(DEFINITION_REFERENCE_PATTERN).expect("valid definition reference regex")
    })
}

/// `text` with every agent definition reference blanked to a space. A
/// reference is how a prompt invokes an agent, not what it is about: its
/// directory names (`claude`, `amplihack`, `core`) must not make a memory
/// that used the same reference look relevant.
pub(crate) fn without_definition_references(text: &str) -> Cow<'_, str> {
    definition_reference_regex().replace_all(text, " ")
}

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
/// definition reference (`@.claude/agents/…/builder.md`), by
/// `Use <name>.md agent` (capital `U`), or by a slash word (`/analyzer`,
/// `/reflect` → `reflection`).
///
/// An agent definition reference names its agent explicitly, so any agent
/// counts, bundled, project-defined (`@.claude/agents/my-reviewer.md`) or
/// user-level (`@~/.claude/agents/my-reviewer.md`), at any directory depth
/// below `.claude/agents/` (`@.claude/agents/team/security-reviewer.md`,
/// `@~/.amplihack/.claude/agents/amplihack/core/builder.md`). The agent is
/// the file's stem: a name is one path component, never prose up to a
/// later `.md`. `Use <name>.md agent` names any agent too.
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
