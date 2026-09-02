---
title: Operate an External LiteLLM Route
description: Configure, rotate, disable, and troubleshoot amplihack routing through an operator-managed LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: howto
---

# Operate an External LiteLLM Route

Use these procedures after completing the
[external LiteLLM gateway tutorial](../tutorials/external-litellm-gateway.md).

## Configure the route

All three variables are one configuration unit:

```bash
export AMPLIHACK_LITELLM_ENDPOINT='https://gateway.example.net'
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
export AMPLIHACK_LITELLM_MODEL='gateway-coding'
```

If any variable is present, all three must be non-empty and valid. The model
alias applies to every supported launcher. Keep the restricted virtual key in
a secret manager and materialize it only in the launch environment; amplihack
does not read a key file or TOML gateway configuration.

Claude Code receives `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`. Amplihack requires
the exact selected executable to report the currently verified version,
`2.1.247`. You can
confirm the executable selected by `PATH`:

```bash
command -v claude
claude --version
```

Amplihack probes the same executable before launch setup. Do not wrap
`claude --version` with output that hides or replaces the semantic
version. A missing executable, failed probe, unrecognized or malformed output,
prerelease, or any release other than `2.1.247` will fail closed.
The probe will run without gateway or direct-provider credentials.
On Linux, install `bubblewrap` and `socat`; Claude's scrub mode refuses to start
without both sandbox dependencies.

Copilot CLI receives `--secret-env-vars=COPILOT_PROVIDER_API_KEY`, and
amplihack requires the exact selected executable to report the currently
verified version, `1.0.83-2`:

```bash
command -v copilot
copilot --version
```

The runtime-attested control removes the gateway key from shell and stdio MCP
subprocess environments and redacts it from tool output. Missing, failed,
malformed, ambiguous, or unverified version probes fail closed. Routed
launches disable Copilot auto-update, so later releases remain blocked until
the real-CLI isolation contract passes and the verified-version set is
updated.

RustyClawd uses its native Copilot-compatible transport. Amplihack supplies
`--provider copilot`, supplies the configured alias as `--model`, maps the
gateway root to `GITHUB_COPILOT_ENDPOINT=<gateway>/v1`, and replaces
`GITHUB_TOKEN` in that child with the LiteLLM virtual key. User-supplied
provider overrides are rejected so a routed launch cannot select the direct
Anthropic, Copilot, or Azure service.

## Rotate a virtual key

Update the value in the secret manager, then refresh the launch environment:

```bash
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
```

Revoke the previous virtual key in LiteLLM after confirming a new launch.
Amplihack does not create, rotate, or revoke gateway keys.

## Change the gateway

Override the configured root for one launch while preserving the key and model:

```bash
AMPLIHACK_LITELLM_ENDPOINT='https://backup-gateway.internal.example' \
  amplihack claude
```

Use HTTPS for remote endpoints. Cleartext HTTP is accepted only for a literal
address in `127.0.0.0/8` or literal `::1`; `http://localhost` is rejected.

## Disable routing

Disable routing by unsetting the complete configuration:

```bash
unset AMPLIHACK_LITELLM_ENDPOINT
unset AMPLIHACK_LITELLM_API_KEY
unset AMPLIHACK_LITELLM_MODEL
amplihack copilot
```

An empty or partial configuration is rejected rather than treated as disabled.

## Copilot configuration isolation

While routing is enabled, each Copilot launch uses a fresh temporary
`COPILOT_HOME`. Persisted user-scoped plugins, agents, hooks, MCP servers,
credentials, and preferences are therefore unavailable to the routed process,
and any state it writes is removed when the process exits. Copilot discovers
repository configuration independently of `COPILOT_HOME`, so routed launches
reject `--add-dir` and reject workspaces containing `.github/agents`; repository
custom agents may select a model. Remove or rename that directory, or unset all
three routing variables to launch with repository custom agents and the normal
Copilot home.

## Troubleshoot a failed launch

| Symptom | Action |
| --- | --- |
| Configuration is incomplete | Set all three required variables in the same environment, or unset all three. |
| Endpoint is rejected | Use HTTPS without credentials, query, or fragment. The path may be empty or `/v1`; HTTP requires a literal loopback address. |
| Model is rejected | Use a 1-128 character alias containing only letters, digits, `.`, `_`, `:`, `/`, or `-`. |
| Launch option is rejected | Remove remote, export, share, resume, provider, custom-agent, `--add-dir`, or conflicting model controls. |
| Copilot workspace is rejected | Remove or rename `.github/agents`, or disable routing before using repository custom agents. |
| Launcher is rejected | Use Claude Code, Copilot CLI, or rustyclawd, or unset all gateway variables. |
| Docker Claude or Copilot launch is rejected | Launch outside Docker, or enter a trusted container and launch there so amplihack can attest the exact executable before agent startup. |
| Gateway cannot be reached | Check the child CLI error, gateway health, DNS, TLS trust, and firewall. Amplihack does not probe or retry the gateway. |

| Symptom | Action |
| --- | --- |
| Claude Code version cannot be verified | Run `command -v claude` and `claude --version` in the same environment. Install an official Claude Code executable whose output contains a valid semantic version. |
| Claude Code version is not `2.1.247` | Install the verified release. Future releases remain blocked until the real-CLI Bash, hook, and stdio MCP isolation test is rerun and the attestation set is updated. |
| Claude reports a missing sandbox dependency | On Linux, install `bubblewrap` and `socat`, then retry. Do not disable `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB`. |
| Copilot CLI version cannot be verified | Run `command -v copilot` and `copilot --version` in the same environment. Install the official Copilot CLI executable. |
| Copilot CLI version is not `1.0.83-2` | Install the verified release. Later releases remain blocked until the real-CLI shell and stdio MCP isolation test is rerun and the attestation set is updated. |

## Operate gateway-owned concerns

Configure these in LiteLLM or its platform, not amplihack:

| Concern | Owner |
| --- | --- |
| Gateway process and deployment | LiteLLM operator |
| Provider credentials and aliases | LiteLLM configuration and secret store |
| Database and migrations | LiteLLM operator |
| Usage, pricing, budgets, and rate limits | LiteLLM policy |
| Logs, metrics, traces, and dashboards | LiteLLM operator |

Restrict every virtual key at the gateway by tenant, route, model alias, budget,
and rate. Client-side model selection is routing metadata, not an authorization
boundary. Rotate or revoke the virtual key after suspected exposure; amplihack
does not control gateway-side authorization or revocation.

See the
[external LiteLLM environment variables](../reference/environment-variables.md#external-litellm-gateway-variables)
for the complete amplihack configuration contract.
