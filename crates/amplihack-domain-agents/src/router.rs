use crate::error::Result;
use crate::models::{DomainAgentType, RoutingDecision};
use amplihack_workflows::provenance::{self, ProvenanceEntry};
use std::path::PathBuf;

pub struct IntentRouter {
    confidence_threshold: f64,
    /// Base directory for provenance logs. When `None`, logging is disabled.
    log_base_dir: Option<PathBuf>,
}

impl IntentRouter {
    pub fn new(confidence_threshold: f64) -> Result<Self> {
        if !(0.0..=1.0).contains(&confidence_threshold) {
            return Err(crate::error::DomainError::InvalidInput(format!(
                "confidence_threshold must be 0.0..=1.0, got {confidence_threshold}"
            )));
        }
        Ok(Self {
            confidence_threshold,
            log_base_dir: None,
        })
    }

    pub fn with_defaults() -> Self {
        // SAFETY: 0.5 is always in range 0.0..=1.0
        Self::new(0.5).unwrap()
    }

    /// Enable provenance logging to the given base directory.
    pub fn with_log_dir(mut self, base_dir: impl Into<PathBuf>) -> Self {
        self.log_base_dir = Some(base_dir.into());
        self
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
        self.log_decision(input, &decision);
        Ok(decision)
    }

    pub fn route_with_context(&self, input: &str, context: &str) -> Result<RoutingDecision> {
        let combined = format!("{} {}", input, context).to_lowercase();
        let mut decision = Self::classify(&combined)?;
        if !context.is_empty() && decision.confidence < 1.0 {
            decision.confidence = (decision.confidence + 0.05).min(1.0);
        }
        if decision.confidence < self.confidence_threshold {
            decision.agent_type = DomainAgentType::Teaching;
            decision.reasoning = format!(
                "Confidence {:.2} below threshold {:.2}; defaulting to teaching",
                decision.confidence, self.confidence_threshold
            );
        }
        self.log_decision(input, &decision);
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

    fn log_decision(&self, input: &str, decision: &RoutingDecision) {
        if let Some(base) = &self.log_base_dir {
            let entry = ProvenanceEntry::new(
                "routing_decision",
                format!("{:?}", decision.agent_type),
                &decision.reasoning,
                decision.confidence,
                vec![],
                input,
            );
            provenance::log_routing_decision(base, &entry);
        }
    }

    fn classify(text: &str) -> Result<RoutingDecision> {
        let density = |keywords: &[&str]| -> f64 {
            let matches = keywords.iter().filter(|kw| text.contains(**kw)).count();
            matches as f64 / keywords.len() as f64
        };

        let categories: &[(&[&str], DomainAgentType, &str)] = &[
            (Self::SECURITY_KEYWORDS, DomainAgentType::Security, "security"),
            (Self::CODE_KEYWORDS, DomainAgentType::CodeSynthesis, "code"),
            (Self::TEACHING_KEYWORDS, DomainAgentType::Teaching, "teaching"),
            (Self::LEARNING_KEYWORDS, DomainAgentType::Learning, "learning"),
        ];

        let mut best_type = DomainAgentType::Teaching;
        let mut best_confidence: f64 = 0.0;
        let mut best_label = "none";

        for &(keywords, ref agent_type, label) in categories {
            let d = density(keywords);
            if d > best_confidence {
                best_confidence = d;
                best_type = agent_type.clone();
                best_label = label;
            }
        }

        if best_confidence > 0.0 {
            Ok(RoutingDecision {
                agent_type: best_type,
                confidence: best_confidence,
                reasoning: format!("Input contains {best_label}-related keywords"),
            })
        } else {
            Ok(RoutingDecision {
                agent_type: DomainAgentType::Teaching,
                confidence: 0.0,
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
        let router = IntentRouter::new(0.75).unwrap();
        assert!((router.confidence_threshold() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn new_rejects_out_of_range() {
        assert!(IntentRouter::new(-0.1).is_err());
        assert!(IntentRouter::new(1.1).is_err());
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
