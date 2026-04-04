//! Workflow classification and execution for amplihack.
//!
//! Routes user requests to the appropriate workflow and manages
//! execution via a 3-tier fallback cascade.
//!
//! # Modules
//! - [`classifier`] — Keyword-based request classification
//! - [`cascade`] — 3-tier execution cascade (Recipe → Skills → Markdown)
//! - [`session`] — Session start detection for triggering classification
//! - [`orchestrator`] — Integrates classifier, cascade, and session detection

pub mod cascade;
pub mod classifier;
pub mod gh_aw_compiler;
pub mod orchestrator;
pub mod provenance;
pub mod session;

pub use cascade::ExecutionTierCascade;
pub use classifier::{WorkflowClassifier, WorkflowType};
pub use gh_aw_compiler::{Diagnostic, GhAwCompiler, Severity, compile_workflow};
pub use orchestrator::SessionStartClassifierSkill;
pub use provenance::{ProvenanceEntry, log_classification, log_routing_decision};
pub use session::SessionStartDetector;
