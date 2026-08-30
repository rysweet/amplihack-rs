---
title: Operate an External LiteLLM Route
description: Configure and operate amplihack routing through an operator-managed LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: howto
---

# Operate an External LiteLLM Route

Use this guide to configure, rotate, disable, and diagnose an external LiteLLM
route. For a first configuration, follow the
[external LiteLLM gateway tutorial](../tutorials/external-litellm-gateway.md).
Amplihack only configures supported clients to use the route. Operate the
LiteLLM service, provider credentials, policy, and telemetry separately.

## Configure a shared HTTPS gateway

Protect the amplihack directory and credential:

```bash
install -d -m 700 "$HOME/.amplihack"
install -m 600 /dev/null "$HOME/.amplihack/litellm.key"
printf '%s\n' 'replace-with-a-user-scoped-virtual-key' \
  > "$HOME/.amplihack/litellm.key"
```

Create the non-secret configuration:

```toml
schema_version = 1
endpoint = "https://llm-gateway.internal.example"

[copilot]
model = "gateway-coding"
```

Save it as `~/.amplihack/litellm-config.toml`, set its mode to `600`, and
select the key file:

```bash
chmod 600 "$HOME/.amplihack/litellm-config.toml"
export AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key"
amplihack copilot --litellm
```

The gateway certificate must validate for
`llm-gateway.internal.example`. Amplihack resolves the DNS
name once, validates the complete answer set, and pins the readiness connection
to one validated address. It still preserves
`llm-gateway.internal.example` as the HTTP host and TLS SNI name.

## Configure one launch without TOML

Set the endpoint, one credential source, and the Copilot model:

```bash
AMPLIHACK_LITELLM_ENDPOINT='https://llm-gateway.internal.example' \
AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key" \
AMPLIHACK_LITELLM_COPILOT_MODEL='gateway-coding' \
amplihack copilot --litellm
```

For Claude, omit `AMPLIHACK_LITELLM_COPILOT_MODEL`:

```bash
AMPLIHACK_LITELLM_ENDPOINT='https://llm-gateway.internal.example' \
AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key" \
amplihack claude --litellm
```

The gateway must expose an Anthropic-compatible API for Claude Code and
RustyClawd. It must expose an OpenAI-compatible API for GitHub Copilot CLI.

## Route RustyClawd

RustyClawd uses the Anthropic-compatible adapter:

```bash
amplihack RustyClawd --litellm
```

It receives the same gateway variables as Claude. Amplihack still validates
the resolved RustyClawd executable immediately before spawn.

## Rotate a virtual key

Write the replacement to a new protected file and rename it atomically:

```bash
install -m 600 /dev/null "$HOME/.amplihack/litellm.key.new"
read -rsp 'New LiteLLM virtual key: ' LITELLM_KEY
printf '%s\n' "$LITELLM_KEY" > "$HOME/.amplihack/litellm.key.new"
unset LITELLM_KEY
mv "$HOME/.amplihack/litellm.key.new" "$HOME/.amplihack/litellm.key"
```

New launches read the replacement. Existing agent processes retain the
credential with which they started. Revoke the old virtual key at the gateway
after those sessions end.

## Override a configured endpoint

An environment endpoint overrides the TOML endpoint:

```bash
AMPLIHACK_LITELLM_ENDPOINT='https://backup-gateway.internal.example' \
  amplihack claude --litellm
```

Use one complete endpoint value. Amplihack does not combine URL components
from different sources.

## Disable routing

Disable routing for one command:

```bash
amplihack copilot --no-litellm
```

To disable implicit routing, remove the gateway configuration variables and
rename or remove `~/.amplihack/litellm-config.toml`. Do not leave a credential
variable set without an endpoint; partial configuration is an error.

## Diagnose a failed launch

Read the stable code at the beginning of the diagnostic:

```text
AH_LITELLM_READINESS: external LiteLLM gateway is not ready
```

Diagnostics deliberately omit endpoint text, credentials, headers, response
bodies, command objects, and nested transport errors. Use the code to choose
the next check:

| Code | Operator check |
|---|---|
| `AH_LITELLM_CONFIG` | Remove unknown, partial, conflicting, or obsolete settings. |
| `AH_LITELLM_CREDENTIAL` | Check that exactly one credential source is set and file permissions are private. |
| `AH_LITELLM_ENDPOINT` | Use HTTPS, or literal-loopback HTTP for local development. |
| `AH_LITELLM_DESTINATION` | Check DNS for mixed, metadata, link-local, multicast, unspecified, or broadcast addresses. |
| `AH_LITELLM_READINESS` | Check service availability and the readiness JSON contract. |
| `AH_LITELLM_CAPABILITY` | Upgrade the target CLI. Claude-compatible targets must advertise `--setting-sources`; Copilot must advertise the documented custom-provider variables and offline mode. |
| `AH_LITELLM_ARGUMENT` | Remove checkout, resume, continue, conflicting model, remote, cloud, sharing, export, connect, or passthrough arguments. |
| `AH_LITELLM_UNSUPPORTED` | Use a local Claude, Copilot, or RustyClawd launch without Docker, auto, or append mode. |

Do not work around readiness by adding a direct provider key. Routed launches
never fall back to a provider.

## Migrate from the removed embedded integration

Delete settings for the former embedded proxy, management client, local
pricing, spend caps, rate limits, databases, and telemetry collectors. They are
not accepted by schema version 1.

Operate those concerns in LiteLLM itself:

| Former amplihack concern | Current owner |
|---|---|
| Gateway process and deployment | LiteLLM operator |
| Provider credentials | LiteLLM secret store |
| Model aliases | LiteLLM configuration |
| Usage database and migrations | LiteLLM operator |
| Budgets, pricing, and rate limits | LiteLLM policy |
| Logs, metrics, traces, and dashboards | Gateway observability stack |

Keep only the external endpoint, the optional Copilot model, and one virtual
key source in amplihack. There is no embedded compatibility mode.
