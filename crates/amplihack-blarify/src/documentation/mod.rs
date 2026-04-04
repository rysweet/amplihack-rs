//! Documentation generation subsystem.
//!
//! Provides bottom-up documentation creation, workflow discovery,
//! and graph queries for the documentation layer.

pub mod batch;
pub mod creator;
pub mod models;
pub mod queries;
pub mod workflow;

pub use batch::{BatchConfig, BottomUpBatchProcessor};
pub use creator::DocumentationCreator;
pub use models::{
    DocumentationResult, FrameworkDetectionResult, ProcessingResult, WorkflowDiscoveryResult,
    WorkflowResult,
};
pub use workflow::WorkflowCreator;
