---
title: Route an Agent Through an External LiteLLM Gateway
description: Configure environment routing and launch Claude Code or GitHub Copilot CLI through an existing LiteLLM gateway.
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
- a gateway model alias supported by that deployment.

Use Claude Code `2.1.83` or newer when routing Claude. Amplihack enforces this
version requirement. Copilot CLI and RustyClawd do not use the Claude-specific
version gate.

Do not use a LiteLLM administrative key or an upstream provider key.

## 1. Verify Claude Code capability

If you intend to launch Claude Code, verify the executable that amplihack will
select:

```bash
command -v claude
claude --version
```

The reported semantic version must be `2.1.83` or newer. Amplihack repeats this
probe before setup and rejects missing executables, failed probes, malformed or
unrecognized output, prereleases below the minimum, and versions older than
`2.1.83`. It does not fall back to direct Anthropic routing.

## 2. Configure the route

Read a restricted virtual key from your secret manager and export the complete
three-variable configuration:

```bash
export AMPLIHACK_LITELLM_ENDPOINT='https://llm-gateway.internal.example'
export AMPLIHACK_LITELLM_API_KEY="$(secret-tool lookup service litellm-agent)"
export AMPLIHACK_LITELLM_MODEL='gateway-coding'
```

The endpoint may have no path or `/v1`; it must not contain credentials, a
query, or a fragment. Remote endpoints require HTTPS. Do not put the key in
command arguments, project configuration, or shell history.

## 3. Launch through the gateway

Launch Copilot:

```bash
amplihack copilot
```

Or launch Claude Code:

```bash
amplihack claude
```

Before the agent starts, amplihack validates the three values, rejects
conflicting launch controls, and projects the gateway route into the child
environment. For Claude Code, it validates the exact selected executable before
launch setup and requests subprocess environment scrubbing. The version probe
does not receive the gateway key or direct provider credentials. The child CLI
connects to the gateway directly.

Routed Claude Code cannot be started with host-side `--docker` or
`AMPLIHACK_USE_DOCKER`, because amplihack cannot attest the container's
executable before Docker operations. Launch outside Docker, or start
amplihack from inside a trusted container.

## 4. Verify disable

Confirm that ordinary launcher behavior remains available:

```bash
unset AMPLIHACK_LITELLM_ENDPOINT
unset AMPLIHACK_LITELLM_API_KEY
unset AMPLIHACK_LITELLM_MODEL
amplihack copilot
```

Routing is disabled only when all three variables are absent. Empty or partial
configuration fails closed.

## Next steps

- [Operate and troubleshoot the route](../howto/operate-external-litellm-route.md)
- [Review the environment contract](../reference/environment-variables.md#external-litellm-gateway-variables)
- [Understand why the gateway stays external](../concepts/external-litellm-boundary.md)
