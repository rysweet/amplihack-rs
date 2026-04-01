//! XPIA (eXtendible Prompt Injection Armor) security module.
//!
//! Provides detection and prevention of Cross-Prompt Injection Attacks.
//!
//! # Modules
//! - [`config`] — Environment-driven configuration
//! - [`risk`] — Risk/severity types and validation results
//! - [`patterns`] — Attack pattern definitions
//! - [`defender`] — Core validation logic
//! - [`health`] — System health checks

pub mod config;
pub mod defender;
pub mod health;
pub mod patterns;
pub mod risk;

pub use config::XpiaConfig;
pub use defender::XpiaDefender;
pub use health::{HealthReport, HealthStatus};
pub use patterns::{AttackPattern, PatternCategory, XpiaPatterns};
pub use risk::{ContentType, RiskLevel, SecurityLevel, ThreatDetection, ValidationResult};
