---
title: External LiteLLM Gateway Reference
description: CLI, configuration, security, readiness, adapter, and error contract for external LiteLLM routing.
last_updated: 2026-08-30
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: reference
---

# External LiteLLM Gateway Reference

The integration routes supported local agent launches through an
operator-managed LiteLLM gateway. Routing is opt-in, performs one strict
readiness check, and never falls back to a direct provider. Amplihack does not
install, embed, provision, start, stop, upgrade, or administer LiteLLM.

## Contents

- [Supported commands](#supported-commands)
- [Configuration](#configuration)
- [Configuration decision matrix](#configuration-decision-matrix)
- [Credential contract](#credential-contract)
- [Endpoint and destination policy](#endpoint-and-destination-policy)
- [Readiness contract](#readiness-contract)
- [Target adapters](#target-adapters)
- [Child argument policy](#child-argument-policy)
- [Environment isolation](#environment-isolation)
- [Rust API](#rust-api)
- [Error codes](#error-codes)
- [Validation and error precedence](#validation-and-error-precedence)

## Supported commands

| Command | Adapter | `--litellm` | `--no-litellm` |
|---|---|---:|---:|
| `amplihack launch` | Anthropic-compatible | yes | yes |
| `amplihack claude` | Anthropic-compatible | yes | yes |
| `amplihack copilot` | OpenAI-compatible Copilot custom provider | yes | yes |
| `amplihack RustyClawd` | Anthropic-compatible | yes | yes |

The two flags are mutually exclusive and are consumed by amplihack. They are
never forwarded to the child.

When external routing is enabled, Docker mode, auto mode, Codex, and Amplifier
are unsupported. These combinations fail before DNS, readiness, or child
spawn. The `--no-litellm` override restores their ordinary behavior.

Routed launches also reject append, checkout, resume, and continue modes.
Append mode fails as an unsupported launch mode; checkout, resume, and continue
fail as conflicting child-session arguments.

## Configuration

### Activation and precedence

Configuration precedence, highest first:

1. `--litellm` or `--no-litellm`
2. `AMPLIHACK_LITELLM_*` environment variables
3. `~/.amplihack/litellm-config.toml`
4. routing disabled

`--litellm` requires a complete valid route. `--no-litellm` disables routing
even when the environment or TOML file configures it. Without either flag, a
gateway endpoint in the environment or a valid TOML file enables routing.

Environment values override corresponding TOML values as whole values. Empty
environment values are invalid; they do not erase lower-precedence values.
Any partial or conflicting gateway configuration is rejected.

### Configuration decision matrix

This matrix is normative for the implementation. "Ignore route inputs" means
the ordinary launcher receives the same arguments and environment it would
receive if the feature did not exist.

| CLI state | Environment state | TOML state | Result |
|---|---|---|---|
| both `--litellm` and `--no-litellm` | any | any | `AH_LITELLM_CONFIG`; conflicting activation controls |
| `--no-litellm` | any | any | routing disabled; route inputs are not parsed or validated |
| `--litellm` | complete or completed by TOML | valid or absent | routing enabled |
| `--litellm` | incomplete after precedence | any | `AH_LITELLM_CONFIG`; missing route value |
| neither | endpoint present in environment | valid or absent | environment values override matching TOML values; routing enabled only if the merged route is complete |
| neither | no endpoint in environment | valid TOML with endpoint | routing enabled from TOML plus environment-only credential |
| neither | credential present but no endpoint in any source | absent | `AH_LITELLM_CONFIG`; partial route |
| neither | no route values | absent | routing disabled |
| enabling case | empty `AMPLIHACK_LITELLM_*` value | any | `AH_LITELLM_CONFIG`; empty values never erase lower-precedence values |
| enabling case | both credential variables set | any | `AH_LITELLM_CREDENTIAL`; credential-source conflict |
| enabling case | present TOML is malformed, has unknown keys, or has the wrong schema | present | `AH_LITELLM_CONFIG`, even when environment values would override its route values |

Precedence is per complete value, not per URL component or credential byte.
The endpoint and Copilot model may be overridden independently. Credentials
never come from TOML, and exactly one environment credential source is required
for an enabled route.

### Environment variables

| Variable | Required | Description |
|---|---:|---|
| `AMPLIHACK_LITELLM_ENDPOINT` | yes, unless TOML supplies it | Deployment root URL. |
| `AMPLIHACK_LITELLM_API_KEY` | exactly one credential source | Inline LiteLLM virtual key. |
| `AMPLIHACK_LITELLM_API_KEY_FILE` | exactly one credential source | Absolute path to a protected virtual-key file. |
| `AMPLIHACK_LITELLM_COPILOT_MODEL` | for Copilot, unless TOML supplies it | Gateway model alias selected for Copilot. |

Only the four variables in the table are recognized. Any other variable with
the `AMPLIHACK_LITELLM_` prefix fails with `AH_LITELLM_CONFIG` while routing is
enabled, so a misspelled control cannot appear active. All variables with that
prefix are removed from the routed child environment.

### TOML file

Path: `~/.amplihack/litellm-config.toml`

```toml
schema_version = 1
endpoint = "https://llm-gateway.internal.example"

[copilot]
model = "gateway-coding"
```

| Key | Type | Required | Constraints |
|---|---|---:|---|
| `schema_version` | integer | yes | Must equal `1`. |
| `endpoint` | string | yes | Must satisfy the endpoint policy below. |
| `copilot.model` | string | for Copilot | Non-empty gateway model alias; control characters are rejected. |

Unknown keys are errors. Secret-bearing keys such as `api_key`, `token`, and
provider credentials are errors. Obsolete embedded-runtime keys for proxy
processes, management APIs, databases, pricing, budgets, rate limits,
telemetry, or deployment are also errors.

The file is opened without following symlinks and is bounded in size. It must
be a regular file owned by the effective user, have one link, and grant no
access to group or other users. `~/.amplihack` must also be owned by the
effective user and inaccessible to group or other users. On systems where
equivalent ownership, link, and ACL checks cannot be proved, file-based
configuration fails closed. The maximum TOML file size is 64 KiB.

## Credential contract

Supply exactly one of:

```bash
export AMPLIHACK_LITELLM_API_KEY='a-user-scoped-virtual-key'
```

```bash
export AMPLIHACK_LITELLM_API_KEY_FILE="$HOME/.amplihack/litellm.key"
```

If both are set, configuration fails. Credentials cannot be supplied in TOML
or argv.

A credential:

- must be non-empty after removal of at most one terminal LF or CRLF;
- must be no more than 4 KiB;
- must not contain NUL or other control characters;
- preserves all other bytes, including leading and trailing spaces;
- is never serialized; and
- has constant redacted `Debug` and `Display` representations.

Credential files use the same no-follow, ownership, regular-file, one-link,
and private-permission checks as the TOML file. Reads are bounded and operate
on the validated open handle to prevent path replacement between validation
and use.

## Endpoint and destination policy

The endpoint is a deployment root, not a completion endpoint. Amplihack
derives readiness and target-specific API URLs by appending structured path
segments.

Accepted endpoints:

```text
https://llm-gateway.internal.example
https://llm-gateway.internal.example/team-a
http://127.0.0.1:4000
http://[::1]:4000
```

Rejected endpoints include:

- non-HTTP schemes;
- HTTP on a non-loopback host;
- user information, query strings, or fragments;
- ambiguous, dot, encoded separator, or encoded traversal path segments;
- unsafe authorities and non-canonical host names; and
- a URL that already names `/health/readiness`, `/v1`, or a completion route.

The implementation has one IDNA policy. IP literals bypass IDNA. Every DNS
name is converted once to canonical ASCII with UTS #46 non-transitional
processing, `UseSTD3ASCIIRules`, `CheckHyphens`, `CheckBidi`, `CheckJoiners`,
and DNS-length verification enabled. Conversion errors, empty labels, and a
trailing root dot are rejected; the ASCII result is lowercased. No transitional
mapping or second normalization path is allowed. That one canonical ASCII name
is used for policy checks, DNS lookup, the HTTP host, and TLS SNI/certificate
verification.

Plain HTTP is accepted only when the URL authority itself is an IPv4 address
in `127.0.0.0/8` or the IPv6 address `::1`. A DNS name is never
literal-loopback: `http://localhost`, an hosts-file alias, and any other DNS
name are rejected before resolution even if they would resolve only to
loopback addresses. With HTTPS, DNS names may resolve to loopback or other
private addresses if every answer passes destination policy.

DNS is resolved exactly once for the readiness request. The complete result
set is normalized and validated before connecting; a mixed safe and prohibited
set is rejected. One address from that validated set is selected and passed
directly to the connector, so the connector cannot perform a second DNS
lookup. The canonical hostname is still used for the HTTP host, TLS SNI, and
certificate verification. This connection pinning covers the readiness request
only; the child agent controls its later gateway connections.

The following destinations are always prohibited:

- cloud metadata and link-local ranges;
- multicast and unspecified addresses;
- IPv4 limited broadcast and mapped equivalents; and
- any address class that cannot identify one routable gateway.

## Readiness contract

Amplihack performs exactly one unauthenticated request:

```http
GET <deployment-root>/health/readiness
Accept: application/json
```

The request has:

- no authorization header;
- no proxy discovery;
- no redirects or retries;
- no cookies, decompression, ambient headers, or client certificate;
- bounded DNS, connection/TLS/response-header, response-body, and
  total-operation phases; and
- an 8 KiB response-body limit.

| Phase | Deadline |
|---|---:|
| DNS resolution | 5 seconds |
| Connection, TLS, and response headers | 5 seconds |
| Response body | 2 seconds |
| Entire readiness operation | 15 seconds |

There is no separate TLS-handshake deadline. TLS negotiation is bounded by the
five-second request phase and the total readiness deadline.

The response must have a successful HTTP status, a JSON media type, and exactly
one complete JSON object. The object may contain only `status` and `db`, with
no duplicate member. Nesting and allocation are bounded. JSON nesting may not
exceed 16 levels.

Accepted bodies:

```json
{"status":"healthy"}
```

```json
{"status":"healthy","db":"connected"}
```

```json
{"status":"healthy","db":"Not connected"}
```

`status` is case-sensitive and must be exactly `healthy`. `db` may be absent,
exactly `connected`, or the legacy value `Not connected`.

## Target adapters

### Claude and RustyClawd

The Anthropic adapter sets:

| Variable | Value |
|---|---|
| `ANTHROPIC_BASE_URL` | validated deployment root |
| `ANTHROPIC_AUTH_TOKEN` | LiteLLM virtual key |

It removes direct Anthropic credentials and conflicting base-URL variables.
It also starts the target with `--setting-sources ""`, disabling ambient user,
project, and local settings that could replace the validated provider route.
The selected executable must advertise this option in deterministic local
`--help` output or the launch fails with `AH_LITELLM_CAPABILITY`.

### GitHub Copilot CLI

The Copilot adapter sets:

| Variable | Value |
|---|---|
| `COPILOT_PROVIDER_TYPE` | `openai` |
| `COPILOT_PROVIDER_BASE_URL` | `<deployment-root>/v1`, derived through URL path segments |
| `COPILOT_PROVIDER_API_KEY` | LiteLLM virtual key |
| `COPILOT_MODEL` | configured gateway model |
| `COPILOT_OFFLINE` | `true` |

The selected Copilot executable must prove support for the custom-provider
variables and local no-fallback operation through deterministic local
`--version` and `--help` output. The executable identity is checked again
immediately before spawn.

Executable identity consists of the canonical path and stable file identity
metadata available on the host. Capability probes run with a sanitized
environment, no network configuration, and fixed arguments. A replacement or
metadata change between the probe and spawn fails with
`AH_LITELLM_EXECUTABLE_CHANGED`.

When routing is disabled, Copilot's existing automatic `--remote` behavior is
unchanged. When routing is enabled, automatic `--remote` injection is
suppressed and an explicit remote request is rejected.

## Child argument policy

Amplihack validates the final semantic argument vector after adding its own
defaults. Both `--flag value` and `--flag=value` forms are recognized.
Duplicate semantic flags do not bypass validation.

Routed launches reject:

- direct-provider endpoint, provider, or credential overrides;
- remote or cloud execution;
- append and checkout launch modes;
- connect, share, export, resume, and continue paths;
- passthrough or nested-command forms that bypass validated arguments; and
- a Copilot model that is absent, duplicated, or differs from the configured
  model.

## Environment isolation

`EnvBuilder` applies gateway removals before gateway additions and applies the
gateway patch last.

The routed child environment starts with this explicit ambient allowlist:

`HOME`, `PATH`, `USER`, `LOGNAME`, `TMPDIR`, `TMP`, `TEMP`, `LANG`, `LC_ALL`,
`LC_CTYPE`, `TERM`, `COLORTERM`, `NO_COLOR`, and `FORCE_COLOR`.

Amplihack then adds the runtime, session, agent, home, asset-resolver, and
project-graph variables required by the ordinary local launch. Finally, it
adds the selected adapter variables. The LiteLLM virtual key is intentionally
passed as the adapter's gateway credential.

The routed child does not inherit:

- any `AMPLIHACK_LITELLM_*` input;
- credentials and endpoint variables for the opposite adapter;
- direct Anthropic, OpenAI, Azure OpenAI, or other provider credentials;
- `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and lowercase equivalents;
- dynamic-loader and runtime injection variables;
- `NODE_OPTIONS`; or
- unrelated cloud credentials that could create another provider path.

Upstream and direct-provider credentials are excluded; the LiteLLM virtual key
is not. Secret values do not appear in intermediate debug maps or command
diagnostics.

## Rust API

The `amplihack_cli::external_litellm` module exposes a type-state route
lifecycle. Callers cannot apply gateway credentials until destination,
capability, and readiness validation have produced a `PreparedRoute`.

| State or entry point | Purpose | Next operation |
|---|---|---|
| `GatewayControl::new(enabled, disabled)` | Represents the two CLI activation controls. | Pass to `resolve`. |
| `resolve(...) -> Result<Option<Route>>` | Applies activation, configuration, target, mode, and argument policy. `None` means ordinary unrouted launch. | Call `Route::resolve_destination`. |
| `Route` | Holds a validated route without a selected network destination. Its fields are private. | `resolve_destination() -> Result<ResolvedRoute>` |
| `ResolvedRoute` | Holds the route and one address selected from the validated complete DNS answer set. Its fields are private. | `prepare(&BinaryInfo) -> Result<PreparedRoute>` |
| `PreparedRoute` | Holds the route and captured executable identity after capability and readiness checks. Its fields are private. | Apply the environment, validate the final command, revalidate the executable, then spawn. |

`PreparedRoute` exposes these launch-boundary methods:

| Method | Contract |
|---|---|
| `apply_environment(EnvBuilder) -> EnvBuilder` | Removes inherited bypasses and applies the adapter patch last. |
| `validate_final_command(&Command) -> Result<()>` | Rejects semantic route overrides in the command that will be spawned. |
| `revalidate_executable() -> Result<()>` | Fails if the selected executable changed after preparation. Call immediately before spawn. |

`resolve_executable(tool)` resolves an already-installed executable without
invoking installation, repair, update, Docker, or remote execution. Gateway
callers use it instead of the ordinary bootstrap path.

The API returns `anyhow::Result` values whose user-facing diagnostics follow
the stable `AH_LITELLM_*` contract below. Route and secret internals are
private; callers cannot construct a prepared route or read the virtual key
directly.

## Error codes

Diagnostics begin with one stable code and contain no endpoint text,
credentials, response bodies, headers, environment dumps, command objects, or
nested transport errors.

| Code | Meaning |
|---|---|
| `AH_LITELLM_CONFIG` | Activation flags conflict; configuration is missing, partial, empty, unknown, obsolete, malformed, or uses an unsupported schema. |
| `AH_LITELLM_CREDENTIAL` | The credential value or credential file is invalid or insecure. |
| `AH_LITELLM_ENDPOINT` | The deployment root is malformed or violates URL policy. |
| `AH_LITELLM_DESTINATION` | DNS or the resolved address set violates destination policy. |
| `AH_LITELLM_READINESS` | Connection, TLS, timeout, HTTP status, media type, redirect, or response-size validation failed after destination validation. |
| `AH_LITELLM_PROTOCOL` | The bounded response is not one complete accepted JSON object, contains an unknown or duplicate member, exceeds nesting limits, or has an unsupported `status` or `db` value. |
| `AH_LITELLM_CAPABILITY` | The target executable cannot prove required external-gateway behavior. |
| `AH_LITELLM_ARGUMENT` | The effective child arguments conflict with the route. |
| `AH_LITELLM_EXECUTABLE_CHANGED` | The validated executable changed before spawn. |
| `AH_LITELLM_UNSUPPORTED` | The selected target or launch mode does not support routing. |

### Validation and error precedence

Only one stable code is emitted. If several defects are present, validation
stops at the first applicable stage in this order:

| Order | Stage | Stable code |
|---:|---|---|
| 1 | Resolve activation flags, source precedence, TOML schema, unknown keys, and route completeness | `AH_LITELLM_CONFIG` |
| 2 | Load and validate the single credential source | `AH_LITELLM_CREDENTIAL` |
| 3 | Parse and canonicalize the deployment root, including the single IDNA policy | `AH_LITELLM_ENDPOINT` |
| 4 | Reject unsupported targets and launch modes | `AH_LITELLM_UNSUPPORTED` |
| 5 | Validate the final semantic child argument vector | `AH_LITELLM_ARGUMENT` |
| 6 | Resolve once and validate the complete destination set | `AH_LITELLM_DESTINATION` |
| 7 | Probe the selected executable's local gateway capability | `AH_LITELLM_CAPABILITY` |
| 8 | Perform the pinned readiness transport and HTTP checks | `AH_LITELLM_READINESS` |
| 9 | Validate the bounded readiness JSON object | `AH_LITELLM_PROTOCOL` |
| 10 | Revalidate executable identity immediately before spawn | `AH_LITELLM_EXECUTABLE_CHANGED` |

No later-stage error replaces an earlier-stage error. No child process is
created after any stage fails. The implementation's unit and integration tests
must cover each row and representative multi-fault cases to keep this ordering
stable.

## Related documentation

- [External gateway tutorial](../tutorials/external-litellm-gateway.md)
- [External gateway operations](../howto/operate-external-litellm-route.md)
- [Why the gateway stays external](../concepts/external-litellm-boundary.md)
