# Azure DevOps CLI Automation Patterns

Use these patterns when automating Azure DevOps with the official Azure CLI `azure-devops` extension.

## Setup and Authentication

Install the extension and authenticate before running Azure DevOps commands:

```bash
az extension add --name azure-devops
az login
export AZURE_DEVOPS_ORG_URL="${AZURE_DEVOPS_ORG_URL:?set the Azure DevOps organization URL}"
export AZURE_DEVOPS_PROJECT="${AZURE_DEVOPS_PROJECT:?set the Azure DevOps project name}"
az devops configure --defaults \
  organization="$AZURE_DEVOPS_ORG_URL" \
  project="$AZURE_DEVOPS_PROJECT"
az devops project list --output table
```

For non-interactive automation, use the narrowest available credential:

```bash
az devops login --organization "$AZURE_DEVOPS_ORG_URL"
```

Paste a least-privilege PAT only at the prompt or provide it through a reviewed secret store in CI. Do not put PAT values, tenant IDs, organization-specific URLs, or project identifiers in scripts committed to the repository.

## Configuration and Output

Configure defaults once to avoid repeating organization and project flags:

```bash
az devops configure --defaults \
  organization="$AZURE_DEVOPS_ORG_URL" \
  project="$AZURE_DEVOPS_PROJECT"
az devops configure --list
```

Use predictable output formats for scripts:

```bash
az pipelines list --output table
az repos pr list --output json
az boards query --wiql "SELECT [System.Id] FROM workitems" --output tsv
```

Enable Azure DevOps Git aliases when useful:

```bash
az devops configure --defaults use-git-aliases=true
```

## Essential Command Groups

### Organization and Projects

```bash
az devops project list --output table
az devops project show --project "$AZURE_DEVOPS_PROJECT" --output json
az devops team list --project "$AZURE_DEVOPS_PROJECT" --output table
```

### Boards

```bash
az boards work-item create \
  --type "User Story" \
  --title "Implement feature" \
  --description "<p>Describe the user value and acceptance criteria.</p>"

az boards work-item show --id 123 --output json
az boards work-item update --id 123 --state "In Progress"
az boards iteration project list --output table
```

### Repos

```bash
az repos list --project "$AZURE_DEVOPS_PROJECT" --output table
az repos create --name "service-api" --project "$AZURE_DEVOPS_PROJECT"
az repos pr list --repository service-api --status active --output table
az repos pr show --id 456 --output json
```

### Pipelines

```bash
az pipelines list --project "$AZURE_DEVOPS_PROJECT" --output table
az pipelines run --name "API-Build" --branch main
az pipelines runs show --id "$AZURE_DEVOPS_RUN_ID" --output json
az pipelines runs list --pipeline-ids "$AZURE_DEVOPS_PIPELINE_ID" --top 10 --output table
```

### Artifacts

```bash
az artifacts feed list --project "$AZURE_DEVOPS_PROJECT" --output table
az artifacts feed create --name "internal-packages" --project "$AZURE_DEVOPS_PROJECT"
az artifacts universal list --feed internal-packages --project "$AZURE_DEVOPS_PROJECT" --output table
```

## Pipeline Automation

Create a pipeline from YAML, run it, and inspect recent runs:

```bash
az pipelines create \
  --name "API-Build" \
  --repository myrepo \
  --branch main \
  --yml-path ci/azure-pipelines.yml

az pipelines run --name "API-Build" --branch main
az pipelines runs list --top 10 \
  --query "[].{Name:pipeline.name, Status:status, Result:result, Started:startTime}" \
  --output table
```

Run a release gate only when no active pull requests remain:

```bash
PENDING=$(az repos pr list --status active --query "length([])")
if [ "$PENDING" -eq 0 ]; then
  az pipelines run --name "Release-Pipeline"
fi
```

## Pull Request Automation

List active pull requests, open details, and record a reviewer vote:

```bash
az repos pr list --repository myrepo --status active --output table
az repos pr show --id 456 --open
az repos pr reviewer update --id 456 --reviewer-id user@example.com --vote approve
```

Create a pull request from the current branch:

```bash
azdo-pr() {
  az repos pr create \
    --source-branch "$(git branch --show-current)" \
    --target-branch main \
    --title "$1" \
    --open
}
```

## Work Item Automation

Create multiple work items from a shell loop:

```bash
for title in "Feature A" "Feature B" "Feature C"; do
  az boards work-item create \
    --type "User Story" \
    --title "$title" \
    --assigned-to team@example.com
done
```

List current sprint items:

```bash
az boards query \
  --wiql "SELECT [System.Id], [System.Title], [System.State] FROM WorkItems WHERE [System.IterationPath] = @CurrentIteration" \
  --output table
```

## Artifact Versioning

Publish a timestamped universal package version:

```bash
VERSION="1.0.$(date +%Y%m%d%H%M%S)"
az artifacts universal publish \
  --feed myfeed \
  --name myapp \
  --version "$VERSION" \
  --path ./build
```

## Dashboard Data Exports

Export project, pipeline, and pull request data for dashboards:

```bash
az devops project show --project "$AZURE_DEVOPS_PROJECT" > project.json
az pipelines runs list --top 50 > recent-runs.json
az repos pr list --status all > all-prs.json
```

## Environment Variable Sync

Export pipeline variables and use them to create a variable group:

```bash
az pipelines variable list --pipeline-name "$AZURE_DEVOPS_PIPELINE" --output json > vars.json
# Convert reviewed JSON values into key=value pairs before creating the group.
az pipelines variable-group create --name "Production" --variables ENVIRONMENT=production REGION=westus
```

## Direct REST API Access

Use `az devops invoke` only when a first-class command group does not expose the needed operation:

```bash
az devops invoke \
  --area build \
  --resource builds \
  --route-parameters project="$AZURE_DEVOPS_PROJECT" \
  --api-version 6.0 \
  --http-method GET

az devops invoke \
  --area git \
  --resource repositories \
  --route-parameters project="$AZURE_DEVOPS_PROJECT" \
  --http-method POST \
  --in-file payload.json
```

## JMESPath Filtering

Filter and reshape JSON output with `--query`:

```bash
az pipelines runs list \
  --query "[?result=='failed'].{Pipeline:pipeline.name, Branch:sourceBranch, Time:finishedDate}" \
  --output table

az repos pr list \
  --query "[?targetRefName=='refs/heads/main' && status=='active'].{ID:pullRequestId, Title:title, Author:createdBy.displayName}" \
  --output table
```

## Shell Aliases

Use local shell aliases for frequently repeated read-only commands:

```bash
alias azdo-pipelines="az pipelines list --output table"
alias azdo-prs="az repos pr list --status active --output table"
alias azdo-builds="az pipelines runs list --top 20 --output table"
```

## Troubleshooting

Check extension installation and current defaults:

```bash
az extension show --name azure-devops --output table
az devops configure --list
az account show --output table
```

If a command returns an authorization error, verify the account, organization, project, and least-privilege scope before broadening permissions:

```bash
az devops project show --project "$AZURE_DEVOPS_PROJECT" --organization "$AZURE_DEVOPS_ORG_URL"
```

For WIQL quoting issues, keep the query in a variable or file so shell quoting does not corrupt brackets and string literals:

```bash
WIQL="SELECT [System.Id], [System.Title] FROM WorkItems WHERE [System.State] = 'Active'"
az boards query --wiql "$WIQL" --output table
```
