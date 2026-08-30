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

For Claude Code, install version `2.1.83` or newer and confirm that the
executable selected by `PATH` reports its version:

```bash
command -v claude
claude --version
```

Amplihack probes that same executable before it performs launch setup. Do not
wrap `claude --version` with output that hides or replaces the semantic
version. A probe failure or unrecognized version output fails closed.

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

## Troubleshoot a failed launch

| Symptom | Action |
| --- | --- |
| Configuration is incomplete | Set all three required variables in the same environment, or unset all three. |
| Endpoint is rejected | Use HTTPS without credentials, query, or fragment. The path may be empty or `/v1`; HTTP requires a literal loopback address. |
| Model is rejected | Use a 1-128 character alias containing only letters, digits, `.`, `_`, `:`, `/`, or `-`. |
| Claude Code version cannot be verified | Run `command -v claude` and `claude --version` in the same environment. Install an official Claude Code executable whose output contains a valid semantic version. |
| Claude Code is older than `2.1.83` | Upgrade Claude Code. Prerelease versions compare using semantic-version precedence and must be at least `2.1.83`. |
| Launch option is rejected | Remove remote, export, share, resume, provider, or conflicting model controls. |
| Launcher is rejected | Use Claude Code, Copilot CLI, or rustyclawd, or unset all gateway variables. |
| Gateway cannot be reached | Check the child CLI error, gateway health, DNS, TLS trust, and firewall. Amplihack does not probe or retry the gateway. |

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
