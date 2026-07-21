//! Agent Bundle Generator.
//!
//! Ported from `amplihack/bundle_generator/`.
//!
//! Provides types, error handling, and the core API for generating, testing,
//! and packaging AI agent bundles from natural language descriptions.
//!
//! ## Architecture
//!
//! The pipeline stages mirror the Python implementation:
//!
//! 1. **Parsing** — analyse natural language prompts ([`PromptParser`])
//! 2. **Extraction** — extract intent and requirements ([`IntentExtractor`])
//! 3. **Generation** — create agent content ([`AgentGenerator`])
//! 4. **Building** — assemble bundles ([`BundleBuilder`])
//! 5. **Packaging** — produce distributable packages ([`FilesystemPackager`])
//! 6. **Distribution** — publish to GitHub ([`GitHubDistributor`])
//!
//! ## Module layout
//!
//! The implementation is decomposed into focused submodules; every public
//! item is re-exported here so callers continue to use the flat
//! `bundle_generator::*` paths:
//!
//! - [`error`] — [`BundleGeneratorError`] and recovery suggestions
//! - [`models`] — serde data models (`ParsedPrompt` … `GenerationMetrics`)
//! - [`traits`] — the pipeline stage traits
//! - [`packager`] — [`FilesystemPackager`] and path-safety guards
//! - [`distributor`] — [`GitHubDistributor`] and the `gh` CLI integration

mod distributor;
mod error;
mod models;
mod packager;
mod traits;

pub use error::BundleGeneratorError;

pub use models::{
    AgentBundle, AgentRequirement, AgentType, BundleAction, BundleStatus, Complexity,
    DistributionPlatform, DistributionResult, ExtractedIntent, GeneratedAgent, GenerationMetrics,
    PackageFormat, PackagedBundle, ParsedPrompt, TestResult, TestType,
};

pub use traits::{AgentGenerator, BundleBuilder, IntentExtractor, PromptParser};

pub use packager::FilesystemPackager;

pub use distributor::GitHubDistributor;
