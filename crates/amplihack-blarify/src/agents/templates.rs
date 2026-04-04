//! Prompt template definitions for LLM-based analysis.
//!
//! Mirrors the Python `agents/prompt_templates/` module.

use std::collections::HashMap;

use anyhow::{Result, bail};
use tracing::warn;

/// A prompt template with system and input prompts and named variables.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub input_prompt: String,
    pub variables: Vec<String>,
}

impl PromptTemplate {
    /// Create a new prompt template.
    pub fn new(
        name: &str,
        description: &str,
        system_prompt: &str,
        input_prompt: &str,
        variables: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            system_prompt: system_prompt.into(),
            input_prompt: input_prompt.into(),
            variables,
        }
    }

    /// Get the system and input prompts as raw templates.
    pub fn get_prompts(&self) -> (&str, &str) {
        (&self.system_prompt, &self.input_prompt)
    }

    /// Validate that all required variables are provided.
    pub fn validate_variables(&self, variables: &HashMap<String, String>) -> bool {
        let missing: Vec<_> = self
            .variables
            .iter()
            .filter(|v| !variables.contains_key(v.as_str()))
            .collect();
        if !missing.is_empty() {
            warn!(
                template = %self.name,
                missing = ?missing,
                "Missing required variables"
            );
            return false;
        }
        true
    }

    /// Format the template by substituting variables into the system prompt.
    pub fn format(&self, variables: &HashMap<String, String>) -> Result<String> {
        if !self.validate_variables(variables) {
            bail!("Missing required template variables");
        }
        let mut result = self.system_prompt.clone();
        for (key, value) in variables {
            let placeholder = format!("{{{key}}}");
            result = result.replace(&placeholder, value);
        }
        Ok(result)
    }
}

// --- Built-in prompt templates ---

/// System overview analysis template.
pub fn system_overview_template() -> PromptTemplate {
    PromptTemplate::new(
        "system_overview",
        "Analyze the codebase structure and generate a system overview",
        concat!(
            "You are an expert software architect. Analyze the following codebase structure ",
            "and framework information to produce a comprehensive system overview.\n\n",
            "Codebase Structure:\n{codebase_skeleton}\n\n",
            "Framework Information:\n{framework_info}\n\n",
            "Respond with a JSON object containing:\n",
            "- executive_summary: Brief overview of the system\n",
            "- business_domain: What business domain this serves\n",
            "- primary_purpose: Main purpose of the codebase\n",
            "- architecture: Architectural patterns used\n",
            "- technology_stack: Technologies and frameworks\n",
            "- core_components: Key components and their roles\n",
            "- data_flow: How data flows through the system\n",
            "- external_dependencies: External services and dependencies",
        ),
        "Analyze the codebase and produce the system overview.",
        vec!["codebase_skeleton".into(), "framework_info".into()],
    )
}

/// Component analysis template.
pub fn component_analysis_template() -> PromptTemplate {
    PromptTemplate::new(
        "component_analysis",
        "Analyze a specific code component in detail",
        concat!(
            "You are a senior software engineer. Analyze the following code component ",
            "and provide a detailed analysis.\n\n",
            "Component Code:\n{component_code}\n\n",
            "Context:\n{context}\n\n",
            "Provide analysis covering:\n",
            "- Purpose and responsibility\n",
            "- Key functionality\n",
            "- Dependencies and relationships\n",
            "- Design patterns used\n",
            "- Potential improvements",
        ),
        "Analyze the component.",
        vec!["component_code".into(), "context".into()],
    )
}

/// Component identification template.
pub fn component_identification_template() -> PromptTemplate {
    PromptTemplate::new(
        "component_identification",
        "Identify the most important components in a codebase",
        concat!(
            "You are an expert software architect. Identify the 5-10 most important ",
            "components in this codebase.\n\n",
            "Codebase Structure:\n{codebase_structure}\n\n",
            "Framework Info:\n{framework_info}\n\n",
            "System Overview:\n{system_overview}\n\n",
            "Return a JSON array where each item has:\n",
            "- name: Component name\n",
            "- path: File path\n",
            "- importance: high/medium/low\n",
            "- reason: Why it's important\n",
            "- type: Component type (service, controller, model, etc.)",
        ),
        "Identify the key components.",
        vec![
            "codebase_structure".into(),
            "framework_info".into(),
            "system_overview".into(),
        ],
    )
}

/// Relationship extraction template.
pub fn relationship_extraction_template() -> PromptTemplate {
    PromptTemplate::new(
        "relationship_extraction",
        "Extract relationships between components",
        concat!(
            "You are a software architect. Extract the relationships between these components.\n\n",
            "Components:\n{components}\n\n",
            "Codebase Structure:\n{codebase_structure}\n\n",
            "Component Analyses:\n{component_analyses}\n\n",
            "Return structured JSON with:\n",
            "- direct_dependencies: Direct import/usage relationships\n",
            "- data_flow: How data flows between components\n",
            "- communication_patterns: APIs, events, messaging\n",
            "- architectural_relationships: Layering, module boundaries\n",
            "- integration_points: External system connections",
        ),
        "Extract the relationships.",
        vec![
            "components".into(),
            "codebase_structure".into(),
            "component_analyses".into(),
        ],
    )
}

/// Framework detection template.
pub fn framework_detection_template() -> PromptTemplate {
    PromptTemplate::new(
        "framework_detection",
        "Detect the primary framework and technology stack",
        concat!(
            "Analyze the following project structure and identify:\n",
            "1. Primary framework (e.g., Django, React, Express)\n",
            "2. Framework version if detectable\n",
            "3. Technology stack components\n",
            "4. Main architectural folders\n",
            "5. Configuration files\n\n",
            "Project Structure:\n{project_structure}",
        ),
        "Detect the framework.",
        vec!["project_structure".into()],
    )
}

/// Leaf node analysis template for documentation generation.
pub fn leaf_node_analysis_template() -> PromptTemplate {
    PromptTemplate::new(
        "leaf_node_analysis",
        "Analyze a leaf node (function/class) for documentation",
        concat!(
            "Analyze the following code and provide a concise description.\n\n",
            "Name: {node_name}\n",
            "Type: {node_labels}\n",
            "Path: {node_path}\n\n",
            "Code:\n{node_content}\n\n",
            "Provide a clear, one-paragraph description of what this code does, ",
            "its purpose, parameters, return values, and any important behavior.",
        ),
        "Analyze the code.",
        vec![
            "node_name".into(),
            "node_labels".into(),
            "node_path".into(),
            "node_content".into(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_creation() {
        let t = system_overview_template();
        assert_eq!(t.name, "system_overview");
        assert_eq!(t.variables.len(), 2);
    }

    #[test]
    fn template_variable_validation() {
        let t = component_analysis_template();
        let mut vars = HashMap::new();
        assert!(!t.validate_variables(&vars));
        vars.insert("component_code".into(), "fn foo() {}".into());
        vars.insert("context".into(), "module context".into());
        assert!(t.validate_variables(&vars));
    }

    #[test]
    fn template_format_substitution() {
        let t = framework_detection_template();
        let mut vars = HashMap::new();
        vars.insert("project_structure".into(), "src/\n  main.rs".into());
        let result = t.format(&vars).unwrap();
        assert!(result.contains("src/\n  main.rs"));
        assert!(!result.contains("{project_structure}"));
    }

    #[test]
    fn template_format_missing_vars() {
        let t = component_analysis_template();
        let vars = HashMap::new();
        assert!(t.format(&vars).is_err());
    }

    #[test]
    fn all_builtin_templates_have_variables() {
        let templates = vec![
            system_overview_template(),
            component_analysis_template(),
            component_identification_template(),
            relationship_extraction_template(),
            framework_detection_template(),
            leaf_node_analysis_template(),
        ];
        for t in templates {
            assert!(
                !t.variables.is_empty(),
                "Template {} has no variables",
                t.name
            );
        }
    }
}
