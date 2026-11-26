# How to Use Awesome-Copilot Integration

Amplihack integrates with [awesome-copilot](https://github.com/github/awesome-copilot) to provide community-curated agents, plugins, and an MCP server for enhanced development workflows.

## Quick Start

### For Copilot CLI Users

When you run `amplihack copilot`, the integration automatically:

1. **Enables the awesome-copilot MCP server** (if Docker is running)
2. **Registers the awesome-copilot marketplace** for plugin discovery

No manual configuration needed.

### Using Awesome-Copilot Plugins

After the marketplace is registered, install any of the 48+ community plugins:

```bash
copilot plugin install security-best-practices@awesome-copilot
copilot plugin install testing-automation@awesome-copilot
copilot plugin install azure-cloud-development@awesome-copilot
```

Browse all available plugins interactively:

```
/plugin
```

### Using the MCP Server

The awesome-copilot MCP server provides tools for discovering community prompts, agents, and skills. It requires Docker:

```bash
# Verify Docker is running
docker info

# Launch amplihack copilot (MCP server auto-enabled)
amplihack copilot
```

If Docker is not available, the MCP server is silently skipped.

## New Agents

Three agents adapted from awesome-copilot are available on all platforms:

### OpenAPI Scaffolder

Scaffolds a complete application from an OpenAPI specification:

```
Use the openapi-scaffolder agent to generate a FastAPI app from my openapi.yaml
```

Supported languages: Python/FastAPI, TypeScript/NestJS, Go, C#/.NET, Java/Spring Boot.

### IaC Planner

Plans Infrastructure-as-Code from requirements:

```
Use the iac-planner agent to create a Terraform plan for a 3-tier web app on Azure
```

Supported tools: Terraform, Bicep, CloudFormation.

### MCP Server Builder

Builds Model Context Protocol servers:

```
Use the mcp-server-builder agent to create a Python MCP server for our internal API
```

Supported languages: Python, TypeScript, C#, Go, Java, Rust.

## Drift Detection

Monitor awesome-copilot for new content with `gh` and `jq`:

```bash
state_file="${AMPLIHACK_HOME:-$HOME/.amplihack}/awesome-copilot-sync-state.json"
latest="$(gh api repos/github/awesome-copilot/commits --jq '.[0]')"
latest_sha="$(printf '%s\n' "$latest" | jq -r '.sha')"
latest_date="$(printf '%s\n' "$latest" | jq -r '.commit.author.date')"
last_sha="$(jq -r '.latest_commit_sha // empty' "$state_file" 2>/dev/null || true)"

if [ "$latest_sha" = "$last_sha" ]; then
  echo "awesome-copilot sync status: CURRENT"
else
  echo "awesome-copilot sync status: DRIFT_DETECTED"
fi
echo "Latest upstream commit: $latest_date $latest_sha"
```

Output states:

- `CURRENT`: No new content since last check
- `DRIFT_DETECTED`: New commits found upstream

The check compares the latest upstream commit with the `latest_commit_sha`
field of a state file that the maintainer keeps. The check does not write that
file.

State is stored at `~/.amplihack/awesome-copilot-sync-state.json`.

## For Claude Code Users

The new agents are available via the Task tool:

```
Task(subagent_type="general-purpose", prompt="Use the openapi-scaffolder agent...")
```

The awesome-copilot MCP server can be added manually to Claude Code's MCP config:

```json
{
  "awesome-copilot": {
    "type": "stdio",
    "command": "docker",
    "args": ["run", "-i", "--rm", "ghcr.io/microsoft/mcp-dotnet-samples/awesome-copilot:latest"]
  }
}
```

## Troubleshooting

**MCP server not appearing**: Verify Docker is running with `docker info`. The server is only enabled when Docker is available.

**Marketplace registration failed**: This is best-effort. You can register manually: `copilot plugin marketplace add github/awesome-copilot`

**Drift detection error**: Ensure `gh` CLI is installed and authenticated: `gh auth status`
