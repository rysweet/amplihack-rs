//! Parallel workstream orchestrator with subprocess isolation.
//!
//! Executes multiple independent development tasks in parallel. Each workstream
//! runs in a clean /tmp clone with its own execution context.
//!
//! Port of `amplifier-bundle/tools/amplihack/skills/multitask/orchestrator.py`.

pub mod models;
pub mod orchestrator;

use anyhow::Result;

/// Run multitask orchestration from a workstreams JSON config.
pub fn run_multitask(
    config: &str,
    mode: &str,
    recipe: &str,
    max_runtime: u64,
    timeout_policy: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    orchestrator::run(config, mode, recipe, max_runtime, timeout_policy, dry_run)
}

/// Clean up completed workstream directories.
pub fn run_cleanup(config: &str, dry_run: bool) -> Result<()> {
    orchestrator::cleanup(config, dry_run)
}

/// Show status of all workstreams.
pub fn run_status(base_dir: Option<&str>) -> Result<()> {
    orchestrator::status(base_dir)
}
