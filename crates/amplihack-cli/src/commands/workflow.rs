use amplihack_workflows::simulation::{RecipeSimulation, RecipeSimulationScenario};
use amplihack_workflows::stale_cleanup::{
    CleanupAction, CleanupMode, CleanupPlan, CleanupPolicy, StaleChangeRequest,
};
use amplihack_workflows::workflow_contract::{
    HelperEnvelope, HelperOperation, ManualAction, ProviderContext, ProviderOperationStatus,
    RepositoryIdentity, RepositoryProvider, provider_capabilities, provider_default_next_action,
    provider_from_remote_url, validate_terminal_transition,
};
use anyhow::{Context, Result};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use serde_json::json;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderDetection {
    provider: RepositoryProvider,
    repository: RepositoryIdentity,
    warnings: Vec<String>,
}

fn detect_provider_from_repo(repo: &Path) -> Result<ProviderDetection> {
    let mut warnings = Vec::new();
    let config = read_git_config(repo)?;
    let remote_url = config.as_deref().and_then(origin_remote_url);
    let provider = provider_from_remote_url(remote_url.as_deref());
    if config.is_none() {
        warnings.push("No Git config found; provider set to Manual.".into());
    } else if remote_url.is_none() {
        warnings.push("No origin remote URL found; provider set to Manual.".into());
    } else if provider == RepositoryProvider::Manual {
        warnings.push("Remote provider is unknown; provider set to Manual.".into());
    }

    let fallback_name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "repository".into());
    let (owner, name) = repository_owner_name(remote_url.as_deref(), &fallback_name);
    Ok(ProviderDetection {
        provider,
        repository: RepositoryIdentity {
            remote_url,
            owner,
            name,
            default_base: "main".into(),
        },
        warnings,
    })
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
        RepositoryProvider::GitHub => ProviderOperationStatus::Succeeded,
        RepositoryProvider::AzureDevOps | RepositoryProvider::Manual => {
            ProviderOperationStatus::ManualRequired
        }
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

fn read_git_config(repo: &Path) -> Result<Option<String>> {
    let Some(path) = git_config_path(repo)? else {
        return Ok(None);
    };
    std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read Git config from {}", path.display()))
        .map(Some)
}

fn git_config_path(repo: &Path) -> Result<Option<PathBuf>> {
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        let config = dot_git.join("config");
        return Ok(config.is_file().then_some(config));
    }
    if !dot_git.is_file() {
        return Ok(None);
    }

    let marker = std::fs::read_to_string(&dot_git)
        .with_context(|| format!("failed to read Git marker file {}", dot_git.display()))?;
    let git_dir = marker
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("invalid Git marker file {}", dot_git.display()))?;
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        repo.join(git_dir)
    };

    let worktree_config = git_dir.join("config");
    if worktree_config.is_file() {
        return Ok(Some(worktree_config));
    }

    let common_dir_file = git_dir.join("commondir");
    if common_dir_file.is_file() {
        let common_dir = std::fs::read_to_string(&common_dir_file).with_context(|| {
            format!(
                "failed to read Git common-dir marker {}",
                common_dir_file.display()
            )
        })?;
        let common_dir = PathBuf::from(common_dir.trim());
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            git_dir.join(common_dir)
        };
        let common_config = common_dir.join("config");
        if common_config.is_file() {
            return Ok(Some(common_config));
        }
    }

    Ok(None)
}

fn origin_remote_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    let mut first_remote_url = None;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_origin = trimmed == r#"[remote "origin"]"#;
            continue;
        }
        let Some(url) = trimmed.strip_prefix("url").and_then(|rest| {
            rest.trim_start()
                .strip_prefix('=')
                .map(str::trim)
                .filter(|value| !value.is_empty())
        }) else {
            continue;
        };
        if first_remote_url.is_none() {
            first_remote_url = Some(url.to_string());
        }
        if in_origin {
            return Some(url.to_string());
        }
    }
    first_remote_url
}

fn repository_owner_name(remote_url: Option<&str>, fallback_name: &str) -> (String, String) {
    let Some(remote_url) = remote_url else {
        return ("unknown".into(), fallback_name.into());
    };
    if let Some(path) = remote_url
        .split("github.com")
        .nth(1)
        .map(|value| value.trim_start_matches([':', '/']))
    {
        return owner_name_from_path(path, fallback_name);
    }
    if let Some(after_git) = remote_url.split("/_git/").nth(1) {
        let name = trim_repo_suffix(after_git);
        let owner = remote_url
            .split("/_git/")
            .next()
            .and_then(|prefix| prefix.split("dev.azure.com/").nth(1))
            .map(|path| path.trim_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "azure-devops".into());
        return (owner, name);
    }
    ("unknown".into(), fallback_name.into())
}

fn owner_name_from_path(path: &str, fallback_name: &str) -> (String, String) {
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next().filter(|value| !value.is_empty());
    let name = parts.next().filter(|value| !value.is_empty());
    match (owner, name) {
        (Some(owner), Some(name)) => (owner.into(), trim_repo_suffix(name)),
        _ => ("unknown".into(), fallback_name.into()),
    }
}

fn trim_repo_suffix(name: &str) -> String {
    name.trim_matches('/')
        .trim_end_matches(".git")
        .trim()
        .to_string()
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
    fn detect_provider_without_git_config_returns_manual_with_warning() {
        let temp = tempfile::tempdir().expect("tempdir");
        let detection = detect_provider_from_repo(temp.path()).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::Manual);
        assert_eq!(
            detection.repository.remote_url, None,
            "unknown repositories must not synthesize a GitHub remote"
        );
        assert!(
            detection
                .warnings
                .iter()
                .any(|warning| warning.contains("provider set to Manual"))
        );
    }

    #[test]
    fn detect_provider_reads_common_config_for_git_worktree_marker() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let git_dir = temp.path().join("main.git").join("worktrees").join("repo");
        let common_dir = temp.path().join("main.git");
        std::fs::create_dir_all(&repo).expect("repo dir");
        std::fs::create_dir_all(&git_dir).expect("git dir");
        std::fs::write(
            repo.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("git marker");
        std::fs::write(git_dir.join("commondir"), "../..\n").expect("commondir");
        std::fs::write(
            common_dir.join("config"),
            r#"
[remote "origin"]
    url = https://dev.azure.com/acme/project/_git/service
"#,
        )
        .expect("config");

        let detection = detect_provider_from_repo(&repo).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::AzureDevOps);
        assert_eq!(detection.repository.owner, "acme/project");
        assert_eq!(detection.repository.name, "service");
        assert!(detection.warnings.is_empty());
    }

    #[test]
    fn unknown_remote_returns_manual_not_github() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let git = repo.join(".git");
        std::fs::create_dir_all(&git).expect("git dir");
        std::fs::write(
            git.join("config"),
            r#"
[remote "origin"]
    url = ssh://git.example.invalid/acme/service
"#,
        )
        .expect("config");

        let detection = detect_provider_from_repo(&repo).expect("detection should succeed");

        assert_eq!(detection.provider, RepositoryProvider::Manual);
        assert!(
            detection
                .warnings
                .iter()
                .any(|warning| warning.contains("unknown"))
        );
    }

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
}
