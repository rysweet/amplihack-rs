use amplihack_types::ProjectDirs;
use std::fs;

#[derive(Debug, Clone)]
pub(crate) struct OriginalRequest {
    pub(crate) session_id: String,
    pub(crate) timestamp: u64,
    pub(crate) raw_prompt: String,
    pub(crate) target: String,
    pub(crate) requirements: Vec<String>,
    pub(crate) constraints: Vec<String>,
    pub(crate) success_criteria: Vec<String>,
}

pub(crate) fn capture_original_request(
    dirs: &ProjectDirs,
    session_id: Option<&str>,
    prompt: &str,
) -> anyhow::Result<Option<OriginalRequest>> {
    let prompt = prompt.trim();
    if prompt.is_empty() || !is_substantial_prompt(prompt) {
        return Ok(None);
    }

    let session_id = session_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("session-start")
        .to_string();
    let request = build_original_request(&session_id, prompt);
    save_original_request(dirs, &request)?;
    Ok(Some(request))
}

pub(crate) fn format_original_request_context(request: &OriginalRequest) -> String {
    let mut parts = vec![
        "## 🎯 ORIGINAL USER REQUEST - PRESERVE THESE REQUIREMENTS".to_string(),
        String::new(),
        format!("**Target**: {}", request.target),
        String::new(),
    ];

    if !request.requirements.is_empty() {
        parts.push("**Requirements**:".to_string());
        for requirement in &request.requirements {
            parts.push(format!("• {requirement}"));
        }
        parts.push(String::new());
    }

    if !request.constraints.is_empty() {
        parts.push("**Constraints**:".to_string());
        for constraint in &request.constraints {
            parts.push(format!("• {constraint}"));
        }
        parts.push(String::new());
    }

    if !request.success_criteria.is_empty() {
        parts.push("**Success Criteria**:".to_string());
        for criterion in &request.success_criteria {
            parts.push(format!("• {criterion}"));
        }
        parts.push(String::new());
    }

    parts.extend([
        "**CRITICAL**: These are the user's explicit requirements. Do NOT optimize them away."
            .to_string(),
        "Your solution must address ALL requirements while following the constraints.".to_string(),
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".to_string(),
    ]);

    parts.join("\n")
}

fn is_substantial_prompt(prompt: &str) -> bool {
    let lowered = prompt.to_ascii_lowercase();
    prompt.len() > 20
        || [
            "implement",
            "create",
            "build",
            "add",
            "fix",
            "update",
            "all",
            "every",
            "each",
            "complete",
            "comprehensive",
        ]
        .iter()
        .any(|keyword| lowered.contains(keyword))
}

fn build_original_request(session_id: &str, prompt: &str) -> OriginalRequest {
    let sentences = prompt
        .split(['\n', '.', '!', '?'])
        .map(str::trim)
        .filter(|sentence| sentence.len() > 10)
        .collect::<Vec<_>>();

    OriginalRequest {
        session_id: session_id.to_string(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        raw_prompt: prompt.to_string(),
        target: sentences
            .first()
            .map(|sentence| (*sentence).to_string())
            .unwrap_or_else(|| prompt.to_string()),
        requirements: extract_matching_sentences(
            &sentences,
            &["all", "every", "each", "complete", "comprehensive"],
        ),
        constraints: extract_matching_sentences(
            &sentences,
            &[
                "do not", "don't", "must not", "without", "avoid", "exclude", "never",
            ],
        ),
        success_criteria: extract_matching_sentences(
            &sentences,
            &["ensure", "verify", "pass", "success"],
        ),
    }
}

fn extract_matching_sentences(sentences: &[&str], keywords: &[&str]) -> Vec<String> {
    let mut matches = Vec::new();
    for sentence in sentences {
        let lowered = sentence.to_ascii_lowercase();
        if keywords.iter().any(|keyword| lowered.contains(keyword))
            && !matches.iter().any(|existing| existing == sentence)
        {
            matches.push((*sentence).to_string());
        }
    }
    matches
}

fn save_original_request(dirs: &ProjectDirs, request: &OriginalRequest) -> anyhow::Result<()> {
    let session_dir = dirs.session_logs(&request.session_id);
    fs::create_dir_all(&session_dir)?;

    let markdown = format!(
        "# Original User Request\n\n**Session**: {}\n**Timestamp**: {}\n**Target**: {}\n\n## Raw Request\n```\n{}\n```\n{}{}{}",
        request.session_id,
        request.timestamp,
        request.target,
        request.raw_prompt,
        format_markdown_section("Extracted Requirements", &request.requirements),
        format_markdown_section("Constraints", &request.constraints),
        format_markdown_section("Success Criteria", &request.success_criteria),
    );
    fs::write(session_dir.join("ORIGINAL_REQUEST.md"), markdown)?;
    fs::write(
        session_dir.join("original_request.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "session_id": request.session_id,
            "timestamp": request.timestamp,
            "target": request.target,
            "raw_prompt": request.raw_prompt,
            "requirements": request.requirements,
            "constraints": request.constraints,
            "success_criteria": request.success_criteria,
        }))?,
    )?;

    Ok(())
}

fn format_markdown_section(title: &str, items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }

    let mut section = format!("\n## {title}\n");
    for (index, item) in items.iter().enumerate() {
        section.push_str(&format!("{}. {}\n", index + 1, item));
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substantial_prompt_long_text() {
        assert!(is_substantial_prompt(
            "This is a prompt that has more than twenty characters"
        ));
    }

    #[test]
    fn substantial_prompt_with_keyword() {
        assert!(is_substantial_prompt("fix this"));
    }

    #[test]
    fn non_substantial_prompt() {
        assert!(!is_substantial_prompt("hi"));
    }

    #[test]
    fn build_request_extracts_target() {
        let req = build_original_request("sess-1", "Implement JWT auth for the API.");
        assert_eq!(req.session_id, "sess-1");
        assert_eq!(req.target, "Implement JWT auth for the API");
    }

    #[test]
    fn build_request_extracts_constraints() {
        let req = build_original_request(
            "s1",
            "Build the feature. Do not modify the database schema.",
        );
        assert!(
            req.constraints.iter().any(|c| c.contains("Do not modify")),
            "should extract constraint: {:?}",
            req.constraints
        );
    }

    #[test]
    fn build_request_extracts_requirements() {
        let req = build_original_request(
            "s1",
            "Create a test suite. Ensure all endpoints are covered.",
        );
        assert!(
            req.success_criteria
                .iter()
                .any(|c| c.contains("Ensure all")),
            "should extract success criterion: {:?}",
            req.success_criteria
        );
    }

    #[test]
    fn extract_matching_deduplicates() {
        let sentences = vec!["do not break it", "do not break it"];
        let result = extract_matching_sentences(&sentences, &["do not"]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn format_context_includes_sections() {
        let req = OriginalRequest {
            session_id: "s1".to_string(),
            timestamp: 0,
            raw_prompt: "test".to_string(),
            target: "Build auth".to_string(),
            requirements: vec!["all endpoints".to_string()],
            constraints: vec!["do not modify db".to_string()],
            success_criteria: vec!["ensure tests pass".to_string()],
        };
        let output = format_original_request_context(&req);
        assert!(output.contains("Build auth"));
        assert!(output.contains("all endpoints"));
        assert!(output.contains("do not modify db"));
        assert!(output.contains("ensure tests pass"));
    }

    #[test]
    fn format_markdown_section_empty() {
        assert!(format_markdown_section("Title", &[]).is_empty());
    }

    #[test]
    fn format_markdown_section_with_items() {
        let items = vec!["first".to_string(), "second".to_string()];
        let output = format_markdown_section("Requirements", &items);
        assert!(output.contains("## Requirements"));
        assert!(output.contains("1. first"));
        assert!(output.contains("2. second"));
    }
}
