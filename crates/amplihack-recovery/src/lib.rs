//! amplihack-recovery: 4-stage recovery pipeline for test/build failures.
//!
//! - Stage 1: Protect staged files, detect .claude changes
//! - Stage 2: Collect pytest error signatures, attempt fixes, compute delta verdict
//! - Stage 3: Quality audit cycles (3–6) with validators
//! - Stage 4: Code atlas execution with retry/backoff

pub mod coordinator;
pub mod models;
pub mod results;
pub mod stage1;
pub mod stage2;
pub mod stage3;
pub mod stage4;

pub use coordinator::run_recovery;
pub use models::*;
pub use results::{recovery_run_to_json, write_recovery_ledger};
pub use stage1::run_stage1;
pub use stage2::run_stage2;
pub use stage3::run_stage3;
pub use stage4::run_stage4;
