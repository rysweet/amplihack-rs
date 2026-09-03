---
title: Route an Agent Through an External LiteLLM Gateway
description: Configure environment routing and launch Claude Code or GitHub Copilot CLI through an existing LiteLLM gateway.
last_updated: 2026-09-02
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

The gateway must expose the protocol required by the selected client:

| Client | Required LiteLLM compatibility surface |
| --- | --- |
| Claude Code | Anthropic-compatible API |
| RustyClawd | Anthropic-compatible API |
| GitHub Copilot CLI | OpenAI-compatible completions API under `/v1` |

Configuring only one compatibility surface does not satisfy clients that use
the other protocol. For Copilot, confirm that the selected model and route
support streaming responses and tool calls before launching an agent.

Use Claude Code `2.1.247` when routing Claude. Install GitHub Copilot CLI from
npm package `1.0.83-2`. Its packaged runtime reports `1.0.83-2` in a clean
home; a previously downloaded user-cache update can make the same launcher
report `1.0.83-3`. Amplihack accepts those two tested runtimes while still
attesting the exact `1.0.83-2` npm package. Routed RustyClawd launches must
resolve to the canonical Cargo binary installed from the pinned git revision
documented below.

Do not use a LiteLLM administrative key or an upstream provider key.

## 1. Verify agent CLI capability

If you intend to launch Claude Code or Copilot, verify the executable that
amplihack will select:

```bash
command -v claude
claude --version
command -v copilot
copilot --version
```

Claude must report exactly `2.1.247`; Copilot must report `1.0.83-2` or
`1.0.83-3`. Amplihack repeats the applicable probe before setup and rejects
missing executables, failed probes, malformed or unrecognized output, and every
release outside the runtime-tested attestation set. It does not fall back to
direct provider routing.

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

Or launch RustyClawd:

```bash
amplihack rustyclawd
```

Before the agent starts, amplihack validates the three values, rejects
conflicting launch controls, and projects the gateway route into the child
environment. For Claude Code and Copilot, it validates the exact selected
executable before launch setup and requests subprocess environment scrubbing.
The version probe does not receive the gateway key or direct provider
credentials. The child CLI connects to the gateway directly. RustyClawd uses
its native Anthropic-compatible gateway transport with an explicit provider
and model; amplihack maps the endpoint, virtual key, and alias to
`ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_MODEL`.

Routed Claude Code and Copilot cannot be started with host-side `--docker` or
`AMPLIHACK_USE_DOCKER`, because amplihack cannot attest the container
executables before Docker operations. Launch outside Docker, or start amplihack
from inside a trusted container.

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
