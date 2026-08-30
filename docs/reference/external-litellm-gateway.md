---
title: External LiteLLM Gateway Reference
description: CLI, configuration, security, readiness, adapter, and error contract for external LiteLLM routing.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: reference
---

# External LiteLLM Gateway Reference

Amplihack configures supported agent CLIs to use an already-running,
operator-managed LiteLLM gateway. It never installs, starts, embeds, supervises,
stops, deploys, or manages LiteLLM.

## Supported commands

| Command | Adapter | `--litellm` | `--no-litellm` |
| --- | --- | --- | --- |
| `amplihack launch` | Anthropic | enable | disable |
| `amplihack claude` | Anthropic | enable | disable |
| `amplihack copilot` | OpenAI-compatible | enable | disable |
| `amplihack RustyClawd` | Anthropic | enable | disable |

Docker, auto mode, Codex, Amplifier, and unimplemented launchers return
`AH_LITELLM_UNSUPPORTED` before DNS, readiness traffic, or child creation.

## Activation and precedence

Sources are applied in this order:

1. command flag;
2. recognized environment configuration;
3. `~/.amplihack/litellm-config.toml`;
4. disabled.

`--litellm` requires a complete valid route. `--no-litellm` disables routing
and suppresses parsing and validation of LiteLLM environment and file
configuration. Supplying both flags is invalid.

Without either flag, any recognized LiteLLM configuration signal enables
routing and must produce a complete route. No signal leaves routing disabled.
An empty value from any recognized source is rejected; it does not erase that
source or expose a lower-precedence value.

## Environment variables

Only these variables are recognized:

| Variable | Use |
| --- | --- |
| `AMPLIHACK_LITELLM_ENDPOINT` | Deployment root URL. |
| `AMPLIHACK_LITELLM_API_KEY` | Inline restricted LiteLLM virtual key. |
| `AMPLIHACK_LITELLM_API_KEY_FILE` | Absolute path to a protected virtual-key file. |
| `AMPLIHACK_LITELLM_COPILOT_MODEL` | Required gateway model alias for Copilot only. |

Exactly one credential variable is required when routing is enabled. Claude
Code and RustyClawd retain their normal model selection; Copilot requires one
configured gateway model.

Unknown variables using the `AMPLIHACK_LITELLM_` prefix are configuration
errors while routing is enabled. Credentials are not accepted in TOML or CLI
arguments.

## TOML configuration

Path: `~/.amplihack/litellm-config.toml`

```toml
schema_version = 1
endpoint = "https://llm-gateway.internal.example"

[copilot]
model = "gateway-coding"
```

The schema permits only `schema_version`, `endpoint`, and optional
`[copilot].model`. Unknown, obsolete, duplicate, empty, malformed, partial, or
unsupported-version configuration is rejected.

## Protected file contract

Configuration and credential files are opened with bounded reads and must be:

- regular files owned by the effective user;
- free of symbolic links;
- linked exactly once;
- inaccessible to group and other users; and
- unchanged between security validation and use.

If the current platform cannot prove every safeguard, routed operation fails
before child creation.

## Endpoint contract

The endpoint is a deployment root. It must be an absolute hierarchical URL
with no username, password, query, or fragment.

HTTPS is accepted after destination validation. HTTP is accepted only for
literal IPv4 addresses in `127.0.0.0/8` and literal IPv6 `::1`.
`http://localhost` is not accepted.

Paths are rejected when they:

- contain dot segments, traversal, ambiguous escaping, or encoded separators;
- already name `/health/readiness` or `/v1`; or
- name chat, message, response, or completion endpoints.

Amplihack canonicalizes the root once, then derives readiness and adapter URLs
with structured URL operations.

## Destination policy

Amplihack resolves the hostname once. The entire answer set is rejected if any
address is metadata, link-local, multicast, unspecified, broadcast,
mapped-prohibited, or another non-gateway class. Valid private and loopback
addresses are accepted for HTTPS because external gateways may be internal.

A validated address is selected deterministically for readiness. The original
hostname remains the HTTP Host and TLS SNI/certificate identity. Child CLI DNS
resolution occurs later and is outside amplihack's control.

## Readiness request

The derived URL is `<deployment-root>/health/readiness`.

| Property | Contract |
| --- | --- |
| Method | `GET` |
| Request header | `Accept: application/json` |
| Authentication | none |
| Redirects | disabled |
| Retries | disabled |
| Ambient proxies | disabled |
| Cookies | disabled |
| Decompression | disabled |
| Ambient headers and client credentials | disabled |
| Total deadline | 15 seconds |
| Response body | maximum 8 KiB |

Connection, TLS, and body phases use bounded timeouts within the total
deadline. Only a 2xx response with a JSON media type is accepted.

## Readiness JSON

The body must contain exactly one top-level JSON object with exactly one
`status` member:

```json
{"status":"healthy"}
```

One optional `db` member is accepted:

```json
{"status":"healthy","db":"connected"}
```

The legacy value `"Not connected"` is also accepted because LiteLLM can be
healthy without a database. Unknown top-level fields, duplicate governed
members, trailing JSON, excessive nesting, malformed JSON, and all other
`status` or `db` values are rejected.

## Launcher projection

### Claude Code and RustyClawd

The Anthropic adapter sets the gateway deployment root and virtual key using
the target CLI's supported environment contract. It removes conflicting
provider credentials and routing selectors. No amplihack model override is
required.

### GitHub Copilot CLI

The Copilot adapter derives the OpenAI-compatible URL, sets the virtual key,
provider type, wire API, offline/no-remote controls, and configured model.
Automatic `--remote` injection is suppressed.

The installed Copilot CLI must prove support for custom-provider and offline
operation. Cloud, remote, export, sharing, connection, resume, and passthrough
controls are rejected. Explicit model controls must exactly match the
configured gateway alias.

## Argument and executable validation

Validation examines the final semantic child arguments, including
`--option=value` and `--option value`, before any network request. Arguments
after `--` remain positional. Option-like prompt text is not interpreted as a
launcher option.

The selected executable is resolved and checked before readiness, then its
identity is revalidated immediately before spawn. A replacement fails with
`AH_LITELLM_EXECUTABLE_CHANGED`.

## Secret handling

The virtual key is materialized only while building the final child
environment. It is excluded from arguments, errors, debug output, logs,
serialization, readiness traffic, and capability probes. Diagnostics name the
field and violated rule, never the rejected endpoint or credential.

## Stable errors

| Code | Meaning |
| --- | --- |
| `AH_LITELLM_CONFIG` | Activation or configuration is conflicting, missing, partial, empty, unknown, obsolete, malformed, or unsupported. |
| `AH_LITELLM_CREDENTIAL` | The credential value or credential file is invalid or insecure. |
| `AH_LITELLM_ENDPOINT` | The deployment root violates URL policy. |
| `AH_LITELLM_DESTINATION` | DNS or the complete resolved address set violates destination policy. |
| `AH_LITELLM_READINESS` | Connection, TLS, timeout, HTTP status, media type, redirect, or body-size validation failed. |
| `AH_LITELLM_PROTOCOL` | The bounded response violates the readiness JSON contract. |
| `AH_LITELLM_CAPABILITY` | The target executable cannot prove required gateway and no-fallback behavior. |
| `AH_LITELLM_ARGUMENT` | Effective child arguments conflict with the route. |
| `AH_LITELLM_EXECUTABLE_CHANGED` | The validated executable changed before spawn. |
| `AH_LITELLM_UNSUPPORTED` | The target or launch mode is unsupported. |

## Validation order

| Order | Check |
| --- | --- |
| 1 | Activation, source precedence, TOML schema, and completeness |
| 2 | Credential source and protected files |
| 3 | Endpoint parsing and canonicalization |
| 4 | Supported target and launch mode |
| 5 | Final semantic child arguments |
| 6 | Local target capability |
| 7 | Complete resolved destination set |
| 8 | Pinned readiness transport and HTTP response |
| 9 | Bounded readiness JSON |
| 10 | Executable identity immediately before spawn |

No failed check starts a child or falls back to another provider.

## Related documentation

- [External gateway tutorial](../tutorials/external-litellm-gateway.md)
- [External gateway operations](../howto/operate-external-litellm-route.md)
- [Why the gateway stays external](../concepts/external-litellm-boundary.md)
