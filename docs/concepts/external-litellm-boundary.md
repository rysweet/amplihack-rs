---
title: Why the LiteLLM Gateway Stays External
description: Explains amplihack's control-plane-only integration with an operator-managed LiteLLM gateway.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: explanation
---

# Why the LiteLLM Gateway Stays External

Amplihack integrates with LiteLLM only at launch time. It validates an existing
gateway, constrains the selected agent CLI, and builds the child environment.
LiteLLM remains a separately installed and operated service.

## Data and control paths

```text
configuration ----> amplihack launch policy ----> agent CLI
                           |                          |
                           | readiness check          | inference traffic
                           v                          v
                    external LiteLLM gateway <--------
```

Amplihack makes one unauthenticated readiness request before starting the child.
Prompts, responses, streaming, retries, and inference credentials then travel
directly between the agent CLI and LiteLLM. Amplihack is not an inference proxy.

## Why amplihack does not embed LiteLLM

The former Python `litellm` dependency was removed after a supply-chain
incident. External routing does not reverse that decision:

- no LiteLLM package is linked, imported, downloaded, or installed;
- no LiteLLM process, container, database, dashboard, or collector is managed;
- no provider credential is stored by amplihack;
- no model request or response body passes through amplihack; and
- no embedded callback or gateway-client compatibility layer remains.

The gateway operator chooses the LiteLLM version, deployment controls, provider
credentials, model aliases, accounting, retention, and availability policy.

## Why routing fails closed

A partially configured route can silently fall back to a provider the operator
did not intend. Amplihack therefore treats any recognized LiteLLM configuration
signal as an intent to route and rejects incomplete or unsafe configuration.

Before child creation, amplihack:

1. resolves configuration using a fixed precedence;
2. validates protected configuration and credential files;
3. canonicalizes the deployment root;
4. rejects unsupported launch modes and bypass arguments;
5. checks the target CLI's gateway capability;
6. validates every resolved destination address;
7. performs one bounded readiness request; and
8. revalidates the selected executable before spawning it.

There is no fallback to a direct provider when one of these checks fails.
`--no-litellm` is the explicit escape hatch: it disables routing without
parsing stale LiteLLM environment or file configuration.

## Why destination validation is strict

The gateway URL is privileged configuration because amplihack connects to it
before child creation. HTTPS endpoints may resolve to public, private, or
loopback addresses, but metadata, link-local, multicast, unspecified,
broadcast, and other prohibited address classes are rejected.

Amplihack resolves a hostname once and rejects the complete answer set when any
answer is prohibited. The readiness connection uses a deterministically
selected validated address while retaining the configured hostname for HTTP
Host and TLS identity checks. This prevents mixed-answer and DNS-rebinding
bypasses during readiness.

The child CLI performs its own later DNS resolution. Amplihack cannot pin that
resolution and does not claim otherwise.

## Ownership boundary

| Concern | Owner |
| --- | --- |
| Route validation and child isolation | amplihack |
| Provider credentials and model mappings | LiteLLM operator |
| Gateway deployment and TLS | LiteLLM operator |
| Usage, pricing, budgets, and rate limits | LiteLLM operator |
| Prompt and response transport | agent CLI and LiteLLM |
| Gateway logs, metrics, and retention | LiteLLM operator |

The integration supports local `launch`, `claude`, `copilot`, and `RustyClawd`
launches. Docker, auto mode, Codex, Amplifier, and other launchers remain
outside the reviewed trust boundary and are rejected before network access.

## Related documentation

- [Route an agent through an external LiteLLM gateway](../tutorials/external-litellm-gateway.md)
- [Operate an external LiteLLM route](../howto/operate-external-litellm-route.md)
- [External LiteLLM gateway reference](../reference/external-litellm-gateway.md)
