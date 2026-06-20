use amplihack_workflows::simulation::{RecipeSimulation, RecipeSimulationScenario};
use amplihack_workflows::stale_cleanup::{
    CleanupAction, CleanupMode, CleanupPlan, CleanupPolicy, StaleChangeRequest,
};
use amplihack_workflows::workflow_contract::{
    HelperEnvelope, HelperOperation, ManualAction, ProviderCapabilityState, ProviderContext,
    ProviderOperationStatus, RepositoryProvider, provider_capabilities,
    provider_default_next_action, validate_terminal_transition,
};
use anyhow::{Context, Result};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use serde_json::json;
use std::path::PathBuf;

mod provider_detection;

use provider_detection::detect_provider_from_repo;

#[derive(Subcommand, Debug)]
pub enum WorkflowCommands {
    /// Detect provider context for the current repository.
    DetectProvider(DetectProviderArgs),
    /// Provider-neutral change request operations.
    ChangeRequest {
        #[command(subcommand)]
        command: ChangeRequestCommands,
    },
    /// Run a deterministic local recipe simulation fixture.
    SimulateRecipe(SimulateRecipeArgs),
    /// Plan or apply stale workflow-owned change-request cleanup.
    CleanupStale(CleanupStaleArgs),
    /// Validate or emit explicit workflow terminal state.
    TerminalState(TerminalStateArgs),
}

#[derive(Args, Debug)]
pub struct DetectProviderArgs {
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

#[derive(Subcommand, Debug)]
pub enum ChangeRequestCommands {
    /// Publish a provider-neutral change request or return a manual action.
    Publish(PublishChangeRequestArgs),
}

#[derive(Args, Debug)]
pub struct PublishChangeRequestArgs {
    #[arg(long, value_enum)]
    provider: ProviderArg,
    #[arg(long)]
    source_branch: String,
    #[arg(long)]
    base_branch: String,
    #[arg(long)]
    title: String,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

#[derive(Args, Debug)]
pub struct SimulateRecipeArgs {
    recipe: String,
    #[arg(long)]
    scenario: String,
    #[arg(long)]
    repo_fixture: Option<PathBuf>,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("mode")
        .required(true)
        .args(["dry_run", "apply"])
))]
pub struct CleanupStaleArgs {
    #[arg(long, value_enum)]
    provider: ProviderArg,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    apply: bool,
    #[arg(long, default_value = "amplihack-workflow")]
    workflow_label: String,
    #[arg(long, default_value = "superseded-by:")]
    superseded_by_label_prefix: String,
    #[arg(long, default_value_t = 48)]
    minimum_age_hours: u64,
    #[arg(long)]
    candidates: Option<PathBuf>,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

#[derive(Args, Debug)]
pub struct TerminalStateArgs {
    #[arg(long, default_value = "HOLLOW_SUCCESS")]
    terminal_state: String,
    #[arg(long, default_value_t = false)]
    terminal_success: bool,
    #[arg(
        long,
        default_value = "Inspect workflow evidence and rerun finalization."
    )]
    required_next_action: String,
    #[arg(long, default_value = "text", value_parser = ["text", "json"])]
    format: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProviderArg {
    Github,
    #[value(name = "azure-devops")]
    AzureDevops,
    Manual,
}

impl From<ProviderArg> for RepositoryProvider {
    fn from(value: ProviderArg) -> Self {
        match value {
            ProviderArg::Github => Self::GitHub,
            ProviderArg::AzureDevops => Self::AzureDevOps,
            ProviderArg::Manual => Self::Manual,
        }
    }
}

pub fn dispatch(command: WorkflowCommands) -> Result<()> {
    match command {
        WorkflowCommands::DetectProvider(args) => run_detect_provider(args),
        WorkflowCommands::ChangeRequest { command } => match command {
            ChangeRequestCommands::Publish(args) => run_publish_change_request(args),
        },
        WorkflowCommands::SimulateRecipe(args) => run_simulate_recipe(args),
        WorkflowCommands::CleanupStale(args) => run_cleanup_stale(args),
        WorkflowCommands::TerminalState(args) => run_terminal_state(args),
    }
}

fn run_detect_provider(args: DetectProviderArgs) -> Result<()> {
    let detection = detect_provider_from_repo(&args.repo)?;
    let provider = detection.provider;
    let repository = detection.repository;
    let capabilities = provider_capabilities(provider);
    let status = provider_status(provider);
    let context = ProviderContext {
        schema_version: 1,
        provider,
        repository: repository.clone(),
        capabilities: capabilities.clone(),
        status,
        next_action: provider_default_next_action(provider).into(),
    };
    let data = json!({
        "repository": repository,
        "capabilities": capabilities,
        "provider_context": context
    });
    let mut envelope = helper_envelope(
        provider,
        HelperOperation::DetectProvider,
        status,
        provider_default_next_action(provider),
        data,
    );
    envelope.warnings = detection.warnings;
    write_output(&envelope, &args.format)
}

fn run_publish_change_request(args: PublishChangeRequestArgs) -> Result<()> {
    let provider = RepositoryProvider::from(args.provider);
    let capabilities = provider_capabilities(provider);
    if capabilities.change_requests == ProviderCapabilityState::Automated {
        let envelope = helper_envelope(
            provider,
            HelperOperation::PublishChangeRequest,
            ProviderOperationStatus::Failed,
            "Use the workflow publish provider adapter; this typed CLI command does not create provider pull requests.",
            json!({
                "change_request": null,
                "manual_action": null,
                "capabilities": capabilities
            }),
        );
        return write_output(&envelope, &args.format);
    }

    let manual_action = manual_publish_action(
        provider,
        &args.source_branch,
        &args.base_branch,
        &args.title,
    );
    let envelope = helper_envelope(
        provider,
        HelperOperation::PublishChangeRequest,
        ProviderOperationStatus::ManualRequired,
        "Create the provider change request manually, then rerun status detection.",
        json!({
            "change_request": null,
            "manual_action": manual_action
        }),
    );
    write_output(&envelope, &args.format)
}

fn run_simulate_recipe(args: SimulateRecipeArgs) -> Result<()> {
    let scenario_text = std::fs::read_to_string(&args.scenario).unwrap_or_else(|_| {
        minimal_named_scenario(&args.scenario, &args.recipe, args.repo_fixture.as_ref())
    });
    let scenario: RecipeSimulationScenario = serde_yaml::from_str(&scenario_text)
        .context("failed to parse workflow simulation scenario")?;
    let result = RecipeSimulation::run(scenario).map_err(anyhow::Error::new)?;
    let envelope = HelperEnvelope::succeeded(
        result.provider,
        HelperOperation::SimulateRecipe,
        if result.terminal_success {
            "Simulation reached a successful terminal state."
        } else {
            "Inspect simulation terminal state and required provider action."
        },
        serde_json::to_value(result)?,
    );
    write_output(&envelope, &args.format)
}

fn run_cleanup_stale(args: CleanupStaleArgs) -> Result<()> {
    let requested_apply = args.apply;
    let candidates = match args.candidates {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read candidates from {}", path.display()))?;
            serde_json::from_str::<Vec<StaleChangeRequest>>(&text)
                .context("failed to parse cleanup candidates JSON")?
        }
        None => Vec::new(),
    };
    let provider = args.provider.into();
    let policy = CleanupPolicy {
        provider,
        mode: CleanupMode::DryRun,
        workflow_label: args.workflow_label,
        superseded_by_label_prefix: args.superseded_by_label_prefix,
        minimum_age_hours: args.minimum_age_hours,
    };
    let plan = CleanupPlan::build(policy, candidates).map_err(anyhow::Error::msg)?;
    let has_eligible_actions = plan
        .actions
        .iter()
        .any(|action| action.action == CleanupAction::WouldCloseAsSuperseded);
    let status = if requested_apply && has_eligible_actions {
        ProviderOperationStatus::ManualRequired
    } else {
        ProviderOperationStatus::Succeeded
    };
    let mut envelope = helper_envelope(
        plan.provider,
        HelperOperation::CleanupStale,
        status,
        if requested_apply && has_eligible_actions {
            "Review the dry-run plan and close eligible change requests through the provider manually."
        } else if plan.mutations_executed == 0 {
            "Cleanup plan completed without provider mutations."
        } else {
            "Cleanup mutations completed for eligible workflow-owned change requests."
        },
        serde_json::to_value(plan)?,
    );
    if requested_apply {
        envelope.warnings.push(
            "Provider mutation adapters are not wired in this helper; emitted a dry-run cleanup plan instead of mutating remote change requests."
                .into(),
        );
    }
    write_output(&envelope, &args.format)
}

fn run_terminal_state(args: TerminalStateArgs) -> Result<()> {
    let result = validate_terminal_transition(json!({
        "terminal_state": args.terminal_state,
        "terminal_success": args.terminal_success,
        "required_next_action": args.required_next_action,
        "evidence_used": []
    }));
    write_output(&result, &args.format)
}

fn helper_envelope(
    provider: RepositoryProvider,
    operation: HelperOperation,
    status: ProviderOperationStatus,
    next_action: impl Into<String>,
    data: serde_json::Value,
) -> HelperEnvelope {
    HelperEnvelope {
        schema_version: 1,
        provider,
        operation,
        status,
        next_action: next_action.into(),
        warnings: Vec::new(),
        data,
    }
}

fn provider_status(provider: RepositoryProvider) -> ProviderOperationStatus {
    match provider {
        RepositoryProvider::GitHub | RepositoryProvider::AzureDevOps => {
            ProviderOperationStatus::Succeeded
        }
        RepositoryProvider::Manual => ProviderOperationStatus::ManualRequired,
    }
}

fn manual_publish_action(
    provider: RepositoryProvider,
    source_branch: &str,
    base_branch: &str,
    title: &str,
) -> ManualAction {
    let required_inputs = vec!["source_branch".into(), "base_branch".into(), "title".into()];
    match provider {
        RepositoryProvider::GitHub => ManualAction {
            action: "CreateGitHubPullRequest".into(),
            instructions: format!(
                "Create a GitHub pull request from {source_branch} to {base_branch} titled '{title}'."
            ),
            required_inputs,
        },
        RepositoryProvider::AzureDevOps => ManualAction {
            action: "CreateAzureReposPullRequest".into(),
            instructions: format!(
                "Create an Azure Repos pull request from {source_branch} to {base_branch} titled '{title}'."
            ),
            required_inputs,
        },
        RepositoryProvider::Manual => ManualAction {
            action: "CreateProviderChangeRequest".into(),
            instructions: format!(
                "Create a provider change request from {source_branch} to {base_branch} titled '{title}'."
            ),
            required_inputs,
        },
    }
}

fn minimal_named_scenario(name: &str, recipe: &str, repo_fixture: Option<&PathBuf>) -> String {
    format!(
        r#"name: {name}
recipe: {recipe}
repo_fixture: {repo_fixture}
provider:
  kind: GitHub
  capabilities:
    tracking_items: Automated
    change_requests: Automated
    stale_cleanup: Automated
expect:
  terminal_state: FOLLOWUP_CREATED
  terminal_success: true
"#,
        repo_fixture = repo_fixture
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "null".into())
    )
}

fn write_output<T: serde::Serialize>(value: &T, _format: &str) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_publish_actions_are_provider_specific_without_fake_success() {
        let github = manual_publish_action(RepositoryProvider::GitHub, "feat/x", "main", "Ship x");
        assert_eq!(github.action, "CreateGitHubPullRequest");
        assert!(github.instructions.contains("GitHub pull request"));

        let azdo =
            manual_publish_action(RepositoryProvider::AzureDevOps, "feat/x", "main", "Ship x");
        assert_eq!(azdo.action, "CreateAzureReposPullRequest");
        assert!(azdo.instructions.contains("Azure Repos pull request"));

        let manual = manual_publish_action(RepositoryProvider::Manual, "feat/x", "main", "Ship x");
        assert_eq!(manual.action, "CreateProviderChangeRequest");
        assert!(
            !manual.instructions.contains("GitHub") && !manual.instructions.contains("Azure"),
            "generic manual provider instructions must remain provider-neutral"
        );
    }

    #[test]
    fn azdo_provider_detection_status_is_automated_not_manual_required() {
        assert_eq!(
            provider_status(RepositoryProvider::AzureDevOps),
            ProviderOperationStatus::Succeeded
        );
    }
}
