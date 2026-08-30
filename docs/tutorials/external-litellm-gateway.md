---
title: Route an Agent Through an External LiteLLM Gateway
description: Configure a protected route and launch Claude Code or GitHub Copilot CLI through an existing LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: tutorial
---

# Route an Agent Through an External LiteLLM Gateway

This tutorial routes an agent through a LiteLLM gateway that is already
running. Amplihack does not install or start the gateway.

## Prerequisites

- amplihack and a supported agent CLI installed;
- an HTTPS LiteLLM deployment root, such as
  `https://llm-gateway.internal.example`;
- a restricted LiteLLM virtual key; and
- for Copilot, a gateway model alias supported by that deployment.

Do not use a LiteLLM administrative key or an upstream provider key.

## 1. Store the virtual key

Create a private regular file without following an existing symlink:

```bash
mkdir -p "$HOME/.amplihack"
chmod 700 "$HOME/.amplihack"
test ! -e "$HOME/.amplihack/litellm.key"
install -m 600 /dev/null "$HOME/.amplihack/litellm.key"
read -rsp 'LiteLLM virtual key: ' LITELLM_KEY
printf '%s\n' "$LITELLM_KEY" > "$HOME/.amplihack/litellm.key"
unset LITELLM_KEY
```

Amplihack rejects symbolic links, non-regular files, files owned by another
user, files with group or other permissions, and files with multiple hard
links.

## 2. Configure the deployment root

Create `~/.amplihack/litellm-config.toml`:

```toml
schema_version = 1
endpoint = "https://llm-gateway.internal.example"

[copilot]
model = "gateway-coding"
```

Protect the file:

```bash
chmod 600 "$HOME/.amplihack/litellm-config.toml"
export AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key"
```

The endpoint is the deployment root, not `/health/readiness`, `/v1`, or a
completion endpoint. Omit `[copilot]` when you do not use Copilot.

## 3. Launch through the gateway

Launch Copilot:

```bash
amplihack copilot --litellm
```

Or launch Claude Code:

```bash
amplihack claude --litellm
```

Before the agent starts, amplihack validates the route and performs one
unauthenticated request to:

```text
https://llm-gateway.internal.example/health/readiness
```

The request does not use the virtual key. A failure exits with a stable
`AH_LITELLM_*` diagnostic and does not start the agent.

## 4. Verify explicit disable

Confirm that ordinary launcher behavior remains available:

```bash
amplihack copilot --no-litellm
```

`--no-litellm` takes precedence over environment and file configuration. It
does not parse or validate those route inputs.

## Use environment-only configuration

Environment values override the TOML file:

```bash
export AMPLIHACK_LITELLM_ENDPOINT='https://llm-gateway.internal.example'
export AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key"
export AMPLIHACK_LITELLM_COPILOT_MODEL='gateway-coding'
amplihack copilot --litellm
```

For Claude Code or RustyClawd, omit
`AMPLIHACK_LITELLM_COPILOT_MODEL`. Without either activation flag, recognized
LiteLLM configuration still enables routing; use `--no-litellm` to disable it
explicitly.

## Next steps

- [Operate and troubleshoot the route](../howto/operate-external-litellm-route.md)
- [Review the complete configuration contract](../reference/external-litellm-gateway.md)
- [Understand why the gateway stays external](../concepts/external-litellm-boundary.md)
