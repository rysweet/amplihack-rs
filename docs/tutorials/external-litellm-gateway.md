---
title: Route an Agent Through an External LiteLLM Gateway
description: Route Claude Code, GitHub Copilot CLI, or RustyClawd through an external LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: tutorial
---

# Route an Agent Through an External LiteLLM Gateway

This tutorial routes an amplihack agent through an existing LiteLLM gateway.
Amplihack validates the gateway and configures the agent process; it does not
install, embed, provision, start, stop, upgrade, or administer LiteLLM.

## What you will do

1. Verify an external gateway.
2. Store a gateway credential in a protected file.
3. Configure amplihack.
4. Launch Claude Code, Copilot, and RustyClawd through the gateway.
5. Confirm fail-closed behavior.

## Prerequisites

- amplihack and the target agent CLI are installed.
- a LiteLLM gateway is already running.
- the gateway exposes an Anthropic-compatible API for Claude Code and
  RustyClawd, an OpenAI-compatible API for GitHub Copilot CLI, and
  `GET /health/readiness`.
- the gateway has a model alias named `gateway-coding`.
- you have a LiteLLM virtual key. Do not use an upstream provider key.

For a shared gateway, use HTTPS with a certificate trusted by the host. Plain
HTTP is accepted only for a literal loopback address.

## 1. Verify the gateway response

For a local gateway listening on port 4000:

```bash
curl --fail-with-body \
  --header 'Accept: application/json' \
  http://127.0.0.1:4000/health/readiness
```

A usable response is one complete JSON object:

```json
{"status":"healthy","db":"connected"}
```

The object may contain only `status` and `db`. The `db` member may be absent.
A legacy gateway may return `"db":"Not connected"`. Extra members and other
status values are not ready.

## 2. Store the virtual key

Create the key file without exposing the key in shell history:

```bash
install -d -m 700 "$HOME/.amplihack"
install -m 600 /dev/null "$HOME/.amplihack/litellm.key"
read -rsp 'LiteLLM virtual key: ' LITELLM_KEY
printf '%s\n' "$LITELLM_KEY" > "$HOME/.amplihack/litellm.key"
unset LITELLM_KEY
```

Amplihack rejects credential files that are symlinks,
non-regular files, hard-linked, owned by another user, or accessible to group
or other users. It also validates the security of `~/.amplihack`.

## 3. Configure the endpoint

The configuration file is
`~/.amplihack/litellm-config.toml`:

```bash
cat > "$HOME/.amplihack/litellm-config.toml" <<'EOF'
schema_version = 1
endpoint = "http://127.0.0.1:4000"

[copilot]
model = "gateway-coding"
EOF
chmod 600 "$HOME/.amplihack/litellm-config.toml"
```

Select the credential file:

```bash
export AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key"
```

The TOML file never contains a credential. See the
[configuration reference](../reference/external-litellm-gateway.md#configuration)
for environment-only configuration and precedence.

## 4. Launch Claude Code

```bash
amplihack claude --litellm
```

The launch sequence validates the endpoint, checks the selected Claude
executable's gateway capability, and performs one readiness request before
Claude starts. It then launches Claude with the LiteLLM route and without
upstream provider credentials. The LiteLLM virtual key is intentionally passed
to the child as its gateway credential.

The same route is available from the default Claude launcher:

```bash
amplihack launch --litellm
```

## 5. Launch GitHub Copilot CLI

```bash
amplihack copilot --litellm
```

The adapter selects the configured `gateway-coding` model
automatically. Routed Copilot runs locally and offline from GitHub's remote
execution service. Amplihack suppresses its normal automatic `--remote` flag.

Do not add `--remote`, `--resume`, `--share`, or another `--model`. Those
arguments create a route around the configured gateway or make the effective
model ambiguous, so amplihack rejects them before contacting the gateway.

Routed launches also reject amplihack's append, checkout, resume, and continue
session modes. Start a new local session when using the gateway route.

## 6. Launch RustyClawd

```bash
amplihack RustyClawd --litellm
```

RustyClawd uses the same Anthropic-compatible endpoint and virtual key as
Claude Code. Amplihack validates the installed RustyClawd executable and
rechecks its identity immediately before spawn.

## 7. Confirm fail-closed behavior

Stop the gateway, then repeat a routed launch:

```bash
amplihack claude --litellm
```

Amplihack exits with an `AH_LITELLM_READINESS` diagnostic. Claude does not
start, and amplihack does not retry against Anthropic.

Force routing off for one launch:

```bash
amplihack claude --no-litellm
```

The `--no-litellm` flag takes precedence over the environment and
configuration file. The ordinary, unrouted launcher behavior is preserved.

## What you completed

Claude Code, Copilot, and RustyClawd now use the same operator-managed gateway.
The child receives the LiteLLM virtual key but not upstream provider
credentials. Amplihack remains outside the prompt and response data path.

For production setup, continue with
[Operate an external LiteLLM route](../howto/operate-external-litellm-route.md).
For every accepted setting and error code, use the
[external LiteLLM gateway reference](../reference/external-litellm-gateway.md).
