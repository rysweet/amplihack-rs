//! CLI entry point orchestrating the full remote execution pipeline.
//!
//! Ties together packaging, provisioning, transfer, execution,
//! retrieval, and integration into a single workflow.

use std::path::Path;

use tracing::info;

use crate::error::RemoteError;
use crate::executor::Executor;
use crate::integrator::{IntegrationSummary, Integrator};
use crate::orchestrator::{Orchestrator, VMOptions};
use crate::packager::ContextPackager;

/// Result of the full remote workflow.
pub struct WorkflowResult {
    pub exit_code: i32,
    pub summary: Option<IntegrationSummary>,
    pub vm_name: Option<String>,
}

/// Borrowed inputs for executing the full remote workflow with explicit credentials.
pub struct WorkflowOptions<'a> {
    pub repo_path: &'a Path,
    pub command: &'a str,
    pub prompt: &'a str,
    pub max_turns: u32,
    pub vm_options: &'a VMOptions,
    pub timeout_minutes: u64,
    pub skip_secret_scan: bool,
    pub api_key: &'a str,
}

/// Execute the complete remote workflow.
///
/// Steps:
/// 1. Validate environment
/// 2. Package context (with secret scan)
/// 3. Provision or reuse VM
/// 4. Transfer context
/// 5. Execute remote command
/// 6. Retrieve results
/// 7. Integrate & cleanup
pub async fn execute_remote_workflow(
    repo_path: &Path,
    command: &str,
    prompt: &str,
    max_turns: u32,
    vm_options: &VMOptions,
    timeout_minutes: u64,
    skip_secret_scan: bool,
) -> Result<WorkflowResult, RemoteError> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| RemoteError::validation("ANTHROPIC_API_KEY not found in environment"))?;
    execute_remote_workflow_with_api_key(WorkflowOptions {
        repo_path,
        command,
        prompt,
        max_turns,
        vm_options,
        timeout_minutes,
        skip_secret_scan,
        api_key: &api_key,
    })
    .await
}

/// Execute the complete remote workflow with an explicit Anthropic API key.
pub async fn execute_remote_workflow_with_api_key(
    options: WorkflowOptions<'_>,
) -> Result<WorkflowResult, RemoteError> {
    info!("starting remote execution workflow");

    // Step 1: Validate
    if options.api_key.trim().is_empty() {
        return Err(RemoteError::validation(
            "ANTHROPIC_API_KEY not found in environment",
        ));
    }
    if !options.repo_path.join(".git").exists() {
        return Err(RemoteError::packaging("Not a git repository"));
    }

    // Step 2: Package context
    info!("packaging context");
    let mut packager = ContextPackager::new(options.repo_path, 500, options.skip_secret_scan);

    let archive_path = packager.package().await?;
    info!(
        path = %archive_path.display(),
        "context packaged"
    );

    // Step 3: Provision VM
    info!("provisioning VM");
    let orchestrator = Orchestrator::new(None).await?;
    let vm = orchestrator.provision_or_reuse(options.vm_options).await?;
    info!(vm = %vm.name, "VM ready");

    // Step 4: Transfer context
    info!("transferring context");
    let executor = Executor::new(
        vm.clone(),
        options.timeout_minutes,
        options.vm_options.tunnel_port,
    );
    executor.transfer_context(&archive_path).await?;
    info!("context transferred");

    // Cleanup temp files from packager
    packager.cleanup();

    // Step 5: Execute remote command
    info!(
        command = options.command,
        max_turns = options.max_turns,
        "executing remote command"
    );
    let result = executor
        .execute_remote_with_api_key(
            options.command,
            options.prompt,
            options.max_turns,
            options.api_key,
        )
        .await?;

    if result.timed_out {
        info!(duration = result.duration_seconds, "execution timed out");
    } else {
        info!(
            exit_code = result.exit_code,
            duration = result.duration_seconds,
            "execution complete"
        );
    }

    // Step 6: Retrieve results
    info!("retrieving results");
    let results_dir = tempfile::tempdir()
        .map_err(|e| RemoteError::transfer(format!("Failed to create temp dir: {e}")))?
        .keep();

    let _ = executor.retrieve_logs(&results_dir).await;
    let _ = executor.retrieve_git_state(&results_dir).await;

    // Step 7: Integrate
    let summary = if results_dir.join("results.bundle").exists() {
        info!("integrating results");
        let integrator = Integrator::new(options.repo_path)?;
        let summary = integrator.integrate(&results_dir).await?;

        let report = integrator.create_summary_report(&summary);
        info!("{report}");

        Some(summary)
    } else {
        None
    };

    // Cleanup VM unless keep_vm is set
    if !options.vm_options.keep_vm && (result.exit_code == 0 || result.timed_out) {
        info!(vm = %vm.name, "cleaning up VM");
        let _ = orchestrator.cleanup(&vm, true).await;
    } else {
        info!(
            vm = %vm.name,
            "VM preserved"
        );
    }

    // Cleanup results dir
    let _ = std::fs::remove_dir_all(&results_dir);

    Ok(WorkflowResult {
        exit_code: result.exit_code,
        summary,
        vm_name: Some(vm.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_result_has_fields() {
        let r = WorkflowResult {
            exit_code: 0,
            summary: None,
            vm_name: Some("test-vm".into()),
        };
        assert_eq!(r.exit_code, 0);
        assert!(r.summary.is_none());
        assert_eq!(r.vm_name.as_deref(), Some("test-vm"));
    }

    #[test]
    fn vm_options_for_workflow() {
        let opts = VMOptions {
            size: "Standard_D2s_v3".into(),
            keep_vm: true,
            ..VMOptions::default()
        };
        assert!(opts.keep_vm);
    }

    #[test]
    fn workflow_requires_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(execute_remote_workflow(
            dir.path(),
            "auto",
            "test",
            10,
            &VMOptions::default(),
            60,
            true,
        ));
        assert!(result.is_err());
    }
}
