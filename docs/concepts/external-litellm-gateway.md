---
title: External LiteLLM Gateway Architecture
description: How amplihack routes supported agent CLIs through an operator-managed LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: explanation
---

# External LiteLLM Gateway Architecture

Amplihack can route Claude Code, GitHub Copilot CLI, and rustyclawd through an
external LiteLLM gateway. Amplihack validates the routing contract and prepares
the child process; it does not proxy model traffic.

## Data path

```text
AMPLIHACK_LITELLM_* configuration
                 |
                 v
       amplihack launch policy
       - validates configuration
       - rejects bypass options
       - isolates child environment
                 |
                 v
 Claude Code / Copilot CLI / rustyclawd
                 |
                 v
       external LiteLLM gateway
          |                 |
          v                 v
   model providers     PostgreSQL
                      keys, usage,
                      spend, limits
```

Prompts, streamed response chunks, cancellation, and agent exit status travel
directly between the child CLI and LiteLLM. Amplihack never listens on an
inference port, buffers a response, retries a model request, calculates spend,
or writes prompt content to telemetry.

## Responsibilities

| Component | Responsibility |
| --- | --- |
| Amplihack | Validate complete gateway configuration, reject bypasses, remove conflicting inherited settings, and create the child environment. |
| Agent CLI | Send requests, stream responses, handle cancellation, and report gateway errors. |
| LiteLLM | Authenticate virtual keys, route model aliases, record usage and spend, and enforce configured budgets or rate limits. |
| PostgreSQL | Persist LiteLLM keys, usage, spend, budgets, and rate-limit state. |
| LiteLLM UI | Provide the authoritative view of agent usage, spend, and controls. |
| OTel Collector, Prometheus, Grafana | Provide optional infrastructure telemetry without prompt, completion, or authorization data. |

This boundary keeps accounting and enforcement in the component that sees every
gateway request. A process-local counter in amplihack could not provide durable
or shared limits across multiple launches.

## Fail-closed routing

Gateway routing is disabled only when all three `AMPLIHACK_LITELLM_*`
variables are absent. If any variable is present, all three must be valid.
Amplihack then requires a supported launcher, rejects bypass arguments, and
removes inherited provider settings that could select another network path.

Conflicting command-line options are rejected before launch. Conflicting
credentials and provider-selection variables are removed from the final child
environment before the restricted LiteLLM virtual key is added.

The routing plan is applied after the ordinary child environment is built, so
later environment construction cannot restore removed provider credentials.
Secrets are supplied only through child environment variables, never command
arguments, logs, diagnostics, serialization, or debug output.

## Launcher protocols

Claude Code and rustyclawd use LiteLLM's Anthropic-compatible API. Copilot CLI
uses LiteLLM's OpenAI-compatible BYOK API. The configured model is a LiteLLM
alias and is enforced for primary and fallback model selection.

Codex, Amplifier, and unknown launch targets are rejected while gateway routing
is enabled. Supporting a launcher requires a reviewed environment projection,
argument policy, and protocol contract; an executable name alone is not enough.

## Docker trust boundary

The outer amplihack process forwards the three configuration variable names
into the container. The API key value is inherited by the Docker client and is
not embedded in `docker run` arguments. The amplihack process inside the
container validates the same configuration again and builds the final agent
environment.

Container loopback is not host loopback. Docker launches therefore reject
`127.0.0.0/8` and `::1` gateway endpoints instead of enabling host networking
or exposing host-local services. Docker images must declare a compatible
LiteLLM-routing capability and amplihack version; unlabeled or stale images
fail before the agent starts.

## Supply-chain boundary

LiteLLM is not a Python or Rust dependency of amplihack. The optional reference
deployment runs version-and-digest-pinned images as services under the
operator's control. No amplihack command installs or starts LiteLLM
automatically.

This distinction preserves the repository's ban on the in-process Python
`litellm` dependency while permitting an explicitly operated external gateway.
This architecture has no embedded callback compatibility layer or direct Rust
gateway client.

## Related documentation

- [LiteLLM gateway quickstart](../tutorials/litellm-gateway-quickstart.md)
- [Operate an external LiteLLM gateway](../howto/operate-external-litellm-gateway.md)
- [External LiteLLM gateway reference](../reference/external-litellm-gateway.md)
