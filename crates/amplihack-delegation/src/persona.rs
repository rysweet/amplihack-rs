use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// PersonaStrategy
// ---------------------------------------------------------------------------

/// Describes how a delegation persona communicates, collects evidence, and
/// frames prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaStrategy {
    /// Unique machine-readable name (e.g. `"guide"`).
    pub name: String,
    /// Communication style keyword (e.g. `"socratic"`, `"precise"`).
    pub communication_style: String,
    /// How thorough this persona is (e.g. `"balanced"`, `"exhaustive"`).
    pub thoroughness_level: String,
    /// Ordered priority list of evidence types to collect.
    pub evidence_collection_priority: Vec<String>,
    /// Prompt template with `{goal}` and `{success_criteria}` placeholders.
    pub prompt_template: String,
}

// ---------------------------------------------------------------------------
// Built-in personas
// ---------------------------------------------------------------------------

/// Teaching-focused guide persona using Socratic method.
pub fn guide() -> PersonaStrategy {
    PersonaStrategy {
        name: "guide".into(),
        communication_style: "socratic".into(),
        thoroughness_level: "balanced".into(),
        evidence_collection_priority: vec![
            "documentation".into(),
            "architecture_doc".into(),
            "code_file".into(),
            "test_file".into(),
            "diagram".into(),
        ],
        prompt_template: concat!(
            "You are a teaching guide. Your approach is Socratic — ask probing questions,\n",
            "guide the learner toward understanding, and build knowledge step by step.\n\n",
            "Goal: {goal}\n\n",
            "Success Criteria:\n{success_criteria}\n\n",
            "Approach:\n",
            "1. Understand the current state of the codebase\n",
            "2. Identify knowledge gaps\n",
            "3. Guide through implementation with explanations\n",
            "4. Validate understanding at each step\n",
            "5. Collect evidence of progress\n",
            "6. Ensure all success criteria are met",
        )
        .into(),
    }
}

/// Validation-focused QA engineer persona.
pub fn qa_engineer() -> PersonaStrategy {
    PersonaStrategy {
        name: "qa_engineer".into(),
        communication_style: "precise".into(),
        thoroughness_level: "exhaustive".into(),
        evidence_collection_priority: vec![
            "test_file".into(),
            "test_results".into(),
            "validation_report".into(),
            "code_file".into(),
            "execution_log".into(),
        ],
        prompt_template: concat!(
            "You are a QA engineer. Your approach is precise and exhaustive — test every\n",
            "requirement, verify every edge case, and document every finding.\n\n",
            "Goal: {goal}\n\n",
            "Success Criteria:\n{success_criteria}\n\n",
            "Approach:\n",
            "1. Analyze requirements and identify test scenarios\n",
            "2. Create comprehensive test plan\n",
            "3. Execute tests systematically\n",
            "4. Document results with evidence\n",
            "5. Verify edge cases and error handling\n",
            "6. Provide detailed validation report",
        )
        .into(),
    }
}

/// System-design-focused architect persona.
pub fn architect() -> PersonaStrategy {
    PersonaStrategy {
        name: "architect".into(),
        communication_style: "strategic".into(),
        thoroughness_level: "holistic".into(),
        evidence_collection_priority: vec![
            "architecture_doc".into(),
            "api_spec".into(),
            "diagram".into(),
            "design_doc".into(),
            "code_file".into(),
        ],
        prompt_template: concat!(
            "You are a software architect. Your approach is strategic and holistic — consider\n",
            "the system as a whole, design for maintainability, and document decisions.\n\n",
            "Goal: {goal}\n\n",
            "Success Criteria:\n{success_criteria}\n\n",
            "Approach:\n",
            "1. Analyze the current architecture\n",
            "2. Identify design constraints and trade-offs\n",
            "3. Propose and document architectural decisions\n",
            "4. Implement with extensibility in mind\n",
            "5. Create architectural documentation\n",
            "6. Validate against quality attributes",
        )
        .into(),
    }
}

/// Task-focused junior developer persona.
pub fn junior_dev() -> PersonaStrategy {
    PersonaStrategy {
        name: "junior_dev".into(),
        communication_style: "task_focused".into(),
        thoroughness_level: "adequate".into(),
        evidence_collection_priority: vec![
            "code_file".into(),
            "test_file".into(),
            "configuration".into(),
            "documentation".into(),
        ],
        prompt_template: concat!(
            "You are a junior developer. Your approach is task-focused — implement the\n",
            "requirements step by step, write tests, and ask for help when stuck.\n\n",
            "Goal: {goal}\n\n",
            "Success Criteria:\n{success_criteria}\n\n",
            "Approach:\n",
            "1. Break down the goal into small tasks\n",
            "2. Implement each task with tests\n",
            "3. Validate against success criteria\n",
            "4. Document what was done\n",
            "5. Request review when complete",
        )
        .into(),
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

static PERSONA_REGISTRY: LazyLock<RwLock<HashMap<String, PersonaStrategy>>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert("guide".into(), guide());
    m.insert("qa_engineer".into(), qa_engineer());
    m.insert("architect".into(), architect());
    m.insert("junior_dev".into(), junior_dev());
    RwLock::new(m)
});

/// Look up a persona by name. Returns `None` if unknown.
pub fn get_persona(name: &str) -> Option<PersonaStrategy> {
    PERSONA_REGISTRY
        .read()
        .ok()
        .and_then(|reg| reg.get(name).cloned())
}

/// Register (or overwrite) a custom persona.
pub fn register_persona(strategy: PersonaStrategy) {
    if let Ok(mut reg) = PERSONA_REGISTRY.write() {
        reg.insert(strategy.name.clone(), strategy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_guide_exists() {
        let p = get_persona("guide").expect("guide should exist");
        assert_eq!(p.name, "guide");
        assert_eq!(p.communication_style, "socratic");
        assert_eq!(p.thoroughness_level, "balanced");
        assert!(p.prompt_template.contains("{goal}"));
        assert!(p.prompt_template.contains("{success_criteria}"));
    }

    #[test]
    fn builtin_qa_engineer_exists() {
        let p = get_persona("qa_engineer").expect("qa_engineer should exist");
        assert_eq!(p.communication_style, "precise");
        assert_eq!(p.thoroughness_level, "exhaustive");
    }

    #[test]
    fn builtin_architect_exists() {
        let p = get_persona("architect").expect("architect should exist");
        assert_eq!(p.communication_style, "strategic");
        assert_eq!(p.thoroughness_level, "holistic");
    }

    #[test]
    fn builtin_junior_dev_exists() {
        let p = get_persona("junior_dev").expect("junior_dev should exist");
        assert_eq!(p.communication_style, "task_focused");
        assert_eq!(p.thoroughness_level, "adequate");
    }

    #[test]
    fn unknown_persona_returns_none() {
        assert!(get_persona("nonexistent").is_none());
    }

    #[test]
    fn register_custom_persona() {
        let custom = PersonaStrategy {
            name: "custom_test_persona".into(),
            communication_style: "verbose".into(),
            thoroughness_level: "minimal".into(),
            evidence_collection_priority: vec!["code_file".into()],
            prompt_template: "Do {goal} per {success_criteria}".into(),
        };
        register_persona(custom);
        let p = get_persona("custom_test_persona").expect("custom should exist after register");
        assert_eq!(p.communication_style, "verbose");
    }

    #[test]
    fn evidence_priorities_match_python() {
        let g = guide();
        assert_eq!(g.evidence_collection_priority[0], "documentation");

        let qa = qa_engineer();
        assert_eq!(qa.evidence_collection_priority[0], "test_file");

        let a = architect();
        assert_eq!(a.evidence_collection_priority[0], "architecture_doc");

        let j = junior_dev();
        assert_eq!(j.evidence_collection_priority[0], "code_file");
    }
}
