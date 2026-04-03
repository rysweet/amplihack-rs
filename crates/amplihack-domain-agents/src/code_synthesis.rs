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

    pub fn generate(&self, _spec: &CodeSpec) -> Result<GeneratedCode> {
        todo!()
    }

    pub fn refactor(&self, _code: &str) -> Result<GeneratedCode> {
        todo!()
    }

    pub fn analyze(&self, _code: &str) -> Result<CodeAnalysis> {
        todo!()
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
