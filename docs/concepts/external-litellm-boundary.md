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
                                                      |
                                                      | inference traffic
                                                      v
                                             external LiteLLM gateway
```

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

Before child creation, amplihack requires all three gateway environment
variables, validates the endpoint and model, rejects unsupported launchers and
bypass arguments, and removes conflicting provider credentials and selectors
from the child environment. The child CLI owns connection, DNS, TLS, and
gateway readiness behavior.

Credentials unrelated to model routing, such as `GITHUB_TOKEN` and database
passwords, remain available to the child and its tools.

There is no fallback to a direct provider when launch policy rejects a route.
To disable routing, unset all three `AMPLIHACK_LITELLM_*` variables before
launch.

Claude Code receives `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`, which requests that
Claude keep the gateway token in its process while removing Anthropic and cloud
credentials from Bash, hook, and stdio MCP subprocess environments.

RustyClawd does not provide a verified equivalent control. Its process and all
descendants are therefore inside the credential trust boundary. Use a
short-lived LiteLLM virtual key restricted at the gateway by tenant, route,
model alias, budget, and rate, and launch RustyClawd only in trusted worktrees.

## Claude Code capability gate

Claude Code routing requires the exact `claude` executable selected for launch
to report a semantic version greater than or equal to `2.1.83`. Amplihack
probes that executable before setup, filesystem changes, Docker
operations, or child creation. A missing executable, failed probe, unrecognized
output, malformed version, prerelease below the minimum, or version below the
minimum will reject the launch.

The probe runs without the LiteLLM virtual key or direct provider credentials.
The version gate and `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` form one capability
check; setting the flag manually does not bypass the minimum.

## Ownership boundary

| Concern | Owner |
| --- | --- |
| Route validation and child isolation | amplihack |
| Provider credentials and model mappings | LiteLLM operator |
| Gateway deployment and TLS | LiteLLM operator |
| Usage, pricing, budgets, and rate limits | LiteLLM operator |
| Prompt and response transport | agent CLI and LiteLLM |
| Gateway logs, metrics, and retention | LiteLLM operator |

The integration supports `launch`, `claude`, `copilot`, and `rustyclawd`.
Codex, Amplifier, and unknown launch targets are rejected while gateway
routing is configured. Host-side `--docker` and `AMPLIHACK_USE_DOCKER` launches
of routed Claude Code are rejected because the container executable cannot be
attested before Docker operations; a launch already inside a trusted container
probes its exact executable normally. Docker routing for other supported
targets requires a container-reachable HTTPS gateway and a compatible image.
Auto mode applies the same restrictions; Codex and Amplifier are rejected.

## Related documentation

- [Route an agent through an external LiteLLM gateway](../tutorials/external-litellm-gateway.md)
- [Operate an external LiteLLM route](../howto/operate-external-litellm-route.md)
- [External LiteLLM environment variables](../reference/environment-variables.md#external-litellm-gateway-variables)
