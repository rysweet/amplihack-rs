//! Version control system abstractions and implementations.
//!
//! Provides the [`VersionController`] trait and a concrete [`GitHubController`].

pub mod controller;
pub mod github;
pub mod types;

pub use controller::{VersionController, extract_change_ranges, parse_patch_header};
pub use github::GitHubController;
pub use types::*;
