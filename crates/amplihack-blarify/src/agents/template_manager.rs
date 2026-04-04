//! Prompt template manager for organizing and accessing templates.
//!
//! Mirrors the Python `agents/prompt_templates/template_manager.py`.

use std::collections::HashMap;

use tracing::warn;

use super::templates::{
    PromptTemplate, component_analysis_template, component_identification_template,
    framework_detection_template, leaf_node_analysis_template, relationship_extraction_template,
    system_overview_template,
};

/// Manages a registry of prompt templates.
#[derive(Debug)]
pub struct TemplateManager {
    templates: HashMap<String, PromptTemplate>,
}

impl TemplateManager {
    /// Create a new template manager with built-in templates.
    pub fn new() -> Self {
        let mut mgr = Self {
            templates: HashMap::new(),
        };
        mgr.initialize_templates();
        mgr
    }

    /// Register all built-in templates.
    fn initialize_templates(&mut self) {
        let builtins = vec![
            system_overview_template(),
            component_analysis_template(),
            component_identification_template(),
            relationship_extraction_template(),
            framework_detection_template(),
            leaf_node_analysis_template(),
        ];
        for t in builtins {
            self.templates.insert(t.name.clone(), t);
        }
    }

    /// Get a template by name.
    pub fn get_template(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.get(name)
    }

    /// List all registered template names.
    pub fn list_templates(&self) -> Vec<&str> {
        self.templates.keys().map(String::as_str).collect()
    }

    /// Add a custom template.
    pub fn add_template(&mut self, template: PromptTemplate) {
        self.templates.insert(template.name.clone(), template);
    }

    /// Remove a template by name.
    pub fn remove_template(&mut self, name: &str) -> bool {
        self.templates.remove(name).is_some()
    }

    /// Format a template with the given variables.
    pub fn format_template(
        &self,
        name: &str,
        variables: &HashMap<String, String>,
    ) -> Option<String> {
        let template = self.templates.get(name)?;
        match template.format(variables) {
            Ok(result) => Some(result),
            Err(e) => {
                warn!(template = name, error = %e, "Failed to format template");
                None
            }
        }
    }

    /// Validate template variables without formatting.
    pub fn validate_template_variables(
        &self,
        name: &str,
        variables: &HashMap<String, String>,
    ) -> bool {
        match self.templates.get(name) {
            Some(template) => template.validate_variables(variables),
            None => false,
        }
    }
}

impl Default for TemplateManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_has_builtin_templates() {
        let mgr = TemplateManager::new();
        let names = mgr.list_templates();
        assert!(names.contains(&"system_overview"));
        assert!(names.contains(&"component_analysis"));
        assert!(names.contains(&"framework_detection"));
    }

    #[test]
    fn manager_get_template() {
        let mgr = TemplateManager::new();
        let t = mgr.get_template("system_overview").unwrap();
        assert_eq!(t.name, "system_overview");
    }

    #[test]
    fn manager_add_and_remove() {
        let mut mgr = TemplateManager::new();
        let custom = PromptTemplate::new("custom", "Custom template", "sys", "input", vec![]);
        mgr.add_template(custom);
        assert!(mgr.get_template("custom").is_some());
        assert!(mgr.remove_template("custom"));
        assert!(mgr.get_template("custom").is_none());
    }

    #[test]
    fn manager_format_template() {
        let mgr = TemplateManager::new();
        let mut vars = HashMap::new();
        vars.insert("project_structure".into(), "src/ lib/".into());
        let result = mgr.format_template("framework_detection", &vars);
        assert!(result.is_some());
        assert!(result.unwrap().contains("src/ lib/"));
    }

    #[test]
    fn manager_nonexistent_template() {
        let mgr = TemplateManager::new();
        assert!(mgr.get_template("nonexistent").is_none());
        assert!(!mgr.validate_template_variables("nonexistent", &HashMap::new()));
    }
}
