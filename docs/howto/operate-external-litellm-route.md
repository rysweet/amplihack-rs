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

## Select configuration precedence

Route inputs use this order:

1. `--litellm` or `--no-litellm`;
2. recognized `AMPLIHACK_LITELLM_*` environment variables;
3. `~/.amplihack/litellm-config.toml`; and
4. disabled when no signal exists.

Environment values replace individual TOML values. A present TOML file must
still have valid version 1 syntax and no unknown keys.

## Use a key file instead of shell history

Prefer `AMPLIHACK_LITELLM_API_KEY_FILE` for interactive shells:

```bash
export AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key"
amplihack claude --litellm
```

Set exactly one of `AMPLIHACK_LITELLM_API_KEY` and
`AMPLIHACK_LITELLM_API_KEY_FILE`. Credentials are not accepted in TOML or
command arguments.

## Rotate a virtual key

Write the replacement to a new protected file and rename it atomically:

```bash
install -m 600 /dev/null "$HOME/.amplihack/litellm.key.new"
read -rsp 'New LiteLLM virtual key: ' LITELLM_KEY
printf '%s\n' "$LITELLM_KEY" > "$HOME/.amplihack/litellm.key.new"
unset LITELLM_KEY
mv "$HOME/.amplihack/litellm.key.new" "$HOME/.amplihack/litellm.key"
```

Revoke the previous virtual key in LiteLLM after confirming a new launch.
Amplihack does not create, rotate, or revoke gateway keys.

## Change the gateway

Override the configured root for one launch:

```bash
AMPLIHACK_LITELLM_ENDPOINT='https://backup-gateway.internal.example' \
  amplihack claude --litellm
```

Use HTTPS for remote endpoints. Cleartext HTTP is accepted only for a literal
address in `127.0.0.0/8` or literal `::1`; `http://localhost` is rejected.

## Disable routing

Disable routing for one launch, even if stored configuration is malformed:

```bash
amplihack copilot --no-litellm
```

To disable implicit activation permanently, unset every recognized
`AMPLIHACK_LITELLM_*` variable and remove or rename
`~/.amplihack/litellm-config.toml`.

## Troubleshoot a failed launch

Diagnostics begin with a stable code and never include the endpoint or
credential value:

```text
AH_LITELLM_READINESS: external LiteLLM gateway is not ready
```

| Code | Action |
| --- | --- |
| `AH_LITELLM_CONFIG` | Remove unknown, obsolete, empty, conflicting, or partial settings; verify `schema_version = 1`. |
| `AH_LITELLM_CREDENTIAL` | Configure exactly one credential source and make credential/config files private regular files owned by the current user. |
| `AH_LITELLM_ENDPOINT` | Supply a deployment-root URL without credentials, query, fragment, traversal, encoded separators, `/v1`, or completion paths. |
| `AH_LITELLM_DESTINATION` | Check DNS for mixed answers and metadata, link-local, multicast, unspecified, broadcast, or mapped-prohibited addresses. |
| `AH_LITELLM_READINESS` | Check gateway availability, TLS identity, media type, response size, and the 15-second deadline. |
| `AH_LITELLM_PROTOCOL` | Return one JSON object with exactly `status: "healthy"` and only the documented optional `db` field. |
| `AH_LITELLM_CAPABILITY` | Upgrade the agent CLI to one that proves the required custom-provider and no-fallback behavior. |
| `AH_LITELLM_ARGUMENT` | Remove model, remote, cloud, export, share, resume, connect, or passthrough options that can bypass the route. |
| `AH_LITELLM_EXECUTABLE_CHANGED` | Retry after resolving a concurrent executable replacement. |
| `AH_LITELLM_UNSUPPORTED` | Use a local `launch`, `claude`, `copilot`, or `RustyClawd` launch; Docker, auto, Codex, and Amplifier are unsupported. |

Readiness is intentionally one request with no proxy, redirect, retry, cookie,
decompression, ambient authorization header, or client credential. Correct the
gateway instead of adding launcher retries.

## Operate gateway-owned concerns

Configure these in LiteLLM or its platform, not amplihack:

| Concern | Owner |
| --- | --- |
| Gateway process and deployment | LiteLLM operator |
| Provider credentials and aliases | LiteLLM configuration and secret store |
| Database and migrations | LiteLLM operator |
| Usage, pricing, budgets, and rate limits | LiteLLM policy |
| Logs, metrics, traces, and dashboards | LiteLLM operator |

See the [external LiteLLM gateway reference](../reference/external-litellm-gateway.md)
for the complete contract.
