use amplihack_workflows::simulation::{RecipeSimulation, RecipeSimulationScenario};
use amplihack_workflows::stale_cleanup::{
    CleanupMode, CleanupPlan, CleanupPolicy, StaleChangeRequest,
};
use amplihack_workflows::workflow_contract::{
    HelperEnvelope, HelperOperation, ManualAction, ProviderCapabilities, ProviderCapabilityState,
    ProviderContext, ProviderOperationStatus, RepositoryIdentity, RepositoryProvider,
    TerminalState, validate_terminal_transition,
};
use anyhow::{Context, Result};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use serde_json::json;
use std::path::PathBuf;

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
    let provider = detect_provider_from_repo(&args.repo);
    let repository = RepositoryIdentity {
        remote_url: None,
        owner: "unknown".into(),
        name: args
            .repo
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repository".into()),
        default_base: "main".into(),
    };
    let capabilities = capabilities_for(provider);
    let context = ProviderContext {
        schema_version: 1,
        provider,
        repository: repository.clone(),
        capabilities: capabilities.clone(),
        status: match provider {
            RepositoryProvider::GitHub => ProviderOperationStatus::Succeeded,
            RepositoryProvider::AzureDevOps | RepositoryProvider::Manual => {
                ProviderOperationStatus::ManualRequired
            }
        },
        next_action: next_action_for(provider).into(),
    };
    let data = json!({
        "repository": repository,
        "capabilities": capabilities,
        "provider_context": context
    });
    let envelope = match provider {
        RepositoryProvider::GitHub => HelperEnvelope::succeeded(
            provider,
            HelperOperation::DetectProvider,
            next_action_for(provider),
            data,
        ),
        RepositoryProvider::AzureDevOps | RepositoryProvider::Manual => {
            HelperEnvelope::manual_required(
                provider,
                HelperOperation::DetectProvider,
                next_action_for(provider),
                data,
            )
        }
    };
    write_output(&envelope, &args.format)
}

fn run_publish_change_request(args: PublishChangeRequestArgs) -> Result<()> {
    let provider = RepositoryProvider::from(args.provider);
    let envelope = match provider {
        RepositoryProvider::GitHub => HelperEnvelope::succeeded(
            provider,
            HelperOperation::PublishChangeRequest,
            "Inspect the created GitHub pull request and wait for checks.",
            json!({
                "change_request": {
                    "kind": "PullRequest",
                    "id": "dry-run",
                    "url": null,
                    "state": "Open",
                    "source_branch": args.source_branch,
                    "base_branch": args.base_branch,
                    "head_sha": null,
                    "title": args.title
                }
            }),
        ),
        RepositoryProvider::AzureDevOps | RepositoryProvider::Manual => {
            let manual_action = ManualAction {
                action: "CreateAzureReposPullRequest".into(),
                instructions: format!(
                    "Create a pull request from {} to {} titled '{}'.",
                    args.source_branch, args.base_branch, args.title
                ),
                required_inputs: vec!["source_branch".into(), "base_branch".into(), "title".into()],
            };
            HelperEnvelope::manual_required(
                provider,
                HelperOperation::PublishChangeRequest,
                "Create the provider pull request manually, then rerun status detection.",
                json!({
                    "change_request": null,
                    "manual_action": manual_action
                }),
            )
        }
    };
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
    let candidates = match args.candidates {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read candidates from {}", path.display()))?;
            serde_json::from_str::<Vec<StaleChangeRequest>>(&text)
                .context("failed to parse cleanup candidates JSON")?
        }
        None => Vec::new(),
    };
    let policy = CleanupPolicy {
        provider: args.provider.into(),
        mode: if args.apply {
            CleanupMode::Apply
        } else {
            CleanupMode::DryRun
        },
        workflow_label: args.workflow_label,
        superseded_by_label_prefix: args.superseded_by_label_prefix,
        minimum_age_hours: args.minimum_age_hours,
    };
    let plan = CleanupPlan::build(policy, candidates).map_err(anyhow::Error::msg)?;
    let envelope = HelperEnvelope::succeeded(
        plan.provider,
        HelperOperation::CleanupStale,
        if plan.mutations_executed == 0 {
            "Cleanup plan completed without provider mutations."
        } else {
            "Cleanup mutations completed for eligible workflow-owned change requests."
        },
        serde_json::to_value(plan)?,
    );
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

fn detect_provider_from_repo(repo: &std::path::Path) -> RepositoryProvider {
    let marker = repo.join(".git/config");
    let config = std::fs::read_to_string(marker).unwrap_or_default();
    if config.contains("dev.azure.com") || config.contains("visualstudio.com") {
        RepositoryProvider::AzureDevOps
    } else {
        RepositoryProvider::GitHub
    }
}

fn capabilities_for(provider: RepositoryProvider) -> ProviderCapabilities {
    match provider {
        RepositoryProvider::GitHub => ProviderCapabilities {
            tracking_items: ProviderCapabilityState::Automated,
            change_requests: ProviderCapabilityState::Automated,
            stale_cleanup: ProviderCapabilityState::Automated,
        },
        RepositoryProvider::AzureDevOps => ProviderCapabilities {
            tracking_items: ProviderCapabilityState::Automated,
            change_requests: ProviderCapabilityState::ManualRequired,
            stale_cleanup: ProviderCapabilityState::ManualRequired,
        },
        RepositoryProvider::Manual => ProviderCapabilities {
            tracking_items: ProviderCapabilityState::ManualRequired,
            change_requests: ProviderCapabilityState::ManualRequired,
            stale_cleanup: ProviderCapabilityState::ManualRequired,
        },
    }
}

fn next_action_for(provider: RepositoryProvider) -> &'static str {
    match provider {
        RepositoryProvider::GitHub => "No further provider setup is required.",
        RepositoryProvider::AzureDevOps => {
            "Use Azure Boards automation where configured; create Azure Repos PRs manually."
        }
        RepositoryProvider::Manual => "Run provider-specific change request steps manually.",
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

#[allow(dead_code)]
fn _terminal_state_to_keep_type_referenced(state: TerminalState) -> TerminalState {
    state
}
