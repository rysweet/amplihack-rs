//! Progressive evaluation framework for amplihack agents (L1-L12).
//!
//! Ports the Python `eval/` module providing grading, progressive test suites,
//! harness execution, self-improvement loops, domain evaluation, LLM grading,
//! agent adapters, long-horizon memory, general capabilities, security log
//! evaluation, and teaching assessment.

pub mod agent_adapter;
pub mod domain_eval;
pub mod error;
pub mod general_capability;
pub mod grader;
pub mod harness;
pub mod levels;
pub mod llm_grader;
pub mod long_horizon;
pub mod models;
pub mod progressive;
pub mod security_log;
pub mod self_improve;
pub(crate) mod self_improve_helpers;
pub mod teaching_eval;

pub use agent_adapter::{AgentAdapter, AgentResponse, MockAgentAdapter, SubprocessAdapter};
pub use domain_eval::{
    DomainEvalAgent, DomainEvalHarness, EvalReport, EvalScenario, ScenarioResult,
};
pub use error::EvalError;
pub use general_capability::{CapabilityReport, EvalTypeResult, ToolTrajectory};
pub use grader::{Grader, SimpleGrader};
pub use harness::HarnessRunner;
pub use levels::TestLevel;
pub use llm_grader::{LlmGrader, StubLlmGrader, extract_json, get_grader_model};
pub use long_horizon::{
    DimensionScore, LongHorizonConfig, LongHorizonReport,
};
pub use models::{
    GradeResult, HarnessConfig, LevelResult, ProgressiveConfig, ProgressiveResult,
    SelfImproveConfig, TestCase, TestQuestion,
};
pub use progressive::ProgressiveSuite;
pub use security_log::{AttackCampaign, SecurityEvalReport, SecurityGradeResult, SecurityQuestion};
pub use self_improve::{ErrorAnalyzer, PatchProposer, ReviewerVoting, SelfImproveRunner};
pub use teaching_eval::{TeachingDimensionScore, TeachingEvalResult, TeachingResult};
