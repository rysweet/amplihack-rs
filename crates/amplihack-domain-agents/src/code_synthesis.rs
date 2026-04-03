use crate::error::Result;
use crate::models::{CodeAnalysis, CodeSpec, CodeSynthesisConfig, GeneratedCode};

pub struct CodeSynthesizer {
    config: CodeSynthesisConfig,
}

impl CodeSynthesizer {
    pub fn new(config: CodeSynthesisConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(CodeSynthesisConfig::default())
    }

    pub fn config(&self) -> &CodeSynthesisConfig {
        &self.config
    }

    pub fn generate(&self, spec: &CodeSpec) -> Result<GeneratedCode> {
        let constraints_text = if spec.constraints.is_empty() {
            String::new()
        } else {
            format!("\n// Constraints: {}", spec.constraints.join(", "))
        };

        let code = match spec.language.to_lowercase().as_str() {
            "rust" => format!(
                "// {}{constraints_text}\nfn generated() {{\n    todo!(\"Implement: {}\")\n}}",
                spec.description, spec.description
            ),
            "python" => format!(
                "# {}{constraints_text}\ndef generated():\n    raise NotImplementedError(\"Implement: {}\")",
                spec.description, spec.description
            ),
            _ => format!(
                "// {}{constraints_text}\n// Language: {}\n// TODO: Implement {}",
                spec.description, spec.language, spec.description
            ),
        };

        Ok(GeneratedCode {
            code,
            language: spec.language.clone(),
            explanation: format!("Generated template for: {}", spec.description),
        })
    }

    pub fn refactor(&self, code: &str) -> Result<GeneratedCode> {
        let trimmed = code.trim().to_string();
        let refactored = if trimmed.is_empty() {
            "// Empty code — nothing to refactor".to_string()
        } else {
            format!("// TODO: Add documentation\n{trimmed}")
        };

        Ok(GeneratedCode {
            code: refactored,
            language: self.config.language.clone(),
            explanation: "Added documentation placeholder and trimmed whitespace".into(),
        })
    }

    pub fn analyze(&self, code: &str) -> Result<CodeAnalysis> {
        let lines: usize = code.lines().count();
        let fn_count = code
            .split_whitespace()
            .filter(|w| *w == "fn" || *w == "def" || *w == "function")
            .count();

        let complexity = (fn_count as u32).saturating_add(lines as u32 / 10);

        let mut issues = Vec::new();
        let mut suggestions = Vec::new();

        if lines > 100 {
            issues.push("File exceeds 100 lines".into());
            suggestions.push("Consider splitting into smaller modules".into());
        }
        if fn_count == 0 && !code.trim().is_empty() {
            issues.push("No functions detected".into());
            suggestions.push("Consider organizing code into functions".into());
        }
        if code.contains("unwrap()") {
            issues.push("Use of unwrap() detected".into());
            suggestions.push("Replace unwrap() with proper error handling".into());
        }

        Ok(CodeAnalysis {
            complexity,
            issues,
            suggestions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_config() {
        let config = CodeSynthesisConfig {
            language: "python".into(),
            style: "pep8".into(),
            max_complexity: 5,
        };
        let synth = CodeSynthesizer::new(config.clone());
        assert_eq!(synth.config(), &config);
    }

    #[test]
    fn with_defaults() {
        let synth = CodeSynthesizer::with_defaults();
        assert_eq!(synth.config().language, "rust");
        assert_eq!(synth.config().style, "idiomatic");
    }
}
