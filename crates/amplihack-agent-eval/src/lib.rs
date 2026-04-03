//! Progressive evaluation framework for amplihack agents (L1-L12).
//!
//! Ports the Python `eval/` module providing grading, progressive test suites,
//! harness execution, self-improvement loops, domain evaluation, LLM grading,
//! and agent adapters.

pub mod agent_adapter;
pub mod domain_eval;
pub mod error;
pub mod grader;
pub mod harness;
pub mod levels;
pub mod llm_grader;
pub mod models;
pub mod progressive;
pub mod self_improve;

pub use agent_adapter::{AgentAdapter, AgentResponse, MockAgentAdapter, SubprocessAdapter};
pub use domain_eval::{
    DomainEvalAgent, DomainEvalHarness, EvalReport, EvalScenario, ScenarioResult,
};
pub use error::EvalError;
pub use grader::{Grader, SimpleGrader};
pub use harness::HarnessRunner;
pub use levels::TestLevel;
pub use llm_grader::{LlmGrader, StubLlmGrader, extract_json, get_grader_model};
pub use models::{
    GradeResult, HarnessConfig, LevelResult, ProgressiveConfig, ProgressiveResult,
    SelfImproveConfig, TestCase, TestQuestion,
};
pub use progressive::ProgressiveSuite;
pub use self_improve::{ErrorAnalyzer, PatchProposer, ReviewerVoting, SelfImproveRunner};
