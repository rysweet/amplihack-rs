pub mod code_synthesis;
pub mod error;
pub mod learning;
pub mod models;
pub mod router;
pub mod security;
pub mod teaching;

pub use code_synthesis::CodeSynthesizer;
pub use error::{DomainError, Result};
pub use learning::LearningAgent;
pub use models::{
    Answer, AuditReport, CodeAnalysis, CodeSpec, CodeSynthesisConfig, DomainAgentType,
    EvaluationResult, GeneratedCode, LearnedContent, LearningConfig, QuizQuestion, RiskAssessment,
    RoutingDecision, SecurityConfig, TeachingConfig, TeachingResult, Vulnerability,
};
pub use router::IntentRouter;
pub use security::SecurityAuditor;
pub use teaching::TeachingAgent;