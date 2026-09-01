---
title: Why the LiteLLM Gateway Stays External
description: Explains amplihack's control-plane-only integration with an operator-managed LiteLLM gateway.
last_updated: 2026-09-01
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
bypass arguments, including custom-agent loading and every supported plugin
loading option (`--plugin-dir` for all launchers, `--plugin-url` for
Claude/RustyClawd, and Copilot's trusted-configuration `--add-dir`), and removes
conflicting provider credentials and selectors from the child environment.
Routed Claude launches use safe mode and do not load the UVX plugin directory.
Routed Copilot launches receive a fresh, empty `COPILOT_HOME` that exists only
for the child process lifetime. This prevents persisted user-scoped plugins and
their contributed agents, hooks, and MCP servers from loading even when no
plugin option appears in argv. `COPILOT_HOME` does not disable repository
configuration discovery. Because Copilot has no supported switch to disable
repository custom agents and those agents may select a model, amplihack rejects
routed launches from workspaces containing `.github/agents`. The user's normal
Copilot home is neither read nor modified by the routed session.
Amplihack retains the validated route as data and removes all three
`AMPLIHACK_LITELLM_*` variables from checkout, Docker probe/build, update,
bootstrap, freshness, memory-detection, installation, and background-indexing
subprocesses. Only the final supported agent receives translated provider
credentials. For Docker launches, the `docker run` client receives only the
virtual key needed to transfer that key into the final container; endpoint and
model are command arguments, while Docker probes and builds receive none of the
route variables. The child CLI owns connection, DNS, TLS, and gateway readiness
behavior.

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

Claude Code routing currently requires the exact `claude` executable selected
for launch to report version `2.1.247`. This is an explicit attestation set, not
a semver lower bound: later releases are rejected until the real-CLI isolation
test proves that Bash, hooks, and stdio MCP servers cannot read the gateway
token. Amplihack probes that executable before checkout, auto-mode staging,
memory configuration, session tracking, Docker operations, or child creation.
A missing executable, failed probe, unrecognized output, malformed version, or
any version outside the attestation set rejects the launch.

The probe runs without the LiteLLM virtual key or direct provider credentials.
The version gate and `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` form one capability
check; setting the flag manually does not bypass the exact-version policy.
On Linux, the verified Claude release enforces this boundary with `bubblewrap`
and `socat` and refuses to start if either dependency is unavailable.

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
When the fixed default image exists but its routing revision or amplihack
version label is stale, the launcher rebuilds it automatically before launch.
Source checkouts with a root Dockerfile rebuild from that definition. Installed
binaries without that source asset create a temporary upgrade layer from the
existing image and replace its amplihack executable with the exact running
binary; the base image's agent tools and runtime remain intact.
Auto mode applies the same restrictions; Codex and Amplifier are rejected.

## Related documentation

- [Route an agent through an external LiteLLM gateway](../tutorials/external-litellm-gateway.md)
- [Operate an external LiteLLM route](../howto/operate-external-litellm-route.md)
- [External LiteLLM environment variables](../reference/environment-variables.md#external-litellm-gateway-variables)
