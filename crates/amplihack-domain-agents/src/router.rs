use crate::error::Result;
use crate::models::{DomainAgentType, RoutingDecision};

pub struct IntentRouter {
    confidence_threshold: f64,
}

impl IntentRouter {
    pub fn new(confidence_threshold: f64) -> Self {
        Self {
            confidence_threshold,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(0.5)
    }

    pub fn confidence_threshold(&self) -> f64 {
        self.confidence_threshold
    }

    pub fn route(&self, input: &str) -> Result<RoutingDecision> {
        let lower = input.to_lowercase();
        let mut decision = Self::classify(&lower)?;
        if decision.confidence < self.confidence_threshold {
            decision.agent_type = DomainAgentType::Teaching;
            decision.reasoning = format!(
                "Confidence {:.2} below threshold {:.2}; defaulting to teaching",
                decision.confidence, self.confidence_threshold
            );
        }
        Ok(decision)
    }

    pub fn route_with_context(&self, input: &str, context: &str) -> Result<RoutingDecision> {
        let combined = format!("{} {}", input, context).to_lowercase();
        let mut decision = Self::classify(&combined)?;
        // Context reinforcement: bump confidence slightly when context is non-empty
        if !context.is_empty() && decision.confidence < 1.0 {
            decision.confidence = (decision.confidence + 0.05).min(1.0);
        }
        Ok(decision)
    }

    const SECURITY_KEYWORDS: &'static [&'static str] = &[
        "vulnerability",
        "security",
        "audit",
        "exploit",
        "injection",
        "xss",
        "csrf",
        "unsafe",
        "risk",
    ];
    const CODE_KEYWORDS: &'static [&'static str] = &[
        "code",
        "function",
        "class",
        "implement",
        "refactor",
        "generate",
        "program",
        "compile",
        "syntax",
        "algorithm",
    ];
    const TEACHING_KEYWORDS: &'static [&'static str] = &[
        "teach",
        "explain",
        "lesson",
        "tutorial",
        "learn",
        "understand",
        "concept",
        "course",
    ];
    const LEARNING_KEYWORDS: &'static [&'static str] = &[
        "remember",
        "recall",
        "store",
        "memorize",
        "knowledge",
        "fact",
        "note",
    ];

    fn classify(text: &str) -> Result<RoutingDecision> {
        let has = |keywords: &[&str]| keywords.iter().any(|kw| text.contains(kw));

        if has(Self::SECURITY_KEYWORDS) {
            Ok(RoutingDecision {
                agent_type: DomainAgentType::Security,
                confidence: 1.0,
                reasoning: "Input contains security-related keywords".into(),
            })
        } else if has(Self::CODE_KEYWORDS) {
            Ok(RoutingDecision {
                agent_type: DomainAgentType::CodeSynthesis,
                confidence: 1.0,
                reasoning: "Input contains code-related keywords".into(),
            })
        } else if has(Self::TEACHING_KEYWORDS) {
            Ok(RoutingDecision {
                agent_type: DomainAgentType::Teaching,
                confidence: 1.0,
                reasoning: "Input contains teaching-related keywords".into(),
            })
        } else if has(Self::LEARNING_KEYWORDS) {
            Ok(RoutingDecision {
                agent_type: DomainAgentType::Learning,
                confidence: 1.0,
                reasoning: "Input contains learning-related keywords".into(),
            })
        } else {
            Ok(RoutingDecision {
                agent_type: DomainAgentType::Teaching,
                confidence: 0.7,
                reasoning: "No specific keywords matched; defaulting to teaching".into(),
            })
        }
    }

    pub fn supported_types(&self) -> Vec<DomainAgentType> {
        vec![
            DomainAgentType::Teaching,
            DomainAgentType::Security,
            DomainAgentType::CodeSynthesis,
            DomainAgentType::Learning,
            DomainAgentType::Research,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_threshold() {
        let router = IntentRouter::new(0.75);
        assert!((router.confidence_threshold() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn with_defaults() {
        let router = IntentRouter::with_defaults();
        assert!((router.confidence_threshold() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn supported_types_returns_all() {
        let router = IntentRouter::with_defaults();
        let types = router.supported_types();
        assert_eq!(types.len(), 5);
        assert!(types.contains(&DomainAgentType::Teaching));
        assert!(types.contains(&DomainAgentType::Security));
        assert!(types.contains(&DomainAgentType::CodeSynthesis));
        assert!(types.contains(&DomainAgentType::Learning));
        assert!(types.contains(&DomainAgentType::Research));
    }
}
