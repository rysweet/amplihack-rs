use crate::error::{DomainError, Result};
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
        let description = spec.description.trim();
        let language = spec.language.trim();

        if description.is_empty() || language.is_empty() {
            return Err(DomainError::InvalidInput(
                "code spec description and language must not be empty".to_string(),
            ));
        }

        Err(DomainError::CodeSynthesis(format!(
            "code synthesis backend not available: cannot synthesize {language}"
        )))
    }

    pub fn refactor(&self, code: &str) -> Result<GeneratedCode> {
        if code.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "code to refactor must not be empty".to_string(),
            ));
        }

        Err(DomainError::CodeSynthesis(
            "refactoring backend not available".to_string(),
        ))
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

    // ── generate: honest error taxonomy (issue #874) ────────────────────────

    #[test]
    fn generate_empty_description_is_invalid_input() {
        let synth = CodeSynthesizer::with_defaults();
        let spec = CodeSpec {
            description: "   ".to_string(),
            language: "rust".to_string(),
            constraints: vec![],
        };
        let err = synth.generate(&spec).unwrap_err();
        assert!(
            matches!(err, DomainError::InvalidInput(_)),
            "empty description must be InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn generate_empty_language_is_invalid_input() {
        let synth = CodeSynthesizer::with_defaults();
        let spec = CodeSpec {
            description: "a valid description".to_string(),
            language: "  ".to_string(),
            constraints: vec![],
        };
        let err = synth.generate(&spec).unwrap_err();
        assert!(
            matches!(err, DomainError::InvalidInput(_)),
            "empty language must be InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn generate_wellformed_is_code_synthesis_error() {
        let synth = CodeSynthesizer::with_defaults();
        let spec = CodeSpec {
            description: "add two numbers".to_string(),
            language: "rust".to_string(),
            constraints: vec!["generic".to_string()],
        };
        match synth.generate(&spec) {
            Err(DomainError::CodeSynthesis(msg)) => {
                assert!(
                    msg.contains("rust"),
                    "message should name the language: {msg}"
                );
                // SR-4: never echo caller-supplied spec content.
                assert!(
                    !msg.contains("add two numbers"),
                    "message must not echo the description: {msg}"
                );
                assert!(
                    !msg.contains("generic"),
                    "message must not echo constraints: {msg}"
                );
            }
            other => panic!("expected CodeSynthesis error, got {other:?}"),
        }
    }

    #[test]
    fn generate_trims_language_in_message() {
        let synth = CodeSynthesizer::with_defaults();
        let spec = CodeSpec {
            description: "add two numbers".to_string(),
            language: "  Rust  ".to_string(),
            constraints: vec![],
        };
        match synth.generate(&spec) {
            Err(DomainError::CodeSynthesis(msg)) => {
                assert!(
                    msg.ends_with("cannot synthesize Rust"),
                    "language token must be trimmed (case preserved): {msg}"
                );
            }
            other => panic!("expected CodeSynthesis error, got {other:?}"),
        }
    }

    #[test]
    fn generate_never_leaks_secret_from_spec() {
        let synth = CodeSynthesizer::with_defaults();
        let spec = CodeSpec {
            description: "SECRET-DESCRIPTION-TOKEN".to_string(),
            language: "rust".to_string(),
            constraints: vec!["SECRET-CONSTRAINT-TOKEN".to_string()],
        };
        let err = synth.generate(&spec).unwrap_err();
        let text = err.to_string();
        assert!(
            !text.contains("SECRET-DESCRIPTION-TOKEN"),
            "SR-4: leaked description into error: {text}"
        );
        assert!(
            !text.contains("SECRET-CONSTRAINT-TOKEN"),
            "SR-4: leaked constraint into error: {text}"
        );
    }

    // ── refactor: honest error taxonomy (issue #874) ────────────────────────

    #[test]
    fn refactor_empty_is_invalid_input() {
        let synth = CodeSynthesizer::with_defaults();
        let err = synth.refactor("   \n\t ").unwrap_err();
        assert!(
            matches!(err, DomainError::InvalidInput(_)),
            "empty code must be InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn refactor_nonempty_is_code_synthesis_error() {
        let synth = CodeSynthesizer::with_defaults();
        let err = synth
            .refactor("fn body() { let SECRET_MARKER = 1; }")
            .unwrap_err();
        assert!(
            matches!(err, DomainError::CodeSynthesis(_)),
            "non-empty code must be CodeSynthesis, got {err:?}"
        );
        // SR-4: never echo the caller-supplied code body.
        assert!(
            !err.to_string().contains("SECRET_MARKER"),
            "message must not echo the code body: {err}"
        );
    }
}
