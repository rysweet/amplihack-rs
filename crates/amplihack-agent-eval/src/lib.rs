//! Progressive evaluation framework for amplihack agents (L1-L12).
//!
//! Ports the Python `eval/` module providing grading, progressive test suites,
//! harness execution, and self-improvement loops.

pub mod error;
pub mod grader;
pub mod harness;
pub mod levels;
pub mod models;
pub mod progressive;
pub mod self_improve;

pub use error::EvalError;
pub use grader::{Grader, SimpleGrader};
pub use harness::HarnessRunner;
pub use levels::TestLevel;
pub use models::{
    GradeResult, HarnessConfig, LevelResult, ProgressiveConfig, ProgressiveResult,
    SelfImproveConfig, TestCase, TestQuestion,
};
pub use progressive::ProgressiveSuite;
pub use self_improve::{ErrorAnalyzer, PatchProposer, ReviewerVoting, SelfImproveRunner};
