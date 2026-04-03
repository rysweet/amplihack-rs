//! Platform-specific prompt formatters and output parsers.
//!
//! Split from [`super`] to keep each file under 400 lines.

/// Format a prompt for Claude Code with persona-specific styling.
pub fn format_claude_prompt(goal: &str, persona: &str, context: &str) -> String {
    match persona {
        "guide" => format!(
            "You are a guide persona helping someone learn and understand.\n\n\
             **Your approach:**\n\
             - Teach concepts through explanation and examples\n\
             - Ask Socratic questions to deepen understanding\n\
             - Provide clear documentation and tutorials\n\
             - Break down complex ideas into digestible parts\n\n\
             **Goal:** {goal}\n\n\
             **Context:** {context}\n\n\
             Remember to focus on educational value and ensure the learner \
             understands not just the \"how\" but the \"why\"."
        ),
        "qa_engineer" => format!(
            "You are a QA engineer persona focused on comprehensive validation.\n\n\
             **Your approach:**\n\
             - Create exhaustive test suites covering all scenarios\n\
             - Identify edge cases, error conditions, and security vulnerabilities\n\
             - Validate against success criteria with precision\n\
             - Document test coverage and results\n\n\
             **Goal:** {goal}\n\n\
             **Context:** {context}\n\n\
             Ensure thorough testing with happy path, error handling, boundary \
             conditions, security, and performance tests."
        ),
        "architect" => format!(
            "You are an architect persona designing robust systems.\n\n\
             **Your approach:**\n\
             - Think holistically about system design and interfaces\n\
             - Consider scalability, maintainability, and extensibility\n\
             - Create clear architectural documentation and diagrams\n\
             - Define module boundaries and contracts\n\n\
             **Goal:** {goal}\n\n\
             **Context:** {context}\n\n\
             Focus on strategic design decisions and long-term system health."
        ),
        "junior_dev" => format!(
            "You are a junior developer persona focused on implementation.\n\n\
             **Your approach:**\n\
             - Follow specifications and requirements closely\n\
             - Implement features step-by-step\n\
             - Write clean, working code\n\
             - Ask questions when requirements are unclear\n\n\
             **Goal:** {goal}\n\n\
             **Context:** {context}\n\n\
             Focus on delivering working code that meets the stated requirements."
        ),
        _ => format!("**Goal:** {goal}\n\n**Context:** {context}"),
    }
}

/// Format a prompt for GitHub Copilot.
pub fn format_copilot_prompt(goal: &str, persona: &str, context: &str) -> String {
    let prefix = match persona {
        "guide" => "As a teaching guide, ",
        "qa_engineer" => "As a QA engineer, ",
        "architect" => "As a software architect, ",
        "junior_dev" => "As a developer, ",
        _ => "",
    };
    let ctx = if context.is_empty() {
        String::new()
    } else {
        format!("\n\nContext: {context}")
    };
    format!("{prefix}{goal}{ctx}")
}

/// Format a prompt for Microsoft Amplifier.
pub fn format_amplifier_prompt(goal: &str, persona: &str, context: &str) -> String {
    let ctx = if context.is_empty() {
        String::new()
    } else {
        format!("\nContext: {context}")
    };
    format!("Goal: {goal}{ctx}\n\nPersona: {persona}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_guide_prompt_contains_goal() {
        let p = format_claude_prompt("build API", "guide", "REST service");
        assert!(p.contains("build API"));
        assert!(p.contains("REST service"));
        assert!(p.contains("guide persona"));
    }

    #[test]
    fn claude_qa_prompt() {
        let p = format_claude_prompt("test it", "qa_engineer", "ctx");
        assert!(p.contains("QA engineer"));
    }

    #[test]
    fn claude_architect_prompt() {
        let p = format_claude_prompt("design", "architect", "ctx");
        assert!(p.contains("architect persona"));
    }

    #[test]
    fn claude_junior_dev_prompt() {
        let p = format_claude_prompt("code it", "junior_dev", "ctx");
        assert!(p.contains("junior developer"));
    }

    #[test]
    fn claude_unknown_persona_fallback() {
        let p = format_claude_prompt("goal", "unknown", "ctx");
        assert!(p.contains("**Goal:** goal"));
    }

    #[test]
    fn copilot_prompt_with_persona() {
        let p = format_copilot_prompt("fix bug", "qa_engineer", "prod crash");
        assert!(p.starts_with("As a QA engineer, "));
        assert!(p.contains("fix bug"));
        assert!(p.contains("prod crash"));
    }

    #[test]
    fn copilot_prompt_empty_context() {
        let p = format_copilot_prompt("do stuff", "guide", "");
        assert!(!p.contains("Context:"));
    }

    #[test]
    fn amplifier_prompt_format() {
        let p = format_amplifier_prompt("goal", "architect", "some ctx");
        assert!(p.contains("Goal: goal"));
        assert!(p.contains("Persona: architect"));
        assert!(p.contains("Context: some ctx"));
    }

    #[test]
    fn amplifier_prompt_no_context() {
        let p = format_amplifier_prompt("g", "p", "");
        assert!(!p.contains("Context:"));
    }
}
