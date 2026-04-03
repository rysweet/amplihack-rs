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

    pub fn route(&self, _input: &str) -> Result<RoutingDecision> {
        todo!()
    }

    pub fn route_with_context(&self, _input: &str, _context: &str) -> Result<RoutingDecision> {
        todo!()
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
