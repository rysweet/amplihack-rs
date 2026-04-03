//! Skill injection registry for domain agents.
//!
//! Ports `domain_agents/skill_injector.py`: SkillInjector registry
//! mapping domain → skill_name → tool function.

use std::collections::HashMap;

use serde_json::Value;

/// Type alias for skill tool functions.
pub type ToolFn = Box<dyn Fn(Value) -> Value + Send + Sync>;

/// Registry that maps skills to domain agent tools.
///
/// Skills are organized by domain. Each domain can have multiple
/// named tools that are injected into domain agents at construction.
pub struct SkillInjector {
    skills: HashMap<String, HashMap<String, ToolFn>>,
}

impl Default for SkillInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillInjector {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// Register a tool function for a domain.
    pub fn register(
        &mut self,
        domain: &str,
        skill_name: &str,
        tool_fn: ToolFn,
    ) -> crate::error::Result<()> {
        let domain = domain.trim();
        let skill_name = skill_name.trim();
        if domain.is_empty() {
            return Err(crate::error::DomainError::InvalidInput(
                "domain cannot be empty".into(),
            ));
        }
        if skill_name.is_empty() {
            return Err(crate::error::DomainError::InvalidInput(
                "skill_name cannot be empty".into(),
            ));
        }
        self.skills
            .entry(domain.to_string())
            .or_default()
            .insert(skill_name.to_string(), tool_fn);
        Ok(())
    }

    /// Get all skill names for a domain.
    pub fn get_skill_names_for_domain(&self, domain: &str) -> Vec<String> {
        let domain = domain.trim();
        if domain.is_empty() {
            return vec![];
        }
        self.skills
            .get(domain)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Check if a skill exists for a domain.
    pub fn has_skill(&self, domain: &str, skill_name: &str) -> bool {
        self.skills
            .get(domain)
            .map(|m| m.contains_key(skill_name))
            .unwrap_or(false)
    }

    /// Get all registered domains.
    pub fn domains(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    /// Execute a skill by domain and name, passing input as JSON value.
    pub fn execute(&self, domain: &str, skill_name: &str, input: Value) -> Option<Value> {
        self.skills
            .get(domain)
            .and_then(|m| m.get(skill_name))
            .map(|f| f(input))
    }
}

// -- Default skill tool implementations --

/// Detect code smells in source code.
pub fn code_smell_detector_tool(input: Value) -> Value {
    let code = input.get("code").and_then(|v| v.as_str()).unwrap_or("");
    let language = input
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("python");
    let lines: Vec<&str> = code.lines().collect();
    let mut smells = Vec::new();

    if lines.len() > 50 {
        smells.push(serde_json::json!({
            "type": "long_function",
            "severity": "warning",
            "message": format!("Function is {} lines long (threshold: 50)", lines.len()),
        }));
    }

    let max_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .max()
        .unwrap_or(0);
    if max_indent > 16 {
        smells.push(serde_json::json!({
            "type": "deep_nesting",
            "severity": "warning",
            "message": format!("Maximum nesting depth is {} levels", max_indent / 4),
        }));
    }

    serde_json::json!({
        "smells": smells,
        "smell_count": smells.len(),
        "language": language,
    })
}

/// Review a code diff for issues.
pub fn pr_review_tool(input: Value) -> Value {
    let diff = input
        .get("code_diff")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let lines: Vec<&str> = diff.lines().collect();
    let added: Vec<&&str> = lines
        .iter()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .collect();
    let removed: Vec<&&str> = lines
        .iter()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .collect();
    let mut findings = Vec::new();

    if added.len() > 200 {
        findings.push(serde_json::json!({
            "type": "large_change",
            "severity": "info",
            "message": format!("Large change: {} lines added", added.len()),
        }));
    }

    for line in &added {
        if line.contains("print(") || line.contains("console.log") || line.contains("debugger") {
            findings.push(serde_json::json!({
                "type": "debug_statement",
                "severity": "warning",
                "message": format!("Debug statement found: {}", &line[..line.len().min(80)]),
            }));
        }
    }

    serde_json::json!({
        "findings": findings,
        "lines_added": added.len(),
        "lines_removed": removed.len(),
    })
}

/// Extract structured notes from a meeting transcript.
pub fn meeting_notes_tool(input: Value) -> Value {
    let transcript = input
        .get("transcript")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let lines: Vec<&str> = transcript.lines().collect();
    let mut speakers = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in &lines {
        if let Some(idx) = line.find(':') {
            let speaker = line[..idx].trim();
            if !speaker.is_empty() && speaker.len() < 30 {
                let key = speaker.to_lowercase();
                if seen.insert(key) {
                    speakers.push(speaker.to_string());
                }
            }
        }
    }

    serde_json::json!({
        "speaker_count": speakers.len(),
        "speakers": speakers,
        "line_count": lines.len(),
        "word_count": transcript.split_whitespace().count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injector_register_and_lookup() {
        let mut inj = SkillInjector::new();
        inj.register("code_review", "lint", Box::new(|_| serde_json::json!("ok")))
            .unwrap();
        assert!(inj.has_skill("code_review", "lint"));
        assert!(!inj.has_skill("code_review", "format"));
        assert_eq!(inj.get_skill_names_for_domain("code_review"), vec!["lint"]);
    }

    #[test]
    fn injector_empty_domain_error() {
        let mut inj = SkillInjector::new();
        assert!(
            inj.register("", "lint", Box::new(|_| serde_json::json!("ok")))
                .is_err()
        );
    }

    #[test]
    fn injector_empty_skill_error() {
        let mut inj = SkillInjector::new();
        assert!(
            inj.register("code_review", "", Box::new(|_| serde_json::json!("ok")))
                .is_err()
        );
    }

    #[test]
    fn injector_execute() {
        let mut inj = SkillInjector::new();
        inj.register("test", "echo", Box::new(|v| v)).unwrap();
        let result = inj.execute("test", "echo", serde_json::json!(42));
        assert_eq!(result, Some(serde_json::json!(42)));
    }

    #[test]
    fn injector_domains() {
        let mut inj = SkillInjector::new();
        inj.register("a", "x", Box::new(|_| serde_json::json!(1)))
            .unwrap();
        inj.register("b", "y", Box::new(|_| serde_json::json!(2)))
            .unwrap();
        let mut domains = inj.domains();
        domains.sort();
        assert_eq!(domains, vec!["a", "b"]);
    }

    #[test]
    fn code_smell_detector_long_function() {
        let code = (0..60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = code_smell_detector_tool(serde_json::json!({"code": code}));
        assert_eq!(result["smell_count"], 1);
        assert_eq!(result["smells"][0]["type"], "long_function");
    }

    #[test]
    fn pr_review_debug_statement() {
        let diff = "+    print('debug')\n+    normal_code()";
        let result = pr_review_tool(serde_json::json!({"code_diff": diff}));
        assert!(
            result["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["type"] == "debug_statement")
        );
    }

    #[test]
    fn meeting_notes_speakers() {
        let transcript = "Alice: Hello\nBob: Hi\nAlice: How are you?";
        let result = meeting_notes_tool(serde_json::json!({"transcript": transcript}));
        assert_eq!(result["speaker_count"], 2);
    }
}
