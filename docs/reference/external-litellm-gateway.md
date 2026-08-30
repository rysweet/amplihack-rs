---
title: External LiteLLM Gateway Reference
description: Complete configuration, launcher routing, Docker, security, and Rust API contract.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: reference
---

# External LiteLLM Gateway Reference

This reference defines amplihack's opt-in external LiteLLM routing contract.

## Configuration

| Variable | Required | Accepted value |
| --- | --- | --- |
| `AMPLIHACK_LITELLM_ENDPOINT` | Yes when routing is enabled | Absolute HTTPS gateway URL, or HTTP with a literal IPv4 address in `127.0.0.0/8` or literal IPv6 `::1`; path empty or `/v1`. |
| `AMPLIHACK_LITELLM_API_KEY` | Yes when routing is enabled | Non-empty restricted LiteLLM virtual key, maximum 4096 bytes. |
| `AMPLIHACK_LITELLM_MODEL` | Yes when routing is enabled | Gateway model alias matching `[A-Za-z0-9._:/-]{1,128}`. |

All three variables absent disables gateway routing and preserves existing
launcher behavior. Every other combination is an error before process or
Docker execution.

Values must be Unicode, non-empty, free of control characters, and unchanged by
trimming. Error output identifies the variable and rule but never includes the
rejected value.

The endpoint must be an absolute hierarchical URL with a host and no username,
password, query, or fragment. `http://localhost`, non-loopback HTTP, relative
URLs, opaque URLs, and paths other than `/v1` are invalid. Copilot routing
normalizes the OpenAI-compatible base to end in `/v1` exactly once.

## Supported launchers

| Launcher | Command | Gateway protocol |
| --- | --- | --- |
| Claude Code | `amplihack launch` or `amplihack claude` | Anthropic-compatible |
| GitHub Copilot CLI | `amplihack copilot` | OpenAI-compatible BYOK |
| rustyclawd | `amplihack rustyclawd` | Anthropic-compatible |

Codex, Amplifier, and unknown binaries are unsupported while gateway routing
is enabled.

## Claude Code and rustyclawd

Amplihack removes inherited Anthropic provider keys and headers, Claude
Bedrock/Vertex/Foundry selectors, relevant AWS credentials, and Google
application credentials. It sets:

| Variable | Value |
| --- | --- |
| `ANTHROPIC_BASE_URL` | Configured gateway endpoint |
| `ANTHROPIC_AUTH_TOKEN` | Configured virtual key |
| `ANTHROPIC_MODEL` | Configured model alias |
| `ANTHROPIC_SMALL_FAST_MODEL` | Configured model alias |

Claude settings sources are disabled. Claude cloud, teleport, remote-control,
environment, settings, `--agent`, `--agents`, `--from-pr`, and other
session-reuse options are rejected. rustyclawd uses the same Claude-compatible
settings, agent, remote, and session restrictions.

## GitHub Copilot CLI

Amplihack removes inherited OpenAI credentials and conflicting Copilot
provider, bearer-token, header, wire-model, GHES, offline, and transport
settings. It rejects custom-agent selection, marks the provider key as a
Copilot secret environment variable so shell and MCP subprocesses cannot
inherit it, and sets:

| Variable | Value |
| --- | --- |
| `COPILOT_PROVIDER_BASE_URL` | Gateway endpoint normalized to `/v1` |
| `COPILOT_PROVIDER_API_KEY` | Configured virtual key |
| `COPILOT_PROVIDER_TYPE` | `openai` |
| `COPILOT_PROVIDER_WIRE_API` | `completions` |
| `COPILOT_MODEL` | Configured model alias |

Amplihack suppresses ordinary `--remote` injection and adds `--no-remote` and
`--no-remote-export`. Cloud, remote, export, share, connect, continue, resume,
and session-ID options are rejected.

## Argument and model enforcement

Both `--option value` and `--option=value` forms are recognized. Option-like
Copilot prompt content is not treated as a routing option, and arguments after
`--` are not interpreted as launcher options.

Explicit `--model` and `--fallback-model` values are accepted only when each
exactly matches `AMPLIHACK_LITELLM_MODEL`. Missing values, mismatches, and
duplicate occurrences are errors. `--append` is rejected because an existing
session's routing cannot be verified. Auto mode validates the gateway
configuration, supported launcher, and passthrough arguments before creating
session state or spawning a nested process.

## Docker contract

Docker launches validate before invoking Docker, reject loopback endpoints,
forward all three variables, and repeat validation inside the container. The
API key uses `docker run --env AMPLIHACK_LITELLM_API_KEY`, never
`--env AMPLIHACK_LITELLM_API_KEY=value`.

Images must declare the LiteLLM routing capability and the running amplihack
version. Unlabeled, stale, or external images that cannot prove compatibility
fail closed. The repository does not ship or pull an image definition; operators
must build the image from matching source and apply
`org.amplihack.litellm-routing=2` and
`org.amplihack.version=<amplihack-version>` in their own image pipeline.
These labels declare compatibility; they are not provenance attestations.
Amplihack never pulls the mutable local `amplihack:latest` tag. Operators own
that local image and should pin its digest or verify its signature in their
image pipeline. A principal that can replace local Docker images already
controls the Docker daemon and is outside the launcher trust boundary.
Amplihack does not enable host networking or add `host.docker.internal`.

## Secret handling

The virtual key is available only when applying the final environment to a
child command. It is excluded from command arguments, debug output, errors,
diagnostics, logs, serialization, and Docker image metadata. Diagnostics name
configuration fields and violated rules rather than echoing their values.

## Accounting and controls

LiteLLM and PostgreSQL own usage, spend, budgets, rate limits, and reports for
all agent traffic. Amplihack does not maintain a second accounting ledger.

Reference virtual keys have no budget or rate limit unless the operator
explicitly sets one. Missing cost remains unknown rather than becoming an exact
zero. Readiness and liveness probes do not invoke a model.

The LiteLLM UI is authoritative for request accounting. Prometheus and Grafana
show privacy-filtered infrastructure telemetry and do not claim complete agent
cost visibility.

## Rust API

The public adapter is `amplihack_utils::litellm_proxy`:

```rust
pub const ENDPOINT_ENV: &str = "AMPLIHACK_LITELLM_ENDPOINT";
pub const API_KEY_ENV: &str = "AMPLIHACK_LITELLM_API_KEY";
pub const MODEL_ENV: &str = "AMPLIHACK_LITELLM_MODEL";

pub fn proxy_requested() -> bool;
pub fn validate_environment() -> Result<bool, ProxyError>;
pub fn validate_launch_args(
    target: CliTarget,
    args: &[String],
) -> Result<(), ProxyError>;
pub fn apply_proxy_to_command(
    command: &mut std::process::Command,
    target: CliTarget,
) -> Result<bool, ProxyError>;
```

There is no public HTTP client, chat-completions DTO, SSE parser, pricing API,
callback API, or amplihack-specific LiteLLM telemetry API. Applications send
inference traffic through one of the supported child launchers.

## Related documentation

- [External LiteLLM gateway architecture](../concepts/external-litellm-gateway.md)
- [LiteLLM gateway quickstart](../tutorials/litellm-gateway-quickstart.md)
- [Operate an external LiteLLM gateway](../howto/operate-external-litellm-gateway.md)
- [Environment variables](environment-variables.md)
